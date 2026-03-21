//! OpenAI provider
//!
//! Supports OpenAI-compatible APIs with full function calling (tool use).
//! Also used for compatible providers like Qwen/DashScope.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::router::{
    ChatRequest, ChatResponse, Choice, ContentBlock, LlmProvider, Message, StopReason,
    ToolDefinition, Usage,
};

/// OpenAI API configuration
///
/// Also used for OpenAI-compatible providers (e.g., Qwen/DashScope, Ollama)
/// by overriding `base_url`, `default_model`, and `name`.
#[derive(Debug, Clone)]
pub struct OpenAIConfig {
    pub api_key: String,
    pub base_url: String,
    pub default_model: String,
    pub organization: Option<String>,
    /// Provider name returned by `LlmProvider::name()` (default: "openai")
    pub name: String,
}

impl Default for OpenAIConfig {
    fn default() -> Self {
        Self {
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            base_url: "https://api.openai.com".to_string(),
            default_model: "gpt-4-turbo-preview".to_string(),
            organization: std::env::var("OPENAI_ORG_ID").ok(),
            name: "openai".to_string(),
        }
    }
}

/// OpenAI provider
pub struct OpenAIProvider {
    client: Client,
    config: OpenAIConfig,
}

impl OpenAIProvider {
    /// Create a new OpenAI provider with default configuration
    pub fn new() -> Self {
        Self::with_config(OpenAIConfig::default())
    }

    /// Create a new OpenAI provider with custom configuration
    pub fn with_config(config: OpenAIConfig) -> Self {
        Self {
            client: super::shared_http_client(),
            config,
        }
    }

