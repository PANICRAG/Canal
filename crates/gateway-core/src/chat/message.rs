//! Chat message types

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Re-export ArtifactType from the canonical definition in artifact.rs
pub use super::artifact::ArtifactType;

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: Uuid,
    pub role: MessageRole,
    pub content: String,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    pub created_at: DateTime<Utc>,
}

/// Message role
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageRole::User => write!(f, "user"),
            MessageRole::Assistant => write!(f, "assistant"),
            MessageRole::System => write!(f, "system"),
        }
    }
}

/// Tool call information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub result: Option<serde_json::Value>,
}

/// Artifact - rich output that can be displayed, edited, or acted upon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub artifact_type: ArtifactType,
    pub title: String,
    pub data: serde_json::Value,
    #[serde(default)]
    pub actions: Vec<ArtifactAction>,
}

/// Action that can be performed on an artifact
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactAction {
    pub id: String,
    pub label: String,
    pub action_type: ActionType,
    pub params: serde_json::Value,
}

/// Action types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    /// Edit the artifact
    Edit,
    /// Confirm execution
    Confirm,
    /// Cancel operation
    Cancel,
    /// Export to file
    Export,
    /// Save artifact
    Save,
    /// Share artifact
    Share,
    /// Download artifact
    Download,
    /// Open in external app
    Open,
}

impl ChatMessage {
    /// Create a new user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            role: MessageRole::User,
            content: content.into(),
            artifacts: vec![],
            tool_calls: None,
            created_at: Utc::now(),
        }
    }

    /// Create a new assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            role: MessageRole::Assistant,
            content: content.into(),
            artifacts: vec![],
            tool_calls: None,
            created_at: Utc::now(),
        }
    }

    /// Create a new assistant message with artifacts
    pub fn assistant_with_artifacts(content: impl Into<String>, artifacts: Vec<Artifact>) -> Self {
        Self {
            id: Uuid::new_v4(),
            role: MessageRole::Assistant,
            content: content.into(),
            artifacts,
            tool_calls: None,
            created_at: Utc::now(),
        }
    }

    /// Create a new system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            role: MessageRole::System,
            content: content.into(),
            artifacts: vec![],
            tool_calls: None,
            created_at: Utc::now(),
        }
    }
}

impl Artifact {
    /// Create a new document artifact
    pub fn document(title: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            artifact_type: ArtifactType::Document,
            title: title.into(),
            data: serde_json::json!({ "content": content.into() }),
            actions: vec![
                ArtifactAction {
                    id: "edit".into(),
                    label: "Edit".into(),
                    action_type: ActionType::Edit,
                    params: serde_json::json!({}),
                },
                ArtifactAction {
                    id: "download".into(),
                    label: "Download".into(),
                    action_type: ActionType::Download,
                    params: serde_json::json!({}),
                },
            ],
        }
    }

    /// Create a confirmation artifact
    pub fn confirm(
        title: impl Into<String>,
        description: impl Into<String>,
        task_id: Uuid,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            artifact_type: ArtifactType::PublishConfirm,
            title: title.into(),
            data: serde_json::json!({
                "description": description.into(),
                "task_id": task_id,
            }),
            actions: vec![
                ArtifactAction {
                    id: "confirm".into(),
                    label: "Confirm".into(),
                    action_type: ActionType::Confirm,
                    params: serde_json::json!({ "task_id": task_id }),
                },
                ArtifactAction {
                    id: "cancel".into(),
                    label: "Cancel".into(),
                    action_type: ActionType::Cancel,
                    params: serde_json::json!({}),
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_user_message() {
        let msg = ChatMessage::user("Hello");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "Hello");
        assert!(msg.artifacts.is_empty());
    }

    #[test]
    fn test_create_assistant_message() {
        let msg = ChatMessage::assistant("Hi there!");
        assert_eq!(msg.role, MessageRole::Assistant);
        assert_eq!(msg.content, "Hi there!");
    }

    #[test]
    fn test_create_document_artifact() {
        let artifact = Artifact::document("Test Doc", "Content here");
        assert_eq!(artifact.artifact_type, ArtifactType::Document);
        assert_eq!(artifact.title, "Test Doc");
        assert_eq!(artifact.actions.len(), 2);
    }
}
