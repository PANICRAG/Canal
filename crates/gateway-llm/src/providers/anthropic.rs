//! Anthropic Claude provider
//!
//! This provider implements the Anthropic Messages API with full tool use support.
//! See: <https://docs.anthropic.com/en/docs/tool-use>

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::router::{
    ChatRequest, ChatResponse, Choice, ContentBlock, LlmProvider, Message, StopReason, StreamChunk,
    StreamResponse, ToolChoice, Usage,
};

/// Anthropic API configuration
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    pub api_key: String,
    pub base_url: String,
    pub default_model: String,
    pub api_version: String,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key: std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
            base_url: "https://api.anthropic.com".to_string(),
            default_model: "claude-sonnet-4-6".to_string(),
            api_version: "2023-06-01".to_string(),
        }
    }
}

/// Anthropic provider
pub struct AnthropicProvider {
    client: Client,
    config: AnthropicConfig,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider with default configuration
    pub fn new() -> Self {
        Self::with_config(AnthropicConfig::default())
    }

    /// Create a new Anthropic provider with custom configuration
    pub fn with_config(config: AnthropicConfig) -> Self {
        Self {
            client: super::shared_http_client(),
            config,
        }
    }
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Request Types
// ============================================================================

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<AnthropicToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking>,
}

/// Extended thinking configuration for Anthropic API
#[derive(Debug, Serialize)]
struct AnthropicThinking {
    r#type: String,
    budget_tokens: u32,
}

/// Tool definition in Anthropic format
#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

/// Tool choice configuration
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum AnthropicToolChoice {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "any")]
    Any,
    #[serde(rename = "tool")]
    Tool { name: String },
}

/// Message with content blocks (for tool_use/tool_result)
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

/// Content can be a simple string or an array of content blocks
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

/// Content block in Anthropic format
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },

    #[serde(rename = "image")]
    Image { source: AnthropicImageSource },
}

/// Image source for Anthropic API
#[derive(Debug, Serialize, Deserialize)]
pub struct AnthropicImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

// ============================================================================
// Response Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    id: String,
    content: Vec<AnthropicResponseBlock>,
    #[allow(dead_code)]
    model: String,
    usage: AnthropicUsage,
    stop_reason: Option<String>,
}

/// Response content block from Anthropic
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    #[serde(rename = "thinking")]
    Thinking { thinking: String },
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: i32,
    output_tokens: i32,
}

#[derive(Debug, Deserialize)]
struct AnthropicError {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorResponse {
    error: AnthropicError,
}

// ============================================================================
// Streaming Response Types
// ============================================================================

/// SSE event from Anthropic streaming API
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum StreamingEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: StreamingMessage },

    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: StreamingContentBlock,
    },

    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: StreamingDelta },

    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },

    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaContent,
        usage: Option<StreamingUsage>,
    },

    #[serde(rename = "message_stop")]
    MessageStop,

    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "error")]
    Error { error: AnthropicError },
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct StreamingMessage {
    id: String,
    model: String,
    usage: StreamingUsage,
}

#[derive(Debug, Deserialize)]
struct StreamingUsage {
    input_tokens: i32,
    #[serde(default)]
    output_tokens: i32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum StreamingContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },

    #[serde(rename = "thinking")]
    Thinking { thinking: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum StreamingDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },

    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },

    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
}

#[derive(Debug, Deserialize)]
struct MessageDeltaContent {
    stop_reason: Option<String>,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let model = request
            .model
            .unwrap_or_else(|| self.config.default_model.clone());
        let max_tokens = request.max_tokens.unwrap_or(4096);

