//! OpenRouter provider
//!
//! Supports OpenRouter API for accessing various models including UI-TARS.
//! OpenRouter is OpenAI-compatible but requires additional headers.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::router::{
    ChatRequest, ChatResponse, Choice, ContentBlock, LlmProvider, Message, StopReason,
    ToolDefinition, Usage,
};

/// OpenRouter configuration
#[derive(Debug, Clone)]
pub struct OpenRouterConfig {
    pub api_key: String,
    pub base_url: String,
    pub default_model: String,
    /// HTTP Referer header (required by OpenRouter)
    pub http_referer: String,
    /// X-Title header (optional, for OpenRouter stats)
    pub x_title: String,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            api_key: std::env::var("OPENROUTER_API_KEY").unwrap_or_default(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            default_model: "bytedance/ui-tars-1.5-7b".to_string(),
            http_referer: "https://canal.app".to_string(),
            x_title: "Canal Browser Automation".to_string(),
        }
    }
}

/// OpenRouter provider
pub struct OpenRouterProvider {
    client: Client,
    config: OpenRouterConfig,
}

impl OpenRouterProvider {
    /// Create a new OpenRouter provider with default configuration
    pub fn new() -> Self {
        Self::with_config(OpenRouterConfig::default())
    }

    /// Create a new OpenRouter provider with custom configuration
    pub fn with_config(config: OpenRouterConfig) -> Self {
        Self {
            client: super::shared_http_client(),
            config,
        }
    }