    /// Convert our ChatRequest messages to OpenAI format
    fn convert_messages(messages: &[Message]) -> Vec<OpenAIMessage> {
        let mut result = Vec::new();

        for msg in messages {
            if msg.role == "system" {
                result.push(OpenAIMessage {
                    role: "system".to_string(),
                    content: Some(OpenAIContent::Text(msg.content.clone())),
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

                        let tool_calls: Vec<OpenAIToolCall> = msg
                            .content_blocks
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::ToolUse { id, name, input } => Some(OpenAIToolCall {
                                    id: id.clone(),
                                    r#type: "function".to_string(),
                                    function: OpenAIFunctionCall {
                                        name: name.clone(),
                                        arguments: serde_json::to_string(input)
                                            .unwrap_or_else(|_| "{}".to_string()),
                                    },
                                }),
                                _ => None,
                            })
                            .collect();

                        let content_text = text_parts.join("");
                        let content = if content_text.is_empty() {
                            None
                        } else {
                            Some(OpenAIContent::Text(content_text))
                        };

                        result.push(OpenAIMessage {
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
                        // User message may contain tool results and/or image blocks
                        let mut has_tool_results = false;
                        // Collect image blocks that follow tool results
                        let mut pending_images: Vec<OpenAIContentPart> = Vec::new();

                        for block in &msg.content_blocks {
                            match block {
                                ContentBlock::ToolResult {
                                    tool_use_id,
                                    content,
                                    is_error,
                                } => {
                                    // If we had pending images from a previous tool result,
                                    // flush them as a user message before this tool result
                                    if !pending_images.is_empty() {
                                        result.push(OpenAIMessage {
                                            role: "user".to_string(),
                                            content: Some(OpenAIContent::Parts(
                                                pending_images.drain(..).collect(),
                                            )),
                                            tool_calls: None,
                                            tool_call_id: None,
                                            name: None,
                                        });
                                    }

                                    has_tool_results = true;
                                    let content_str = if *is_error {
                                        format!("[Error] {}", content)
                                    } else {
                                        content.clone()
                                    };
                                    result.push(OpenAIMessage {
                                        role: "tool".to_string(),
                                        content: Some(OpenAIContent::Text(content_str)),
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
                                    // Collect image as a content part for a user message
                                    let url = if source_type == "base64" {
                                        format!("data:{};base64,{}", media_type, data)
                                    } else {
                                        data.clone()
                                    };
                                    pending_images.push(OpenAIContentPart::ImageUrl {
                                        image_url: OpenAIImageUrl {
                                            url,
                                            detail: Some("low".to_string()),
                                        },
                                    });
                                }
                                _ => {}
                            }
                        }

                        // Flush any remaining pending images as a user message
                        if !pending_images.is_empty() {
                            // Add a text part to explain the image context
                            let mut parts = vec![OpenAIContentPart::Text {
                                text: "Here is the screenshot from the tool execution above:"
                                    .to_string(),
                            }];
                            parts.extend(pending_images);
                            result.push(OpenAIMessage {
                                role: "user".to_string(),
                                content: Some(OpenAIContent::Parts(parts)),
                                tool_calls: None,
                                tool_call_id: None,
                                name: None,
                            });
                        }

                        // If there were no tool results, treat as normal user message
                        // (may include text and/or images)
                        if !has_tool_results {
                            let mut parts: Vec<OpenAIContentPart> = Vec::new();

                            for block in &msg.content_blocks {
                                match block {
                                    ContentBlock::Text { text } => {
                                        parts.push(OpenAIContentPart::Text { text: text.clone() });
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
                                        parts.push(OpenAIContentPart::ImageUrl {
                                            image_url: OpenAIImageUrl {
                                                url,
                                                detail: Some("low".to_string()),
                                            },
                                        });
                                    }
                                    _ => {}
                                }
                            }

                            if !parts.is_empty() {
                                // Use simple text if there are no images
                                let has_images = parts
                                    .iter()
                                    .any(|p| matches!(p, OpenAIContentPart::ImageUrl { .. }));
                                let content = if has_images {
                                    OpenAIContent::Parts(parts)
                                } else {
                                    // All text — join into a single string for simplicity
                                    let text = parts
                                        .iter()
                                        .filter_map(|p| match p {
                                            OpenAIContentPart::Text { text } => Some(text.as_str()),
                                            _ => None,
                                        })
                                        .collect::<Vec<_>>()
                                        .join("");
                                    OpenAIContent::Text(text)
                                };
                                result.push(OpenAIMessage {
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
                        // Other roles - use content as text
                        result.push(OpenAIMessage {
                            role: msg.role.clone(),
                            content: Some(OpenAIContent::Text(msg.content.clone())),
                            tool_calls: None,
                            tool_call_id: None,
                            name: None,
                        });
                    }
                }
            } else {
                // Simple text message (no content_blocks)
                result.push(OpenAIMessage {
                    role: msg.role.clone(),
                    content: Some(OpenAIContent::Text(msg.content.clone())),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                });
            }
        }

        // Post-process: ensure all tool_calls have corresponding tool responses
        // This fixes issues where context compaction may remove tool responses
        Self::validate_tool_call_responses(&mut result);

        result
    }

    /// Validate that all tool_calls have corresponding tool responses
    /// Remove orphaned tool_calls that don't have responses (required by Qwen/OpenAI APIs)
    fn validate_tool_call_responses(messages: &mut Vec<OpenAIMessage>) {
        use std::collections::HashSet;

        // Debug: Print message structure before validation
        tracing::debug!(
            "Validating tool_call responses for {} messages",
            messages.len()
        );

        // Collect all tool_call_ids that have responses (from tool messages)
        let responded_tool_ids: HashSet<String> = messages
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| m.tool_call_id.clone())
            .collect();

        tracing::debug!(
            responded_count = responded_tool_ids.len(),
            "Found tool responses"
        );

        // Collect all tool_call_ids from assistant messages (before filtering)
        let all_requested_tool_ids: HashSet<String> = messages
            .iter()
            .filter(|m| m.role == "assistant")
            .filter_map(|m| m.tool_calls.as_ref())
            .flat_map(|tcs| tcs.iter().map(|tc| tc.id.clone()))
            .collect();

        tracing::debug!(
            requested_count = all_requested_tool_ids.len(),
            "Found tool_calls in assistant messages"
        );

        // Find orphaned tool_calls (requested but not responded)
        let orphaned: Vec<_> = all_requested_tool_ids
            .difference(&responded_tool_ids)
            .collect();

        if !orphaned.is_empty() {
            tracing::warn!(
                orphaned_count = orphaned.len(),
                orphaned_ids = ?orphaned,
                "Found orphaned tool_calls without responses - will remove"
            );
        }

        // For each assistant message with tool_calls, filter to only those with responses
        for (idx, msg) in messages.iter_mut().enumerate() {
            if msg.role == "assistant" {
                if let Some(ref mut tool_calls) = msg.tool_calls {
                    let original_count = tool_calls.len();
                    let original_ids: Vec<_> = tool_calls.iter().map(|tc| tc.id.clone()).collect();

                    tool_calls.retain(|tc| responded_tool_ids.contains(&tc.id));

                    if tool_calls.len() != original_count {
                        tracing::warn!(
                            message_idx = idx,
                            removed = original_count - tool_calls.len(),
                            remaining = tool_calls.len(),
                            original_ids = ?original_ids,
                            "Removed orphaned tool_calls from assistant message"
                        );
                    }

                    // If all tool_calls were removed, set to None
                    if tool_calls.is_empty() {
                        msg.tool_calls = None;
                    }
                }
            }
        }

        // Rebuild the set of valid tool_call_ids after filtering
        let valid_tool_call_ids: HashSet<String> = messages
            .iter()
            .filter(|m| m.role == "assistant")
            .filter_map(|m| m.tool_calls.as_ref())
            .flat_map(|tcs| tcs.iter().map(|tc| tc.id.clone()))
            .collect();

        // Remove tool responses that don't have corresponding tool_calls
        let original_len = messages.len();
        messages.retain(|m| {
            if m.role == "tool" {
                if let Some(ref tool_call_id) = m.tool_call_id {
                    let keep = valid_tool_call_ids.contains(tool_call_id);
                    if !keep {
                        tracing::warn!(
                            tool_call_id = %tool_call_id,
                            "Removing orphaned tool response"
                        );
                    }
                    return keep;
                }
            }
            true
        });

        if messages.len() != original_len {
            tracing::warn!(
                removed = original_len - messages.len(),
                "Removed orphaned tool responses"
            );
        }

        // Final validation: check that ordering is correct
        // assistant with tool_calls must be immediately followed by tool messages
        Self::fix_message_ordering(messages);
    }

    /// Fix message ordering to ensure tool messages follow their assistant message
    fn fix_message_ordering(messages: &mut Vec<OpenAIMessage>) {
        // Build a map of tool_call_id -> tool message index
        let mut tool_msg_indices: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (idx, msg) in messages.iter().enumerate() {
            if msg.role == "tool" {
                if let Some(ref id) = msg.tool_call_id {
                    tool_msg_indices.insert(id.clone(), idx);
                }
            }
        }

        // Check each assistant message with tool_calls
        for (idx, msg) in messages.iter().enumerate() {
            if msg.role == "assistant" {
                if let Some(ref tool_calls) = msg.tool_calls {
                    for tc in tool_calls {
                        if let Some(&tool_idx) = tool_msg_indices.get(&tc.id) {
                            // The tool message should come after the assistant message
                            if tool_idx <= idx {
                                tracing::error!(
                                    assistant_idx = idx,
                                    tool_idx = tool_idx,
                                    tool_call_id = %tc.id,
                                    "INVALID: tool message appears before/at assistant message!"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Convert our ToolDefinitions to OpenAI function format
    fn convert_tools(tools: &[ToolDefinition]) -> Vec<OpenAITool> {
        tools
            .iter()
            .map(|t| OpenAITool {
                r#type: "function".to_string(),
                function: OpenAIFunction {
                    name: t.name.clone(),
                    description: Some(t.description.clone()),
                    parameters: t.input_schema.clone(),
                },
            })
            .collect()
    }

    /// Convert OpenAI response to our ChatResponse
    fn convert_response(openai_response: OpenAIResponse) -> ChatResponse {
        ChatResponse {
            id: openai_response.id,
            model: openai_response.model,
            choices: openai_response
                .choices
                .into_iter()
                .map(|c| {
                    let finish_reason = c
                        .finish_reason
                        .clone()
                        .unwrap_or_else(|| "stop".to_string());

                    // Build content blocks from the response
                    let mut content_blocks = Vec::new();

                    // Add reasoning content as Thinking block (Qwen/DeepSeek)
                    if let Some(reasoning) = &c.message.reasoning_content {
                        if !reasoning.is_empty() {
                            content_blocks.push(ContentBlock::Thinking {
                                thinking: reasoning.clone(),
                            });
                        }
                    }

                    // Add text content if present
                    if let Some(text) = &c.message.content {
                        if !text.is_empty() {
                            content_blocks.push(ContentBlock::Text { text: text.clone() });
                        }
                    }

                    // Add tool calls if present
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

                    // Determine stop reason
                    let stop_reason = match finish_reason.as_str() {
                        "stop" => Some(StopReason::EndTurn),
                        "length" => Some(StopReason::MaxTokens),
                        "tool_calls" => Some(StopReason::ToolUse),
                        _ => Some(StopReason::EndTurn),
                    };

                    // Build the Message with content_blocks
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
                prompt_tokens: openai_response.usage.prompt_tokens,
                completion_tokens: openai_response.usage.completion_tokens,
                total_tokens: openai_response.usage.total_tokens,
            },
        }
    }
}

impl Default for OpenAIProvider {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// OpenAI API Request/Response types
// ============================================================================

#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAITool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

/// OpenAI tool definition (function calling format)
#[derive(Debug, Serialize, Deserialize)]
struct OpenAITool {
    r#type: String,
    function: OpenAIFunction,
}

/// OpenAI function definition
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunction {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    parameters: serde_json::Value,
}

/// OpenAI function call in a tool_call
#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

/// OpenAI tool call in assistant message
#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenAIToolCall {
    id: String,
    r#type: String,
    function: OpenAIFunctionCall,
}

/// OpenAI message content — either a plain string or multimodal parts array.
///
/// Serializes as `"hello"` (string) or `[{"type":"text","text":"..."},...]` (array)
/// via `#[serde(untagged)]`, which is compatible with the OpenAI API format.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum OpenAIContent {
    Text(String),
    Parts(Vec<OpenAIContentPart>),
}

/// A single part inside a multimodal content array.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum OpenAIContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: OpenAIImageUrl },
}

/// Image URL object for vision models.
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIImageUrl {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

/// OpenAI message (supports text, tool_calls, tool results, and multimodal content)
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<OpenAIContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    id: String,
    choices: Vec<OpenAIChoice>,
    model: String,
    usage: OpenAIUsage,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    index: i32,
    message: OpenAIResponseMessage,
    finish_reason: Option<String>,
}

/// Response message (separate from request message for deserialization)
#[derive(Debug, Deserialize)]
struct OpenAIResponseMessage {
    #[allow(dead_code)]
    role: String,
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
    /// Reasoning content (Qwen/DeepSeek extended thinking)
    #[serde(default)]
    reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: i32,
    completion_tokens: i32,
    total_tokens: i32,
}

#[derive(Debug, Deserialize)]
struct OpenAIError {
    message: String,
    #[serde(rename = "type")]
    error_type: String,
    #[allow(dead_code)]
    code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIErrorResponse {
    error: OpenAIError,
}

// ============================================================================
// LlmProvider implementation
// ============================================================================

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| self.config.default_model.clone());

        let openai_messages = Self::convert_messages(&request.messages);
        let openai_tools = Self::convert_tools(&request.tools);

        let has_tools = !openai_tools.is_empty();

        let openai_request = OpenAIRequest {
            model: model.clone(),
            messages: openai_messages,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            tools: openai_tools,
            tool_choice: if has_tools {
                Some("auto".to_string())
            } else {
                None
            },
        };

        tracing::debug!(
            provider = %self.config.name,
            model = %model,
            tools_count = request.tools.len(),
            messages_count = request.messages.len(),
            "Sending chat request to OpenAI-compatible API"
        );

        // Log the tools being sent for debugging
        if has_tools {
            let tool_names: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
            tracing::debug!(
                provider = %self.config.name,
                tools = ?tool_names,
                "Sending tools to LLM"
            );
        }

        let mut req = self
            .client
            .post(format!("{}/v1/chat/completions", self.config.base_url))
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("content-type", "application/json");

        if let Some(org) = &self.config.organization {
            req = req.header("OpenAI-Organization", org);
        }

        let response = req.json(&openai_request).send().await?;

        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();

            // Try to parse as OpenAI error
            if let Ok(error_response) = serde_json::from_str::<OpenAIErrorResponse>(&error_text) {
                return Err(Error::Llm(format!(
                    "{} API error ({}): {}",
                    self.config.name, error_response.error.error_type, error_response.error.message
                )));
            }

            return Err(Error::Llm(format!(
                "{} API error: {} - {}",
                self.config.name, status, error_text
            )));
        }

        let openai_response: OpenAIResponse = response.json().await.map_err(|e| {
            Error::Llm(format!(
                "Failed to parse {} response: {}",
                self.config.name, e
            ))
        })?;

        let chat_response = Self::convert_response(openai_response);

        // Log tool call information for debugging
        if chat_response.requires_tool_use() {
            let tool_uses = chat_response.get_tool_uses();
            tracing::info!(
                provider = %self.config.name,
                model = %chat_response.model,
                tool_calls = tool_uses.len(),
                tool_names = ?tool_uses.iter().map(|t| &t.name).collect::<Vec<_>>(),
                "LLM requested tool calls"
            );
        }

        Ok(chat_response)
    }

    fn name(&self) -> &str {
        &self.config.name
    }

    async fn is_available(&self) -> bool {
        !self.config.api_key.is_empty()
    }
}
