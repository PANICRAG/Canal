//! Anthropic API client for the task worker
//!
//! A lightweight Anthropic client implementation for streaming chat completions
//! with tool use support.

use futures::{Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

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

/// Anthropic client
#[derive(Clone)]
pub struct AnthropicClient {
    client: Client,
    config: AnthropicConfig,
}

impl AnthropicClient {
    /// Create a new Anthropic client
    pub fn new(config: AnthropicConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }

    /// Send a streaming messages request
    pub async fn messages_stream(
        &self,
        request: AnthropicRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamingEvent, String>> + Send>>, String> {
        let body = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": request.messages,
            "system": request.system,
            "tools": request.tools,
            "stream": true
        });

        tracing::debug!(
            "Sending Anthropic streaming request to model: {}",
            request.model
        );

        let response = self
            .client
            .post(format!("{}/v1/messages", self.config.base_url))
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", &self.config.api_version)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();

            // Try to parse as Anthropic error
            if let Ok(error_response) = serde_json::from_str::<AnthropicErrorResponse>(&error_text)
            {
                return Err(format!(
                    "Anthropic API error ({}): {}",
                    error_response.error.error_type, error_response.error.message
                ));
            }

            return Err(format!("Anthropic API error: {} - {}", status, error_text));
        }

        // Process SSE stream
        let byte_stream = response.bytes_stream();
        let stream = parse_sse_stream(byte_stream);

        Ok(Box::pin(stream))
    }
}

// ============================================================================
// Request Types
// ============================================================================

/// Anthropic messages request
#[derive(Debug, Clone, Serialize)]
pub struct AnthropicRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<AnthropicTool>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stream: bool,
}

/// Tool definition in Anthropic format
#[derive(Debug, Clone, Serialize)]
pub struct AnthropicTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Message with content blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessage {
    pub role: String,
    pub content: AnthropicContent,
}

impl AnthropicMessage {
    /// Create a simple user message
    pub fn user(text: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: AnthropicContent::Text(text.to_string()),
        }
    }

    /// Create a simple assistant message
    pub fn assistant(text: &str) -> Self {
        Self {
            role: "assistant".to_string(),
            content: AnthropicContent::Text(text.to_string()),
        }
    }

    /// Create a message with content blocks
    pub fn with_blocks(role: &str, blocks: Vec<AnthropicContentBlock>) -> Self {
        Self {
            role: role.to_string(),
            content: AnthropicContent::Blocks(blocks),
        }
    }
}

/// Content can be a simple string or an array of content blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnthropicContent {
    Text(String),
    Blocks(Vec<AnthropicContentBlock>),
}

/// Content block in Anthropic format
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AnthropicContentBlock {
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
}

// ============================================================================
// Error Types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct AnthropicError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct AnthropicErrorResponse {
    pub error: AnthropicError,
}

// ============================================================================
// Streaming Response Types
// ============================================================================

/// SSE event from Anthropic streaming API
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum StreamingEvent {
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
pub struct StreamingMessage {
    pub id: String,
    #[allow(dead_code)]
    pub model: String,
    pub usage: StreamingUsage,
}

#[derive(Debug, Deserialize)]
pub struct StreamingUsage {
    pub input_tokens: i32,
    #[serde(default)]
    pub output_tokens: i32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum StreamingContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum StreamingDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },

    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
pub struct MessageDeltaContent {
    pub stop_reason: Option<String>,
}

// ============================================================================
// Stream Parsing
// ============================================================================

/// Parse SSE stream from Anthropic into StreamingEvents
fn parse_sse_stream(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> impl Stream<Item = Result<StreamingEvent, String>> + Send {
    async_stream::stream! {
        let mut byte_stream = Box::pin(byte_stream);
        let mut buffer = String::new();

        while let Some(result) = byte_stream.next().await {
            match result {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));

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
                                continue;
                            }

                            match serde_json::from_str::<StreamingEvent>(data) {
                                Ok(event) => {
                                    yield Ok(event);
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        data = %data,
                                        "Failed to parse streaming event"
                                    );
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    yield Err(format!("Stream error: {}", e));
                    break;
                }
            }
        }
    }
}