    /// Convert our ChatRequest messages to OpenAI/OpenRouter format
    fn convert_messages(messages: &[Message]) -> Vec<OpenRouterMessage> {
        let mut result = Vec::new();

        for msg in messages {
            if msg.role == "system" {
                result.push(OpenRouterMessage {
                    role: "system".to_string(),
                    content: Some(OpenRouterContent::Text(msg.content.clone())),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                });
                continue;
            }

            // Check if the message has content_blocks with tool-related content
            if !msg.content_blocks.is_empty() {
                match msg.role.as_str() {
                    "assistant" => {
                        // Assistant message may contain text + tool_calls
                        let text_parts: Vec<&str> = msg
                            .content_blocks
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect();

                        let tool_calls: Vec<OpenRouterToolCall> = msg
                            .content_blocks
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::ToolUse { id, name, input } => {
                                    Some(OpenRouterToolCall {
                                        id: id.clone(),
                                        r#type: "function".to_string(),
                                        function: OpenRouterFunctionCall {
                                            name: name.clone(),
                                            arguments: serde_json::to_string(input)
                                                .unwrap_or_else(|_| "{}".to_string()),
                                        },
                                    })
                                }
                                _ => None,
                            })
                            .collect();

                        let content_text = text_parts.join("");
                        let content = if content_text.is_empty() {
                            None
                        } else {
                            Some(OpenRouterContent::Text(content_text))
                        };

                        result.push(OpenRouterMessage {
                            role: "assistant".to_string(),
                            content,
                            tool_calls: if tool_calls.is_empty() {
                                None
                            } else {
                                Some(tool_calls)
                            },
                            tool_call_id: None,
                            name: None,
                        });
                    }
                    "user" => {
                        // Check if this message contains tool results
                        let has_tool_results = msg
                            .content_blocks
                            .iter()
                            .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

                        if has_tool_results {
                            // Process tool results with any associated images
                            let mut pending_images: Vec<OpenRouterContentPart> = Vec::new();

                            for block in &msg.content_blocks {
                                match block {
                                    ContentBlock::ToolResult {
                                        tool_use_id,
                                        content,
                                        is_error,
                                    } => {
                                        // Flush pending images before tool result
                                        if !pending_images.is_empty() {
                                            result.push(OpenRouterMessage {
                                                role: "user".to_string(),
                                                content: Some(OpenRouterContent::Parts(
                                                    pending_images.drain(..).collect(),
                                                )),
                                                tool_calls: None,
                                                tool_call_id: None,
                                                name: None,
                                            });
                                        }

                                        let content_str = if *is_error {
                                            format!("[Error] {}", content)
                                        } else {
                                            content.clone()
                                        };
                                        result.push(OpenRouterMessage {
                                            role: "tool".to_string(),
                                            content: Some(OpenRouterContent::Text(content_str)),
                                            tool_calls: None,
                                            tool_call_id: Some(tool_use_id.clone()),
                                            name: None,
                                        });
                                    }
                                    ContentBlock::Image {
                                        source_type,
                                        media_type,
                                        data,
                                    } => {
                                        let url = if source_type == "base64" {
                                            format!("data:{};base64,{}", media_type, data)
                                        } else {
                                            data.clone()
                                        };
                                        pending_images.push(OpenRouterContentPart::ImageUrl {
                                            image_url: OpenRouterImageUrl {
                                                url,
                                                detail: Some("auto".to_string()),
                                            },
                                        });
                                    }
                                    _ => {}
                                }
                            }

                            // Flush remaining pending images after tool results
                            if !pending_images.is_empty() {
                                let mut parts = vec![OpenRouterContentPart::Text {
                                    text: "Here is the screenshot from the tool execution above:"
                                        .to_string(),
                                }];
                                parts.extend(pending_images);
                                result.push(OpenRouterMessage {
                                    role: "user".to_string(),
                                    content: Some(OpenRouterContent::Parts(parts)),
                                    tool_calls: None,
                                    tool_call_id: None,
                                    name: None,
                                });
                            }
                        } else {
                            // Normal user message (text and/or images, no tool results)
                            let mut parts: Vec<OpenRouterContentPart> = Vec::new();

                            for block in &msg.content_blocks {
                                match block {
                                    ContentBlock::Text { text } => {
                                        parts.push(OpenRouterContentPart::Text {
                                            text: text.clone(),
                                        });
                                    }
                                    ContentBlock::Image {
                                        source_type,
                                        media_type,
                                        data,
                                    } => {
                                        let url = if source_type == "base64" {
                                            format!("data:{};base64,{}", media_type, data)
                                        } else {
                                            data.clone()
                                        };
                                        parts.push(OpenRouterContentPart::ImageUrl {
                                            image_url: OpenRouterImageUrl {
                                                url,
                                                detail: Some("auto".to_string()),
                                            },
                                        });
                                    }
                                    _ => {}
                                }
                            }

                            if !parts.is_empty() {
                                let has_images = parts
                                    .iter()
                                    .any(|p| matches!(p, OpenRouterContentPart::ImageUrl { .. }));
                                let content = if has_images {
                                    OpenRouterContent::Parts(parts)
                                } else {
                                    let text = parts
                                        .iter()
                                        .filter_map(|p| match p {
                                            OpenRouterContentPart::Text { text } => {
                                                Some(text.as_str())
                                            }
                                            _ => None,
                                        })
                                        .collect::<Vec<_>>()
                                        .join("");
                                    OpenRouterContent::Text(text)
                                };
                                result.push(OpenRouterMessage {
                                    role: "user".to_string(),
                                    content: Some(content),
                                    tool_calls: None,
                                    tool_call_id: None,
                                    name: None,
                                });
                            }
                        }
                    }
                    _ => {
                        result.push(OpenRouterMessage {
                            role: msg.role.clone(),
                            content: Some(OpenRouterContent::Text(msg.content.clone())),
                            tool_calls: None,
                            tool_call_id: None,
                            name: None,
                        });
                    }
                }
            } else {
                // Simple text message (no content_blocks)
                result.push(OpenRouterMessage {
                    role: msg.role.clone(),
                    content: Some(OpenRouterContent::Text(msg.content.clone())),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                });
            }
        }

        // Validate tool call responses
        Self::validate_tool_call_responses(&mut result);

        result
    }

    /// Validate that all tool_calls have corresponding tool responses
    fn validate_tool_call_responses(messages: &mut Vec<OpenRouterMessage>) {
        use std::collections::HashSet;

        // Collect all tool_call_ids that have responses
        let responded_tool_ids: HashSet<String> = messages
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| m.tool_call_id.clone())
            .collect();

        // Filter orphaned tool_calls from assistant messages
        for msg in messages.iter_mut() {
            if msg.role == "assistant" {
                if let Some(ref mut tool_calls) = msg.tool_calls {
                    tool_calls.retain(|tc| responded_tool_ids.contains(&tc.id));
                    if tool_calls.is_empty() {
                        msg.tool_calls = None;
                    }
                }
            }
        }

        // Rebuild valid tool_call_ids
        let valid_tool_call_ids: HashSet<String> = messages
            .iter()
            .filter(|m| m.role == "assistant")
            .filter_map(|m| m.tool_calls.as_ref())
            .flat_map(|tcs| tcs.iter().map(|tc| tc.id.clone()))
            .collect();

        // Remove orphaned tool responses
        messages.retain(|m| {
            if m.role == "tool" {
                if let Some(ref tool_call_id) = m.tool_call_id {
                    return valid_tool_call_ids.contains(tool_call_id);
                }
            }
            true
        });
    }

    /// Convert our ToolDefinitions to OpenAI/OpenRouter function format
    fn convert_tools(tools: &[ToolDefinition]) -> Vec<OpenRouterTool> {
        tools
            .iter()
            .map(|t| OpenRouterTool {
                r#type: "function".to_string(),
                function: OpenRouterFunction {
                    name: t.name.clone(),
                    description: Some(t.description.clone()),
                    parameters: t.input_schema.clone(),
                },
            })
            .collect()
    }

    /// Convert OpenRouter response to our ChatResponse
    fn convert_response(response: OpenRouterResponse) -> ChatResponse {
        ChatResponse {
            id: response.id,
            model: response.model,
            choices: response
                .choices
                .into_iter()
                .map(|c| {
                    let finish_reason = c
                        .finish_reason
                        .clone()
                        .unwrap_or_else(|| "stop".to_string());

                    let mut content_blocks = Vec::new();

                    if let Some(text) = &c.message.content {
                        if !text.is_empty() {
                            content_blocks.push(ContentBlock::Text { text: text.clone() });
                        }
                    }

                    if let Some(tool_calls) = &c.message.tool_calls {
                        for tc in tool_calls {
                            let input: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments)
                                    .unwrap_or_else(|_| serde_json::json!({}));
                            content_blocks.push(ContentBlock::ToolUse {
                                id: tc.id.clone(),
                                name: tc.function.name.clone(),
                                input,
                            });
                        }
                    }

                    let stop_reason = match finish_reason.as_str() {
                        "stop" => Some(StopReason::EndTurn),
                        "length" => Some(StopReason::MaxTokens),
                        "tool_calls" => Some(StopReason::ToolUse),
                        _ => Some(StopReason::EndTurn),
                    };

                    let text_content = c.message.content.unwrap_or_default();
                    let message = if content_blocks.is_empty() {
                        Message::text("assistant", text_content)
                    } else {
                        Message::with_blocks("assistant", content_blocks)
                    };

                    Choice {
                        index: c.index,
                        message,
                        finish_reason: finish_reason.clone(),
                        stop_reason,
                    }
                })
                .collect(),
            usage: Usage {
                prompt_tokens: response.usage.prompt_tokens,
                completion_tokens: response.usage.completion_tokens,
                total_tokens: response.usage.total_tokens,
            },
        }
    }
}

