//! Session checkpoint system
//!
//! Provides state snapshots for session recovery and rollback.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::error::{Error, Result};

/// Checkpoint type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum CheckpointType {
    /// Manually created by user
    Manual,
    /// Automatically created on interval
    Auto,
    /// Created before a destructive action
    PreAction,
    /// Created during error recovery
    Recovery,
}

impl std::fmt::Display for CheckpointType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckpointType::Manual => write!(f, "manual"),
            CheckpointType::Auto => write!(f, "auto"),
            CheckpointType::PreAction => write!(f, "pre_action"),
            CheckpointType::Recovery => write!(f, "recovery"),
        }
    }
}

/// Conversation state snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSnapshot {
    /// Message history
    pub messages: Vec<MessageSnapshot>,
    /// Current context/system prompt
    pub context: Option<String>,
    /// Active tools
    pub active_tools: Vec<String>,
    /// Custom metadata
    pub metadata: serde_json::Value,
}

/// Message snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSnapshot {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub tool_calls: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

/// Checkpoint record
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: Uuid,
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub checkpoint_name: Option<String>,
    pub checkpoint_type: CheckpointType,
    pub conversation_state: serde_json::Value,
    pub workspace_snapshot_path: Option<String>,
    pub workspace_file_count: Option<i32>,
    pub workspace_size_bytes: Option<i64>,
    pub container_id: Option<Uuid>,
    pub container_status: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Request to create a checkpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCheckpointRequest {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub name: Option<String>,
    pub checkpoint_type: CheckpointType,
    pub conversation_state: ConversationSnapshot,
    pub include_workspace: bool,
}

/// Checkpoint manager
#[derive(Clone)]
pub struct CheckpointManager {
    db: PgPool,
    max_checkpoints_per_session: usize,
}

impl CheckpointManager {
    /// Create a new checkpoint manager
    pub fn new(db: PgPool) -> Self {
        Self {
            db,
            max_checkpoints_per_session: 10,
        }
    }

    /// Create a new checkpoint manager with custom limits
    pub fn with_limits(db: PgPool, max_checkpoints_per_session: usize) -> Self {
        Self {
            db,
            max_checkpoints_per_session,
        }
    }

    /// Create a new checkpoint
    pub async fn create_checkpoint(&self, request: CreateCheckpointRequest) -> Result<Checkpoint> {
        // Clean up old checkpoints if over limit
        self.cleanup_old_checkpoints(request.session_id).await?;

        // Create the checkpoint
        let conversation_state = serde_json::to_value(&request.conversation_state)
            .map_err(|e| Error::Internal(e.to_string()))?;

        let checkpoint = sqlx::query_as::<_, Checkpoint>(
            r#"
            INSERT INTO session_checkpoints (
                session_id, user_id, checkpoint_name, checkpoint_type,
                conversation_state
            )
            VALUES ($1, $2, $3, $4, $5)
            RETURNING *
            "#,
        )
        .bind(request.session_id)
        .bind(request.user_id)
        .bind(&request.name)
        .bind(request.checkpoint_type)
        .bind(&conversation_state)
        .fetch_one(&self.db)
        .await?;

        tracing::info!(
            checkpoint_id = %checkpoint.id,
            session_id = %request.session_id,
            checkpoint_type = %request.checkpoint_type,
            "Checkpoint created"
        );

        Ok(checkpoint)
    }

    /// Get a checkpoint by ID
    pub async fn get_checkpoint(&self, checkpoint_id: Uuid) -> Result<Option<Checkpoint>> {
        Ok(
            sqlx::query_as::<_, Checkpoint>("SELECT * FROM session_checkpoints WHERE id = $1")
                .bind(checkpoint_id)
                .fetch_optional(&self.db)
                .await?,
        )
    }

    /// List checkpoints for a session
    pub async fn list_checkpoints(&self, session_id: Uuid) -> Result<Vec<Checkpoint>> {
        Ok(sqlx::query_as::<_, Checkpoint>(
            r#"
            SELECT * FROM session_checkpoints
            WHERE session_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.db)
        .await?)
    }

