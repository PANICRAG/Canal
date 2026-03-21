//! Message and Conversation repository for persistence
//!
//! Provides database operations for storing and retrieving messages
//! and conversations to enable session persistence across app restarts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::error::Result;

/// Stored message record from database
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: String,
    pub content: String,
    pub artifacts: Option<serde_json::Value>,
    pub tool_calls: Option<serde_json::Value>,
    pub tool_results: Option<serde_json::Value>,
    pub tokens_used: Option<i32>,
    pub model_used: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Stored conversation record from database
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct StoredConversation {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub organization_id: Option<Uuid>,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input for creating a new message
#[derive(Debug, Clone)]
pub struct NewMessage {
    pub conversation_id: Uuid,
    pub role: String,
    pub content: String,
    pub artifacts: Option<serde_json::Value>,
    pub tool_calls: Option<serde_json::Value>,
    pub tool_results: Option<serde_json::Value>,
    pub tokens_used: Option<i32>,
    pub model_used: Option<String>,
}

/// Input for creating a new conversation
#[derive(Debug, Clone)]
pub struct NewConversation {
    pub id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub organization_id: Option<Uuid>,
    pub title: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Message repository for database operations
#[derive(Clone)]
pub struct MessageRepository {
    db: PgPool,
}

impl MessageRepository {
    /// Create a new message repository
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Save a message to the database
    pub async fn save_message(&self, message: NewMessage) -> Result<StoredMessage> {
        let result = sqlx::query_as::<_, StoredMessage>(
            r#"
            INSERT INTO messages (conversation_id, role, content, artifacts, tool_calls, tool_results, tokens_used, model_used)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING *
            "#,
        )
        .bind(message.conversation_id)
        .bind(&message.role)
        .bind(&message.content)
        .bind(&message.artifacts)
        .bind(&message.tool_calls)
        .bind(&message.tool_results)
        .bind(message.tokens_used)
        .bind(&message.model_used)
        .fetch_one(&self.db)
        .await?;

        tracing::debug!(
            message_id = %result.id,
            conversation_id = %result.conversation_id,
            role = %result.role,
            "Message saved to database"
        );

        Ok(result)
    }

    /// Get messages by conversation ID, ordered by creation time
    pub async fn get_messages_by_conversation(
        &self,
        conversation_id: Uuid,
    ) -> Result<Vec<StoredMessage>> {
        let messages = sqlx::query_as::<_, StoredMessage>(
            r#"
            SELECT * FROM messages
            WHERE conversation_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(conversation_id)
        .fetch_all(&self.db)
        .await?;

        tracing::debug!(
            conversation_id = %conversation_id,
            message_count = messages.len(),
            "Loaded messages from database"
        );

        Ok(messages)
    }

    /// Get messages by conversation ID with pagination
    pub async fn get_messages_by_conversation_paginated(
        &self,
        conversation_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<StoredMessage>> {
        Ok(sqlx::query_as::<_, StoredMessage>(
            r#"
            SELECT * FROM messages
            WHERE conversation_id = $1
            ORDER BY created_at ASC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(conversation_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db)
        .await?)
    }

    /// Get the most recent N messages for a conversation
    pub async fn get_recent_messages(
        &self,
        conversation_id: Uuid,
        limit: i64,
    ) -> Result<Vec<StoredMessage>> {
        // Get the most recent messages but return them in chronological order
        Ok(sqlx::query_as::<_, StoredMessage>(
            r#"
            SELECT * FROM (
                SELECT * FROM messages
                WHERE conversation_id = $1
                ORDER BY created_at DESC
                LIMIT $2
            ) sub
            ORDER BY created_at ASC
            "#,
        )
        .bind(conversation_id)
        .bind(limit)
        .fetch_all(&self.db)
        .await?)
    }

    /// Get a single message by ID
    pub async fn get_message(&self, message_id: Uuid) -> Result<Option<StoredMessage>> {
        Ok(
            sqlx::query_as::<_, StoredMessage>("SELECT * FROM messages WHERE id = $1")
                .bind(message_id)
                .fetch_optional(&self.db)
                .await?,
        )
    }

    /// Delete all messages for a conversation
    pub async fn delete_messages_by_conversation(&self, conversation_id: Uuid) -> Result<u64> {
        let result = sqlx::query("DELETE FROM messages WHERE conversation_id = $1")
            .bind(conversation_id)
            .execute(&self.db)
            .await?;

        Ok(result.rows_affected())
    }

    /// Get message count for a conversation
    pub async fn get_message_count(&self, conversation_id: Uuid) -> Result<i64> {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM messages WHERE conversation_id = $1")
                .bind(conversation_id)
                .fetch_one(&self.db)
                .await?;

        Ok(row.0)
    }

    /// Get messages by conversation ID with user ownership verification
    /// Returns empty vec if user doesn't own the conversation
    pub async fn get_messages_for_user(
        &self,
        conversation_id: Uuid,
        user_id: Uuid,
    ) -> Result<Vec<StoredMessage>> {
        // Join with conversations table to verify ownership
        let messages = sqlx::query_as::<_, StoredMessage>(
            r#"
            SELECT m.* FROM messages m
            INNER JOIN conversations c ON m.conversation_id = c.id
            WHERE m.conversation_id = $1 AND c.user_id = $2
            ORDER BY m.created_at ASC
            "#,
        )
        .bind(conversation_id)
        .bind(user_id)
        .fetch_all(&self.db)
        .await?;

        tracing::debug!(
            conversation_id = %conversation_id,
            user_id = %user_id,
            message_count = messages.len(),
            "Loaded messages for user from database"
        );

        Ok(messages)
    }
}

/// Conversation repository for database operations
#[derive(Clone)]
pub struct ConversationRepository {
    db: PgPool,
}

impl ConversationRepository {
    /// Create a new conversation repository
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Save a new conversation or update existing
    pub async fn save_conversation(&self, conv: NewConversation) -> Result<StoredConversation> {
        let id = conv.id.unwrap_or_else(Uuid::new_v4);

        let result = sqlx::query_as::<_, StoredConversation>(
            r#"
            INSERT INTO conversations (id, user_id, organization_id, title, metadata)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (id) DO UPDATE SET
                title = COALESCE(EXCLUDED.title, conversations.title),
                metadata = COALESCE(EXCLUDED.metadata, conversations.metadata),
                updated_at = NOW()
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(conv.user_id)
        .bind(conv.organization_id)
        .bind(&conv.title)
        .bind(&conv.metadata)
        .fetch_one(&self.db)
        .await?;

        tracing::debug!(
            conversation_id = %result.id,
            title = ?result.title,
            "Conversation saved to database"
        );

        Ok(result)
    }

    /// Get a conversation by ID
    pub async fn get_conversation(&self, id: Uuid) -> Result<Option<StoredConversation>> {
        Ok(
            sqlx::query_as::<_, StoredConversation>("SELECT * FROM conversations WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.db)
                .await?,
        )
    }

    /// Get a conversation by ID with user ownership verification
    pub async fn get_conversation_for_user(
        &self,
        id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<StoredConversation>> {
        Ok(sqlx::query_as::<_, StoredConversation>(
            "SELECT * FROM conversations WHERE id = $1 AND user_id = $2",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.db)
        .await?)
    }

    /// Check if a user owns a conversation
    pub async fn user_owns_conversation(
        &self,
        conversation_id: Uuid,
        user_id: Uuid,
    ) -> Result<bool> {
        let row: (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = $1 AND user_id = $2)",
        )
        .bind(conversation_id)
        .bind(user_id)
        .fetch_one(&self.db)
        .await?;

        Ok(row.0)
    }

    /// Get conversations for a user, ordered by most recent
    pub async fn get_user_conversations(
        &self,
        user_id: Uuid,
        limit: i64,
    ) -> Result<Vec<StoredConversation>> {
        Ok(sqlx::query_as::<_, StoredConversation>(
            r#"
            SELECT * FROM conversations
            WHERE user_id = $1
            ORDER BY updated_at DESC
            LIMIT $2
            "#,
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.db)
        .await?)
    }

    /// Get all conversations, ordered by most recent
    pub async fn list_conversations(&self, limit: i64) -> Result<Vec<StoredConversation>> {
        Ok(sqlx::query_as::<_, StoredConversation>(
            r#"
            SELECT * FROM conversations
            ORDER BY updated_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.db)
        .await?)
    }

    /// Update conversation title
    pub async fn update_title(&self, id: Uuid, title: &str) -> Result<StoredConversation> {
        Ok(sqlx::query_as::<_, StoredConversation>(
            r#"
            UPDATE conversations
            SET title = $2, updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(title)
        .fetch_one(&self.db)
        .await?)
    }

    /// Update conversation summary
    pub async fn update_summary(&self, id: Uuid, summary: &str) -> Result<StoredConversation> {
        Ok(sqlx::query_as::<_, StoredConversation>(
            r#"
            UPDATE conversations
            SET summary = $2, updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(summary)
        .fetch_one(&self.db)
        .await?)
    }

    /// Touch conversation to update the updated_at timestamp
    pub async fn touch(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE conversations
            SET updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    /// Delete a conversation (messages are cascade deleted)
    pub async fn delete_conversation(&self, id: Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM conversations WHERE id = $1")
            .bind(id)
            .execute(&self.db)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Check if a conversation exists
    pub async fn exists(&self, id: Uuid) -> Result<bool> {
        let row: (bool,) =
            sqlx::query_as("SELECT EXISTS(SELECT 1 FROM conversations WHERE id = $1)")
                .bind(id)
                .fetch_one(&self.db)
                .await?;

        Ok(row.0)
    }

    /// Ensure a conversation exists, creating it if necessary
    pub async fn ensure_exists(
        &self,
        id: Uuid,
        user_id: Option<Uuid>,
    ) -> Result<StoredConversation> {
        if let Some(conv) = self.get_conversation(id).await? {
            return Ok(conv);
        }

        self.save_conversation(NewConversation {
            id: Some(id),
            user_id,
            organization_id: None,
            title: None,
            metadata: None,
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_message() {
        let msg = NewMessage {
            conversation_id: Uuid::new_v4(),
            role: "user".to_string(),
            content: "Hello".to_string(),
            artifacts: None,
            tool_calls: None,
            tool_results: None,
            tokens_used: Some(10),
            model_used: Some("claude-3".to_string()),
        };

        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "Hello");
    }

    #[test]
    fn test_new_conversation() {
        let conv = NewConversation {
            id: None,
            user_id: Some(Uuid::new_v4()),
            organization_id: None,
            title: Some("Test Chat".to_string()),
            metadata: None,
        };

        assert!(conv.id.is_none());
        assert!(conv.user_id.is_some());
        assert_eq!(conv.title, Some("Test Chat".to_string()));
    }
}