impl Default for OpenRouterProvider {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// OpenRouter API Request/Response types (OpenAI-compatible)
// ============================================================================

#[derive(Debug, Serialize)]
struct OpenRouterRequest {
    model: String,
    messages: Vec<OpenRouterMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenRouterTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenRouterTool {
    r#type: String,
    function: OpenRouterFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenRouterFunction {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenRouterFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenRouterToolCall {
    id: String,
    r#type: String,
    function: OpenRouterFunctionCall,
}

/// OpenRouter message content — either a plain string or multimodal parts array
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum OpenRouterContent {
    Text(String),
    Parts(Vec<OpenRouterContentPart>),
}

/// A single part inside a multimodal content array
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum OpenRouterContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: OpenRouterImageUrl },
}

/// Image URL object for vision models
#[derive(Debug, Serialize, Deserialize)]
struct OpenRouterImageUrl {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

/// OpenRouter message (supports text, tool_calls, tool results, and multimodal content)
#[derive(Debug, Serialize, Deserialize)]
struct OpenRouterMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<OpenRouterContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenRouterToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterResponse {
    id: String,
    choices: Vec<OpenRouterChoice>,
    model: String,
    usage: OpenRouterUsage,
}

#[derive(Debug, Deserialize)]
struct OpenRouterChoice {
    index: i32,
    message: OpenRouterResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterResponseMessage {
    #[allow(dead_code)]
    role: String,
    content: Option<String>,
    tool_calls: Option<Vec<OpenRouterToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterUsage {
    prompt_tokens: i32,
    completion_tokens: i32,
    total_tokens: i32,
}

#[derive(Debug, Deserialize)]
struct OpenRouterError {
    message: String,
    #[serde(rename = "type")]
    error_type: Option<String>,
    #[allow(dead_code)]
    code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterErrorResponse {
    error: OpenRouterError,
}

// ============================================================================
// LlmProvider implementation
// ============================================================================

#[async_trait]
impl LlmProvider for OpenRouterProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| self.config.default_model.clone());

        let openrouter_messages = Self::convert_messages(&request.messages);
        let openrouter_tools = Self::convert_tools(&request.tools);

        let has_tools = !openrouter_tools.is_empty();

        let openrouter_request = OpenRouterRequest {
            model: model.clone(),
            messages: openrouter_messages,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            tools: openrouter_tools,
            tool_choice: if has_tools {
                Some("auto".to_string())
            } else {
                None
            },
        };

        tracing::debug!(
            provider = "openrouter",
            model = %model,
            tools_count = request.tools.len(),
            messages_count = request.messages.len(),
            "Sending chat request to OpenRouter API"
        );

        if has_tools {
            let tool_names: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
            tracing::debug!(
                provider = "openrouter",
                tools = ?tool_names,
                "Sending tools to LLM"
            );
        }

        let response = self
            .client
            .post(format!("{}/chat/completions", self.config.base_url))
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            // OpenRouter-specific headers
            .header("HTTP-Referer", &self.config.http_referer)
            .header("X-Title", &self.config.x_title)
            .json(&openrouter_request)
            .send()
            .await?;

        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();

            if let Ok(error_response) = serde_json::from_str::<OpenRouterErrorResponse>(&error_text)
            {
                let error_type = error_response
                    .error
                    .error_type
                    .unwrap_or_else(|| "unknown".to_string());
                return Err(Error::Llm(format!(
                    "OpenRouter API error ({}): {}",
                    error_type, error_response.error.message
                )));
            }

            return Err(Error::Llm(format!(
                "OpenRouter API error: {} - {}",
                status, error_text
            )));
        }