    /// Get the latest checkpoint for a session
    pub async fn get_latest_checkpoint(&self, session_id: Uuid) -> Result<Option<Checkpoint>> {
        Ok(sqlx::query_as::<_, Checkpoint>(
            r#"
            SELECT * FROM session_checkpoints
            WHERE session_id = $1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.db)
        .await?)
    }

    /// Delete a checkpoint
    pub async fn delete_checkpoint(&self, checkpoint_id: Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM session_checkpoints WHERE id = $1")
            .bind(checkpoint_id)
            .execute(&self.db)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Clean up old checkpoints beyond the limit
    async fn cleanup_old_checkpoints(&self, session_id: Uuid) -> Result<()> {
        // Get count of existing checkpoints
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM session_checkpoints WHERE session_id = $1")
                .bind(session_id)
                .fetch_one(&self.db)
                .await?;

        // Delete oldest if over limit
        if count.0 >= self.max_checkpoints_per_session as i64 {
            let to_delete = count.0 - self.max_checkpoints_per_session as i64 + 1;

            sqlx::query(
                r#"
                DELETE FROM session_checkpoints
                WHERE id IN (
                    SELECT id FROM session_checkpoints
                    WHERE session_id = $1
                    ORDER BY created_at ASC
                    LIMIT $2
                )
                "#,
            )
            .bind(session_id)
            .bind(to_delete)
            .execute(&self.db)
            .await?;

            tracing::debug!(
                session_id = %session_id,
                deleted = to_delete,
                "Cleaned up old checkpoints"
            );
        }

        Ok(())
    }

    /// Restore a checkpoint (returns the conversation state)
    pub async fn restore_checkpoint(&self, checkpoint_id: Uuid) -> Result<ConversationSnapshot> {
        let checkpoint = self
            .get_checkpoint(checkpoint_id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("Checkpoint not found: {}", checkpoint_id)))?;

        let snapshot: ConversationSnapshot = serde_json::from_value(checkpoint.conversation_state)
            .map_err(|e| Error::Internal(format!("Failed to parse checkpoint state: {}", e)))?;

        tracing::info!(
            checkpoint_id = %checkpoint_id,
            session_id = %checkpoint.session_id,
            "Checkpoint restored"
        );

        Ok(snapshot)
    }

    /// Create an auto-checkpoint (called on interval)
    pub async fn create_auto_checkpoint(
        &self,
        session_id: Uuid,
        user_id: Uuid,
        conversation_state: ConversationSnapshot,
    ) -> Result<Checkpoint> {
        self.create_checkpoint(CreateCheckpointRequest {
            session_id,
            user_id,
            name: Some(format!("Auto-save at {}", Utc::now().format("%H:%M"))),
            checkpoint_type: CheckpointType::Auto,
            conversation_state,
            include_workspace: false,
        })
        .await
    }

    /// Create a pre-action checkpoint (before destructive operations)
    pub async fn create_pre_action_checkpoint(
        &self,
        session_id: Uuid,
        user_id: Uuid,
        action_name: &str,
        conversation_state: ConversationSnapshot,
    ) -> Result<Checkpoint> {
        self.create_checkpoint(CreateCheckpointRequest {
            session_id,
            user_id,
            name: Some(format!("Before: {}", action_name)),
            checkpoint_type: CheckpointType::PreAction,
            conversation_state,
            include_workspace: true,
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_type_display() {
        assert_eq!(CheckpointType::Manual.to_string(), "manual");
        assert_eq!(CheckpointType::Auto.to_string(), "auto");
        assert_eq!(CheckpointType::PreAction.to_string(), "pre_action");
    }

    #[test]
    fn test_conversation_snapshot_serde() {
        let snapshot = ConversationSnapshot {
            messages: vec![MessageSnapshot {
                id: Uuid::new_v4(),
                role: "user".to_string(),
                content: "Hello".to_string(),
                tool_calls: None,
                created_at: Utc::now(),
            }],
            context: Some("System prompt".to_string()),
            active_tools: vec!["read_file".to_string()],
            metadata: serde_json::json!({}),
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        let restored: ConversationSnapshot = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.messages.len(), 1);
        assert_eq!(restored.messages[0].content, "Hello");
    }
}
