//! Message Types - Claude Agent SDK Compatible
//!
//! These types are designed to be wire-compatible with Claude Agent SDK protocol.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::content::ContentBlock;
use super::permissions::PermissionRequest;

/// Union of all message types in the agent protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentMessage {
    #[serde(rename = "user")]
    User(UserMessage),

    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),

    #[serde(rename = "system")]
    System(SystemMessage),

    #[serde(rename = "result")]
    Result(ResultMessage),

    #[serde(rename = "stream_event")]
    StreamEvent(StreamEventMessage),

    /// Permission request requiring user approval
    #[serde(rename = "permission_request")]
    PermissionRequest(PermissionRequest),
}

/// User message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    /// Message content (text or content blocks)
    pub content: MessageContent,
    /// Unique identifier for this message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<Uuid>,
    /// If this is a tool result, the parent tool_use ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
    /// Tool result data (if this message contains a tool result)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_result: Option<serde_json::Value>,
}

/// Assistant message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    /// Content blocks (text, tool_use, thinking, etc.)
    pub content: Vec<ContentBlock>,
    /// Model that generated this message
    pub model: String,
    /// If this is part of a tool use flow, the parent tool_use ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_tool_use_id: Option<String>,
    /// Error information if the message represents an error
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AssistantMessageError>,
}

/// System message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    /// Subtype of the system message
    pub subtype: String,
    /// Additional data for the system message
    pub data: serde_json::Value,
}

/// Result message - sent when agent loop completes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultMessage {
    /// Result subtype
    pub subtype: ResultSubtype,
    /// Total duration of the agent loop
    pub duration_ms: u64,
    /// Duration spent on API calls
    pub duration_api_ms: u64,
    /// Whether the result is an error
    pub is_error: bool,
    /// Number of turns in the conversation
    pub num_turns: u32,
    /// Session identifier
    pub session_id: String,
    /// Total cost in USD
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    /// Token usage statistics
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Final result text
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Structured output (if requested)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
}

/// Stream event message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEventMessage {
    /// Event subtype
    pub subtype: StreamEventSubtype,
    /// Event data
    pub data: serde_json::Value,
}

/// Message content - can be text or content blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Plain text content
    Text(String),
    /// Array of content blocks
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Create text content
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }

    /// Create content from blocks
    pub fn blocks(blocks: Vec<ContentBlock>) -> Self {
        Self::Blocks(blocks)
    }

    /// Get as text string
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::Blocks(_) => None,
        }
    }

    /// Convert to string representation
    pub fn to_string_content(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| b.as_text())
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

/// Result subtypes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSubtype {
    /// Successful completion
    Success,
    /// Exceeded maximum turns
    ErrorMaxTurns,
    /// Error during execution
    ErrorDuringExecution,
    /// Exceeded maximum budget
    ErrorMaxBudgetUsd,
    /// Exceeded structured output retries
    ErrorMaxStructuredOutputRetries,
    /// User interrupted
    Interrupted,
    /// Waiting for user permission approval
    WaitingForPermission,
}

/// Stream event subtypes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamEventSubtype {
    /// Text chunk
    TextDelta,
    /// Tool use started
    ToolUseStart,
    /// Tool use input delta
    ToolUseInputDelta,
    /// Tool use completed
    ToolUseComplete,
    /// Tool result
    ToolResult,
    /// Thinking
    Thinking,
    /// Message complete
    MessageComplete,
    /// Error
    Error,
}

/// Assistant message error types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantMessageError {
    /// Authentication failed
    AuthenticationFailed,
    /// Billing error
    BillingError,
    /// Rate limit exceeded
    RateLimit,
    /// Invalid request
    InvalidRequest,
    /// Server error
    ServerError,
    /// Unknown error
    Unknown,
}

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens
    pub input_tokens: u32,
    /// Output tokens
    pub output_tokens: u32,
    /// Cache creation input tokens
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    /// Cache read input tokens
    #[serde(default)]
    pub cache_read_input_tokens: u32,
}

impl Usage {
    /// Total tokens
    pub fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }

    /// Add another usage to this one
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_content_text() {
        let content = MessageContent::text("Hello");
        assert_eq!(content.as_text(), Some("Hello"));
        assert_eq!(content.to_string_content(), "Hello");
    }

    #[test]
    fn test_result_subtype_serialization() {
        let subtype = ResultSubtype::Success;
        let json = serde_json::to_string(&subtype).unwrap();
        assert_eq!(json, "\"success\"");

        let subtype = ResultSubtype::ErrorMaxTurns;
        let json = serde_json::to_string(&subtype).unwrap();
        assert_eq!(json, "\"error_max_turns\"");
    }

    #[test]
    fn test_usage_total() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        };
        assert_eq!(usage.total_tokens(), 150);
    }

    #[test]
    fn test_user_message_serialization() {
        let msg = UserMessage {
            content: MessageContent::text("Hello"),
            uuid: Some(Uuid::new_v4()),
            parent_tool_use_id: None,
            tool_use_result: None,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("Hello"));
    }
}