        let openrouter_response: OpenRouterResponse = response
            .json()
            .await
            .map_err(|e| Error::Llm(format!("Failed to parse OpenRouter response: {}", e)))?;

        let chat_response = Self::convert_response(openrouter_response);

        if chat_response.requires_tool_use() {
            let tool_uses = chat_response.get_tool_uses();
            tracing::info!(
                provider = "openrouter",
                model = %chat_response.model,
                tool_calls = tool_uses.len(),
                tool_names = ?tool_uses.iter().map(|t| &t.name).collect::<Vec<_>>(),
                "LLM requested tool calls"
            );
        }

        Ok(chat_response)
    }

    fn name(&self) -> &str {
        "openrouter"
    }

    async fn is_available(&self) -> bool {
        !self.config.api_key.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = OpenRouterConfig::default();
        assert_eq!(config.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(config.default_model, "bytedance/ui-tars-1.5-7b");
        assert_eq!(config.http_referer, "https://canal.app");
        assert_eq!(config.x_title, "Canal Browser Automation");
    }

    #[test]
    fn test_convert_simple_message() {
        let messages = vec![Message::text("user", "Hello, world!")];
        let converted = OpenRouterProvider::convert_messages(&messages);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "user");
        match &converted[0].content {
            Some(OpenRouterContent::Text(text)) => assert_eq!(text, "Hello, world!"),
            _ => panic!("Expected text content"),
        }
    }

    #[test]
    fn test_convert_multimodal_message() {
        let messages = vec![Message::with_blocks(
            "user",
            vec![
                ContentBlock::Text {
                    text: "What is in this image?".to_string(),
                },
                ContentBlock::Image {
                    source_type: "base64".to_string(),
                    media_type: "image/png".to_string(),
                    data: "iVBORw0KGgo=".to_string(),
                },
            ],
        )];
        let converted = OpenRouterProvider::convert_messages(&messages);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "user");
        match &converted[0].content {
            Some(OpenRouterContent::Parts(parts)) => {
                assert_eq!(parts.len(), 2);
            }
            _ => panic!("Expected parts content"),
        }
    }

    #[test]
    fn test_provider_name() {
        let provider = OpenRouterProvider::new();
        assert_eq!(provider.name(), "openrouter");
    }
}