        // Extract system message if present
        let system_message = request
            .messages
            .iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.clone());

        // Convert messages to Anthropic format
        let messages: Vec<AnthropicMessage> = request
            .messages
            .into_iter()
            .filter(|m| m.role != "system")
            .map(|m| convert_message_to_anthropic(&m))
            .collect();

        // Convert tools to Anthropic format
        let mut tools: Vec<AnthropicTool> = request
            .tools
            .into_iter()
            .map(|t| AnthropicTool {
                name: t.name,
                description: t.description,
                input_schema: t.input_schema,
            })
            .collect();

        // Convert tool choice
        // R3-M: ToolChoice::None → don't send tools at all (Anthropic has no "none" mode)
        let is_tool_choice_none = matches!(request.tool_choice, Some(ToolChoice::None));
        let tool_choice = if is_tool_choice_none {
            None
        } else {
            request.tool_choice.map(|tc| match tc {
                ToolChoice::Auto => AnthropicToolChoice::Auto,
                ToolChoice::Any => AnthropicToolChoice::Any,
                ToolChoice::Tool { name } => AnthropicToolChoice::Tool { name },
                ToolChoice::None => unreachable!(),
            })
        };
        if is_tool_choice_none {
            tools = Vec::new();
        }

        let thinking = request.thinking_budget.map(|budget| AnthropicThinking {
            r#type: "enabled".to_string(),
            budget_tokens: budget,
        });

        let anthropic_request = AnthropicRequest {
            model: model.clone(),
            max_tokens,
            messages,
            temperature: if thinking.is_some() {
                None
            } else {
                request.temperature
            },
            system: system_message,
            tools,
            tool_choice,
            thinking,
        };

        tracing::debug!(
            "Sending Anthropic request: {}",
            serde_json::to_string_pretty(&anthropic_request).unwrap_or_default()
        );

        let response = self
            .client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", &self.config.api_version)
            .header("content-type", "application/json")
            .json(&anthropic_request)
            .send()
            .await?;

        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();

            // Try to parse as Anthropic error
            if let Ok(error_response) = serde_json::from_str::<AnthropicErrorResponse>(&error_text)
            {
                return Err(Error::Llm(format!(
                    "Anthropic API error ({}): {}",
                    error_response.error.error_type, error_response.error.message
                )));
            }

            return Err(Error::Llm(format!(
                "Anthropic API error: {} - {}",
                status, error_text
            )));
        }

        let anthropic_response: AnthropicResponse = response
            .json()
            .await
            .map_err(|e| Error::Llm(format!("Failed to parse Anthropic response: {}", e)))?;

        // Convert response blocks to our ContentBlock format
        let content_blocks: Vec<ContentBlock> = anthropic_response
            .content
            .iter()
            .map(|block| match block {
                AnthropicResponseBlock::Text { text } => ContentBlock::Text { text: text.clone() },
                AnthropicResponseBlock::ToolUse { id, name, input } => ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                },
                AnthropicResponseBlock::Thinking { thinking } => ContentBlock::Thinking {
                    thinking: thinking.clone(),
                },
            })
            .collect();

        // Build the response message
        let message = if content_blocks.is_empty() {
            Message::text("assistant", "")
        } else {
            Message::with_blocks("assistant", content_blocks)
        };

        // Determine stop reason
        let stop_reason = anthropic_response.stop_reason.as_deref().map(|s| match s {
            "end_turn" => StopReason::EndTurn,
            "max_tokens" => StopReason::MaxTokens,
            "tool_use" => StopReason::ToolUse,
            "stop_sequence" => StopReason::StopSequence,
            _ => StopReason::EndTurn,
        });

        Ok(ChatResponse {
            id: anthropic_response.id,
            model,
            choices: vec![Choice {
                index: 0,
                message,
                finish_reason: anthropic_response
                    .stop_reason
                    .clone()
                    .unwrap_or_else(|| "stop".to_string()),
                stop_reason,
            }],
            usage: Usage {
                prompt_tokens: anthropic_response.usage.input_tokens,
                completion_tokens: anthropic_response.usage.output_tokens,
                total_tokens: anthropic_response.usage.input_tokens
                    + anthropic_response.usage.output_tokens,
            },
        })
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<StreamResponse> {
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| self.config.default_model.clone());
        let max_tokens = request.max_tokens.unwrap_or(4096);

        // Extract system message if present
        let system_message = request
            .messages
            .iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.clone());

        // Convert messages to Anthropic format
        let messages: Vec<AnthropicMessage> = request
            .messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| convert_message_to_anthropic(m))
            .collect();

        // Convert tools to Anthropic format
        // R3-M: ToolChoice::None → don't send tools at all (Anthropic has no "none" mode)
        let is_tool_choice_none = matches!(request.tool_choice, Some(ToolChoice::None));
        let tools: Vec<AnthropicTool> = if is_tool_choice_none {
            Vec::new()
        } else {
            request
                .tools
                .iter()
                .map(|t| AnthropicTool {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t.input_schema.clone(),
                })
                .collect()
        };
        let tool_choice = if is_tool_choice_none {
            None
        } else {
            request.tool_choice.as_ref().map(|tc| match tc {
                ToolChoice::Auto => AnthropicToolChoice::Auto,
                ToolChoice::Any => AnthropicToolChoice::Any,
                ToolChoice::Tool { name } => AnthropicToolChoice::Tool { name: name.clone() },
                ToolChoice::None => unreachable!(),
            })
        };

        // Build thinking configuration
        let thinking = request.thinking_budget.map(|budget| AnthropicThinking {
            r#type: "enabled".to_string(),
            budget_tokens: budget,
        });

        // Build request body with stream: true
        // When thinking is enabled, temperature must not be set (Anthropic API constraint)
        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": messages,
            "stream": true
        });
        let obj = body.as_object_mut().unwrap();
        if thinking.is_none() {
            if let Some(temp) = request.temperature {
                obj.insert("temperature".into(), serde_json::json!(temp));
            }
        } else {
            obj.insert("thinking".into(), serde_json::json!(thinking));
        }
        if let Some(sys) = &system_message {
            obj.insert("system".into(), serde_json::json!(sys));
        }
        if !tools.is_empty() {
            obj.insert("tools".into(), serde_json::json!(tools));
        }
        if let Some(tc) = &tool_choice {
            obj.insert("tool_choice".into(), serde_json::json!(tc));
        }

        tracing::debug!("Sending streaming Anthropic request");

        let response = self
            .client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", &self.config.api_version)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            if let Ok(error_response) = serde_json::from_str::<AnthropicErrorResponse>(&error_text)
            {
                return Err(Error::Llm(format!(
                    "Anthropic API error ({}): {}",
                    error_response.error.error_type, error_response.error.message
                )));
            }
            return Err(Error::Llm(format!(
                "Anthropic API error: {} - {}",
                status, error_text
            )));
        }

        // Process SSE stream
        let byte_stream = response.bytes_stream();
        let stream = parse_sse_stream(byte_stream);

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    async fn is_available(&self) -> bool {
        !self.config.api_key.is_empty()
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Parse SSE stream from Anthropic into StreamChunks
fn parse_sse_stream(
    byte_stream: impl Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> impl Stream<Item = Result<StreamChunk>> + Send {
    async_stream::stream! {
        use std::collections::HashMap;

        let mut byte_stream = Box::pin(byte_stream);
        let mut buffer = String::new();
        let mut input_tokens = 0i32;
        let mut output_tokens = 0i32;
        let mut tool_inputs: HashMap<String, String> = HashMap::new();
        let mut current_tool_id: Option<String> = None;
        let mut stop_reason: Option<StopReason> = None;

        while let Some(result) = byte_stream.next().await {
            match result {
                Ok(chunk) => {
                    buffer.push_str(&String::from_utf8_lossy(&chunk));

                    // R3-M: Reject streams that accumulate beyond 10MB buffer limit
                    if buffer.len() > 10 * 1024 * 1024 {
                        yield Err(Error::Llm(
                            "SSE buffer exceeded 10MB limit".to_string(),
                        ));
                        return;
                    }

                    // Process complete SSE events from buffer
                    while let Some(pos) = buffer.find("\n\n") {
                        let event_str = buffer[..pos].to_string();
                        buffer = buffer[pos + 2..].to_string();

                        // Parse SSE event
                        if let Some(data) = event_str
                            .lines()
                            .find(|l| l.starts_with("data: "))
                            .map(|l| &l[6..])
                        {
                            if data == "[DONE]" {
                                yield Ok(StreamChunk::Done {
                                    usage: Usage {
                                        prompt_tokens: input_tokens,
                                        completion_tokens: output_tokens,
                                        total_tokens: input_tokens + output_tokens,
                                    },
                                    stop_reason: stop_reason.clone(),
                                });
                                continue;
                            }

                            if let Ok(event) = serde_json::from_str::<StreamingEvent>(data) {
                                match event {
                                    StreamingEvent::MessageStart { message } => {
                                        input_tokens = message.usage.input_tokens;
                                    }
                                    StreamingEvent::ContentBlockStart { content_block, .. } => {
                                        match content_block {
                                            StreamingContentBlock::ToolUse { id, name } => {
                                                tool_inputs.insert(id.clone(), String::new());
                                                current_tool_id = Some(id.clone());
                                                yield Ok(StreamChunk::ToolUseStart { id, name });
                                            }
                                            StreamingContentBlock::Thinking { thinking } => {
                                                // Initial thinking block content
                                                if !thinking.is_empty() {
                                                    yield Ok(StreamChunk::ThinkingDelta { text: thinking });
                                                }
                                            }
                                            StreamingContentBlock::Text { .. } => {
                                                // Text block start, content comes via deltas
                                            }
                                        }
                                    }
                                    StreamingEvent::ContentBlockDelta { delta, .. } => {
                                        match delta {
                                            StreamingDelta::TextDelta { text } => {
                                                yield Ok(StreamChunk::TextDelta { text });
                                            }
                                            StreamingDelta::InputJsonDelta { partial_json } => {
                                                if let Some(ref id) = current_tool_id {
                                                    if let Some(input) = tool_inputs.get_mut(id) {
                                                        input.push_str(&partial_json);
                                                    }
                                                    yield Ok(StreamChunk::ToolUseInputDelta {
                                                        id: id.clone(),
                                                        input_delta: partial_json,
                                                    });
                                                }
                                            }
                                            StreamingDelta::ThinkingDelta { thinking } => {
                                                yield Ok(StreamChunk::ThinkingDelta { text: thinking });
                                            }
                                        }
                                    }
                                    StreamingEvent::ContentBlockStop { .. } => {
                                        // Emit completed tool use
                                        if let Some(ref id) = current_tool_id {
                                            if let Some(input) = tool_inputs.get(id) {
                                                if !input.is_empty() {
                                                    if let Ok(parsed) = serde_json::from_str(input) {
                                                        yield Ok(StreamChunk::ToolUseComplete {
                                                            id: id.clone(),
                                                            input: parsed,
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                        current_tool_id = None;
                                    }
                                    StreamingEvent::MessageDelta { delta, usage } => {
                                        if let Some(u) = usage {
                                            output_tokens = u.output_tokens;
                                        }
                                        if let Some(stop) = delta.stop_reason {
                                            stop_reason = Some(match stop.as_str() {
                                                "end_turn" => StopReason::EndTurn,
                                                "max_tokens" => StopReason::MaxTokens,
                                                "tool_use" => StopReason::ToolUse,
                                                "stop_sequence" => StopReason::StopSequence,
                                                _ => StopReason::EndTurn,
                                            });
                                        }
                                    }
                                    StreamingEvent::MessageStop => {
                                        yield Ok(StreamChunk::Done {
                                            usage: Usage {
                                                prompt_tokens: input_tokens,
                                                completion_tokens: output_tokens,
                                                total_tokens: input_tokens + output_tokens,
                                            },
                                            stop_reason: stop_reason.clone(),
                                        });
                                    }
                                    StreamingEvent::Error { error } => {
                                        yield Err(Error::Llm(format!("Stream error: {}", error.message)));
                                    }
                                    StreamingEvent::Ping => {}
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    yield Err(Error::Llm(format!("Stream error: {}", e)));
                    break;
                }
            }
        }
    }
}

/// Convert our Message format to Anthropic format
fn convert_message_to_anthropic(msg: &Message) -> AnthropicMessage {
    let role = if msg.role == "assistant" {
        "assistant".to_string()
    } else {
        "user".to_string()
    };

    // If the message has content blocks, convert them
    if !msg.content_blocks.is_empty() {
        let blocks: Vec<AnthropicContentBlock> = msg
            .content_blocks
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => AnthropicContentBlock::Text { text: text.clone() },
                ContentBlock::ToolUse { id, name, input } => AnthropicContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                },
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => AnthropicContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                },
                ContentBlock::Image {
                    source_type,
                    media_type,
                    data,
                } => AnthropicContentBlock::Image {
                    source: AnthropicImageSource {
                        source_type: source_type.clone(),
                        media_type: media_type.clone(),
                        data: data.clone(),
                    },
                },
                // Thinking blocks are not sent back to Anthropic in messages
                ContentBlock::Thinking { thinking } => AnthropicContentBlock::Text {
                    text: thinking.clone(),
                },
            })
            .collect();

        AnthropicMessage {
            role,
            content: AnthropicContent::Blocks(blocks),
        }
    } else {
        // Simple text message
        AnthropicMessage {
            role,
            content: AnthropicContent::Text(msg.content.clone()),
        }
    }
}
