//! Content Block Types - Claude Agent SDK Compatible
//!
//! Defines the content block types used in messages.

use serde::{Deserialize, Serialize};

/// Content block types for messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Text content
    #[serde(rename = "text")]
    Text { text: String },

    /// Thinking content (extended thinking)
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },

    /// Tool use request from the model
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    /// Tool result from execution
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<ToolResultContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },

    /// Image content
    #[serde(rename = "image")]
    Image {
        source: ImageSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        alt_text: Option<String>,
    },

    /// Document content
    #[serde(rename = "document")]
    Document {
        source: DocumentSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

impl ContentBlock {
    /// Create a text content block
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text { text: s.into() }
    }

    /// Create a thinking content block
    pub fn thinking(s: impl Into<String>) -> Self {
        Self::Thinking {
            thinking: s.into(),
            signature: None,
        }
    }

    /// Create a tool use content block
    pub fn tool_use(
        id: impl Into<String>,
        name: impl Into<String>,
        input: serde_json::Value,
    ) -> Self {
        Self::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }
    }

    /// Create a tool result content block
    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: Some(ToolResultContent::Text(content.into())),
            is_error: Some(is_error),
        }
    }

    /// Get text content if this is a text block
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Check if this is a tool use block
    pub fn is_tool_use(&self) -> bool {
        matches!(self, Self::ToolUse { .. })
    }

    /// Check if this is a tool result block
    pub fn is_tool_result(&self) -> bool {
        matches!(self, Self::ToolResult { .. })
    }

    /// Get tool use info if this is a tool use block
    pub fn as_tool_use(&self) -> Option<(&str, &str, &serde_json::Value)> {
        match self {
            Self::ToolUse { id, name, input } => Some((id, name, input)),
            _ => None,
        }
    }
}

/// Tool result content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    /// Plain text result
    Text(String),
    /// Array of content blocks
    Blocks(Vec<ToolResultBlock>),
}

impl ToolResultContent {
    /// Get as text
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::Blocks(_) => None,
        }
    }

    /// Convert to string
    pub fn to_string_content(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ToolResultBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

/// Tool result block types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolResultBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "image")]
    Image { source: ImageSource },
}

/// Image source
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ImageSource {
    #[serde(rename = "base64")]
    Base64 { media_type: String, data: String },

    #[serde(rename = "url")]
    Url { url: String },
}

/// Document source
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DocumentSource {
    #[serde(rename = "base64")]
    Base64 { media_type: String, data: String },

    #[serde(rename = "url")]
    Url { url: String },

    #[serde(rename = "text")]
    Text { text: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_block_text() {
        let block = ContentBlock::text("Hello");
        assert_eq!(block.as_text(), Some("Hello"));
        assert!(!block.is_tool_use());
    }

    #[test]
    fn test_content_block_tool_use() {
        let block = ContentBlock::tool_use(
            "id1",
            "filesystem_read_file",
            serde_json::json!({"path": "/tmp"}),
        );
        assert!(block.is_tool_use());

        let (id, name, _) = block.as_tool_use().unwrap();
        assert_eq!(id, "id1");
        assert_eq!(name, "filesystem_read_file");
    }

    #[test]
    fn test_content_block_serialization() {
        let block = ContentBlock::text("Hello");
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"Hello\""));
    }

    #[test]
    fn test_tool_result_content() {
        let content = ToolResultContent::Text("Success".to_string());
        assert_eq!(content.as_text(), Some("Success"));
        assert_eq!(content.to_string_content(), "Success");
    }
}
