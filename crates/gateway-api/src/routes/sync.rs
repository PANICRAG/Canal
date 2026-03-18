//! Chat Sync Protocol — pull/push endpoints for offline-first conversation sync.
//!
//! Implements the dual-storage model from A28 Section 11:
//! - Server (PostgreSQL) is the source of truth
//! - Client (SQLite) maintains a local cache with offline queue
//! - Conflict resolution: server-wins (last-write-wins by server timestamp)

use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::middleware::auth::AuthContext;
use crate::state::AppState;

/// Register sync routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/pull", post(sync_pull))
        .route("/push", post(sync_push))
}

// ─── Pull ────────────────────────────────────────────────────────────────────

/// Request body for `POST /api/sync/pull`.
#[derive(Debug, Deserialize)]
pub struct SyncPullRequest {
    /// Client's last known sync timestamp. Server returns all changes after this.
    pub last_sync_timestamp: DateTime<Utc>,
    /// Optional: only sync specific conversation IDs (empty = all).
    #[serde(default)]
    pub conversation_ids: Vec<Uuid>,
}

/// Response for `POST /api/sync/pull`.
#[derive(Debug, Serialize)]
pub struct SyncPullResponse {
    /// Conversations created or updated since the timestamp.
    pub conversations: Vec<SyncConversation>,
    /// Messages created since the timestamp.
    pub messages: Vec<SyncMessage>,
    /// IDs deleted on the server since the timestamp.
    pub deleted: DeletedIds,
    /// Server timestamp for the client to store as `last_sync_timestamp`.
    pub server_timestamp: DateTime<Utc>,
}

/// Deleted entity IDs.
#[derive(Debug, Serialize, Default)]
pub struct DeletedIds {
    pub conversations: Vec<Uuid>,
    pub messages: Vec<Uuid>,
}

/// Conversation payload for sync.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyncConversation {
    pub id: Uuid,
    pub title: Option<String>,
    pub summary: Option<String>,
    pub model: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Message payload for sync.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyncMessage {
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

/// Pull changes since a given timestamp.
///
/// Returns all conversations and messages that were created or updated after
/// `last_sync_timestamp` for the authenticated user.
#[tracing::instrument(skip(state, auth), fields(user_id = %auth.user_id))]
async fn sync_pull(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(req): Json<SyncPullRequest>,
) -> Result<Json<SyncPullResponse>, StatusCode> {
    let since = req.last_sync_timestamp;

    // Fetch conversations updated since timestamp
    let conversations = sqlx::query_as::<_, ConversationRow>(
        r#"
        SELECT id, title, summary, metadata, created_at, updated_at
        FROM conversations
        WHERE user_id = $1 AND updated_at > $2
        ORDER BY updated_at ASC
        "#,
    )
    .bind(auth.user_id)
    .bind(since)
    .fetch_all(&state.db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to fetch conversations for sync");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Optionally filter to specific conversation IDs
    let conv_ids: Vec<Uuid> = if req.conversation_ids.is_empty() {
        conversations.iter().map(|c| c.id).collect()
    } else {
        req.conversation_ids.clone()
    };

    // Fetch new messages since timestamp for user's conversations
    let messages = if conv_ids.is_empty() {
        vec![]
    } else {
        sqlx::query_as::<_, MessageRow>(
            r#"
            SELECT m.id, m.conversation_id, m.role, m.content,
                   m.artifacts, m.tool_calls, m.tool_results,
                   m.tokens_used, m.model_used, m.created_at
            FROM messages m
            INNER JOIN conversations c ON m.conversation_id = c.id
            WHERE c.user_id = $1 AND m.created_at > $2
            ORDER BY m.created_at ASC
            "#,
        )
        .bind(auth.user_id)
        .bind(since)
        .fetch_all(&state.db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to fetch messages for sync");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
    };

    let now = Utc::now();

    let response = SyncPullResponse {
        conversations: conversations
            .into_iter()
            .map(|c| SyncConversation {
                id: c.id,
                title: c.title,
                summary: c.summary,
                model: None,
                metadata: c.metadata,
                created_at: c.created_at,
                updated_at: c.updated_at,
            })
            .collect(),
        messages: messages
            .into_iter()
            .map(|m| SyncMessage {
                id: m.id,
                conversation_id: m.conversation_id,
                role: m.role,
                content: m.content,
                artifacts: m.artifacts,
                tool_calls: m.tool_calls,
                tool_results: m.tool_results,
                tokens_used: m.tokens_used,
                model_used: m.model_used,
                created_at: m.created_at,
            })
            .collect(),
        deleted: DeletedIds::default(), // TODO: implement soft-delete tracking
        server_timestamp: now,
    };

    tracing::info!(
        conversations = response.conversations.len(),
        messages = response.messages.len(),
        "Sync pull completed"
    );

    Ok(Json(response))
}

// ─── Push ────────────────────────────────────────────────────────────────────

/// Request body for `POST /api/sync/push`.
#[derive(Debug, Deserialize)]
pub struct SyncPushRequest {
    /// Changes made locally while offline.
    pub changes: Vec<SyncChange>,
}

/// A single local change to push to the server.
#[derive(Debug, Deserialize)]
pub struct SyncChange {
    /// Entity type.
    #[serde(rename = "type")]
    pub entity_type: SyncEntityType,
    /// Action to perform.
    pub action: SyncAction,
    /// Entity ID.
    pub id: Uuid,
    /// Entity data (for create/update).
    pub data: Option<serde_json::Value>,
    /// Client-side timestamp when the change was made.
    pub client_timestamp: Option<DateTime<Utc>>,
}

/// Entity type for sync changes.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SyncEntityType {
    Conversation,
    Message,
}

/// Action type for sync changes.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SyncAction {
    Create,
    Update,
    Delete,
}

/// Response for `POST /api/sync/push`.
#[derive(Debug, Serialize)]
pub struct SyncPushResponse {
    /// Successfully applied changes.
    pub accepted: Vec<SyncChangeResult>,
    /// Changes that conflicted with server state.
    pub conflicts: Vec<SyncConflict>,
    /// Server timestamp after all changes applied.
    pub server_timestamp: DateTime<Utc>,
}

/// Result of an accepted change.
#[derive(Debug, Serialize)]
pub struct SyncChangeResult {
    pub id: Uuid,
    pub entity_type: SyncEntityType,
    pub action: SyncAction,
    /// Server-assigned ID if different from client ID.
    pub server_id: Option<Uuid>,
}

/// A conflict between client and server state.
#[derive(Debug, Serialize)]
pub struct SyncConflict {
    pub id: Uuid,
    pub entity_type: SyncEntityType,
    pub action: SyncAction,
    pub reason: String,
    /// Server's current version of the entity (for client to reconcile).
    pub server_data: Option<serde_json::Value>,
}

/// Push local changes to the server.
///
/// Conflict resolution: server-wins (last-write-wins). If the server entity
/// was updated after the client's change, the client change is rejected as a
/// conflict with the server's current data returned for reconciliation.
#[tracing::instrument(skip(state, auth), fields(user_id = %auth.user_id))]
async fn sync_push(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(req): Json<SyncPushRequest>,
) -> Result<Json<SyncPushResponse>, StatusCode> {
    // R4-M: Limit changes array size to prevent DoS
    const MAX_CHANGES: usize = 1000;
    if req.changes.len() > MAX_CHANGES {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut accepted = Vec::new();
    let mut conflicts = Vec::new();

    for change in &req.changes {
        match (&change.entity_type, &change.action) {
            // ── Conversation operations ──
            (SyncEntityType::Conversation, SyncAction::Create) => {
                match apply_create_conversation(&state.db, &auth, change).await {
                    Ok(result) => accepted.push(result),
                    Err(conflict) => conflicts.push(conflict),
                }
            }
            (SyncEntityType::Conversation, SyncAction::Update) => {
                match apply_update_conversation(&state.db, &auth, change).await {
                    Ok(result) => accepted.push(result),
                    Err(conflict) => conflicts.push(conflict),
                }
            }
            (SyncEntityType::Conversation, SyncAction::Delete) => {
                match apply_delete_conversation(&state.db, &auth, change).await {
                    Ok(result) => accepted.push(result),
                    Err(conflict) => conflicts.push(conflict),
                }
            }
            // ── Message operations ──
            (SyncEntityType::Message, SyncAction::Create) => {
                match apply_create_message(&state.db, &auth, change).await {
                    Ok(result) => accepted.push(result),
                    Err(conflict) => conflicts.push(conflict),
                }
            }
            (SyncEntityType::Message, SyncAction::Update) => {
                // Messages are append-only; update is a conflict
                conflicts.push(SyncConflict {
                    id: change.id,
                    entity_type: SyncEntityType::Message,
                    action: SyncAction::Update,
                    reason: "Messages are immutable; updates not supported".into(),
                    server_data: None,
                });
            }
            (SyncEntityType::Message, SyncAction::Delete) => {
                match apply_delete_message(&state.db, &auth, change).await {
                    Ok(result) => accepted.push(result),
                    Err(conflict) => conflicts.push(conflict),
                }
            }
        }
    }

    tracing::info!(
        accepted = accepted.len(),
        conflicts = conflicts.len(),
        "Sync push completed"
    );

    Ok(Json(SyncPushResponse {
        accepted,
        conflicts,
        server_timestamp: Utc::now(),
    }))
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Internal row type for conversation queries.
#[derive(Debug, sqlx::FromRow)]
struct ConversationRow {
    id: Uuid,
    title: Option<String>,
    summary: Option<String>,
    metadata: Option<serde_json::Value>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Internal row type for message queries.
#[derive(Debug, sqlx::FromRow)]
struct MessageRow {
    id: Uuid,
    conversation_id: Uuid,
    role: String,
    content: String,
    artifacts: Option<serde_json::Value>,
    tool_calls: Option<serde_json::Value>,
    tool_results: Option<serde_json::Value>,
    tokens_used: Option<i32>,
    model_used: Option<String>,
    created_at: DateTime<Utc>,
}

/// Apply a create-conversation change.
async fn apply_create_conversation(
    db: &sqlx::PgPool,
    auth: &AuthContext,
    change: &SyncChange,
) -> Result<SyncChangeResult, SyncConflict> {
    // Check if conversation already exists
    let existing: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM conversations WHERE id = $1")
        .bind(change.id)
        .fetch_optional(db)
        .await
        .map_err(|e| SyncConflict {
            id: change.id,
            entity_type: SyncEntityType::Conversation,
            action: SyncAction::Create,
            reason: format!("Database error: {e}"),
            server_data: None,
        })?;

    if existing.is_some() {
        return Err(SyncConflict {
            id: change.id,
            entity_type: SyncEntityType::Conversation,
            action: SyncAction::Create,
            reason: "Conversation already exists on server".into(),
            server_data: None,
        });
    }

    let title = change
        .data
        .as_ref()
        .and_then(|d| d.get("title"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let metadata = change
        .data
        .as_ref()
        .and_then(|d| d.get("metadata"))
        .cloned();

    sqlx::query(
        r#"
        INSERT INTO conversations (id, user_id, title, metadata, created_at, updated_at)
        VALUES ($1, $2, $3, $4, NOW(), NOW())
        "#,
    )
    .bind(change.id)
    .bind(auth.user_id)
    .bind(&title)
    .bind(&metadata)
    .execute(db)
    .await
    .map_err(|e| SyncConflict {
        id: change.id,
        entity_type: SyncEntityType::Conversation,
        action: SyncAction::Create,
        reason: format!("Failed to create: {e}"),
        server_data: None,
    })?;

    Ok(SyncChangeResult {
        id: change.id,
        entity_type: SyncEntityType::Conversation,
        action: SyncAction::Create,
        server_id: None,
    })
}

/// Apply an update-conversation change (server-wins conflict resolution).
async fn apply_update_conversation(
    db: &sqlx::PgPool,
    auth: &AuthContext,
    change: &SyncChange,
) -> Result<SyncChangeResult, SyncConflict> {
    // Fetch server version
    let server: Option<ConversationRow> = sqlx::query_as(
        "SELECT id, title, summary, metadata, created_at, updated_at FROM conversations WHERE id = $1 AND user_id = $2",
    )
    .bind(change.id)
    .bind(auth.user_id)
    .fetch_optional(db)
    .await
    .map_err(|e| SyncConflict {
        id: change.id,
        entity_type: SyncEntityType::Conversation,
        action: SyncAction::Update,
        reason: format!("Database error: {e}"),
        server_data: None,
    })?;

    let server = server.ok_or_else(|| SyncConflict {
        id: change.id,
        entity_type: SyncEntityType::Conversation,
        action: SyncAction::Update,
        reason: "Conversation not found or not owned by user".into(),
        server_data: None,
    })?;

    // Server-wins: if server was updated after client's timestamp, reject
    if let Some(client_ts) = change.client_timestamp {
        if server.updated_at > client_ts {
            return Err(SyncConflict {
                id: change.id,
                entity_type: SyncEntityType::Conversation,
                action: SyncAction::Update,
                reason: "Server version is newer".into(),
                server_data: Some(serde_json::json!({
                    "title": server.title,
                    "summary": server.summary,
                    "updated_at": server.updated_at,
                })),
            });
        }
    }

    let title = change
        .data
        .as_ref()
        .and_then(|d| d.get("title"))
        .and_then(|v| v.as_str());

    let summary = change
        .data
        .as_ref()
        .and_then(|d| d.get("summary"))
        .and_then(|v| v.as_str());

    sqlx::query(
        r#"
        UPDATE conversations
        SET title = COALESCE($3, title),
            summary = COALESCE($4, summary),
            updated_at = NOW()
        WHERE id = $1 AND user_id = $2
        "#,
    )
    .bind(change.id)
    .bind(auth.user_id)
    .bind(title)
    .bind(summary)
    .execute(db)
    .await
    .map_err(|e| SyncConflict {
        id: change.id,
        entity_type: SyncEntityType::Conversation,
        action: SyncAction::Update,
        reason: format!("Failed to update: {e}"),
        server_data: None,
    })?;

    Ok(SyncChangeResult {
        id: change.id,
        entity_type: SyncEntityType::Conversation,
        action: SyncAction::Update,
        server_id: None,
    })
}

/// Apply a delete-conversation change.
async fn apply_delete_conversation(
    db: &sqlx::PgPool,
    auth: &AuthContext,
    change: &SyncChange,
) -> Result<SyncChangeResult, SyncConflict> {
    let result = sqlx::query("DELETE FROM conversations WHERE id = $1 AND user_id = $2")
        .bind(change.id)
        .bind(auth.user_id)
        .execute(db)
        .await
        .map_err(|e| SyncConflict {
            id: change.id,
            entity_type: SyncEntityType::Conversation,
            action: SyncAction::Delete,
            reason: format!("Database error: {e}"),
            server_data: None,
        })?;

    if result.rows_affected() == 0 {
        return Err(SyncConflict {
            id: change.id,
            entity_type: SyncEntityType::Conversation,
            action: SyncAction::Delete,
            reason: "Conversation not found or not owned by user".into(),
            server_data: None,
        });
    }

    Ok(SyncChangeResult {
        id: change.id,
        entity_type: SyncEntityType::Conversation,
        action: SyncAction::Delete,
        server_id: None,
    })
}

/// Apply a create-message change.
async fn apply_create_message(
    db: &sqlx::PgPool,
    auth: &AuthContext,
    change: &SyncChange,
) -> Result<SyncChangeResult, SyncConflict> {
    let data = change.data.as_ref().ok_or_else(|| SyncConflict {
        id: change.id,
        entity_type: SyncEntityType::Message,
        action: SyncAction::Create,
        reason: "Missing message data".into(),
        server_data: None,
    })?;

    let conversation_id = data
        .get("conversation_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| SyncConflict {
            id: change.id,
            entity_type: SyncEntityType::Message,
            action: SyncAction::Create,
            reason: "Missing or invalid conversation_id".into(),
            server_data: None,
        })?;

    // Verify user owns the conversation
    let owns: (bool,) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM conversations WHERE id = $1 AND user_id = $2)")
            .bind(conversation_id)
            .bind(auth.user_id)
            .fetch_one(db)
            .await
            .map_err(|e| SyncConflict {
                id: change.id,
                entity_type: SyncEntityType::Message,
                action: SyncAction::Create,
                reason: format!("Database error: {e}"),
                server_data: None,
            })?;

    if !owns.0 {
        return Err(SyncConflict {
            id: change.id,
            entity_type: SyncEntityType::Message,
            action: SyncAction::Create,
            reason: "Conversation not found or not owned by user".into(),
            server_data: None,
        });
    }

    // Check for duplicate message ID
    let existing: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM messages WHERE id = $1")
        .bind(change.id)
        .fetch_optional(db)
        .await
        .map_err(|e| SyncConflict {
            id: change.id,
            entity_type: SyncEntityType::Message,
            action: SyncAction::Create,
            reason: format!("Database error: {e}"),
            server_data: None,
        })?;

    if existing.is_some() {
        // Idempotent: treat duplicate as accepted
        return Ok(SyncChangeResult {
            id: change.id,
            entity_type: SyncEntityType::Message,
            action: SyncAction::Create,
            server_id: None,
        });
    }

    let role = data.get("role").and_then(|v| v.as_str()).unwrap_or("user");
    let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let artifacts = data.get("artifacts").cloned();
    let tool_calls = data.get("tool_calls").cloned();
    let tool_results = data.get("tool_results").cloned();
    let tokens_used = data
        .get("tokens_used")
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);
    let model_used = data
        .get("model_used")
        .and_then(|v| v.as_str())
        .map(String::from);

    sqlx::query(
        r#"
        INSERT INTO messages (id, conversation_id, role, content, artifacts, tool_calls, tool_results, tokens_used, model_used)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(change.id)
    .bind(conversation_id)
    .bind(role)
    .bind(content)
    .bind(&artifacts)
    .bind(&tool_calls)
    .bind(&tool_results)
    .bind(tokens_used)
    .bind(&model_used)
    .execute(db)
    .await
    .map_err(|e| SyncConflict {
        id: change.id,
        entity_type: SyncEntityType::Message,
        action: SyncAction::Create,
        reason: format!("Failed to create: {e}"),
        server_data: None,
    })?;

    // Touch conversation updated_at
    let _ = sqlx::query("UPDATE conversations SET updated_at = NOW() WHERE id = $1")
        .bind(conversation_id)
        .execute(db)
        .await;

    Ok(SyncChangeResult {
        id: change.id,
        entity_type: SyncEntityType::Message,
        action: SyncAction::Create,
        server_id: None,
    })
}

/// Apply a delete-message change.
async fn apply_delete_message(
    db: &sqlx::PgPool,
    auth: &AuthContext,
    change: &SyncChange,
) -> Result<SyncChangeResult, SyncConflict> {
    // Verify ownership via conversation join
    let result = sqlx::query(
        r#"
        DELETE FROM messages
        WHERE id = $1 AND conversation_id IN (
            SELECT id FROM conversations WHERE user_id = $2
        )
        "#,
    )
    .bind(change.id)
    .bind(auth.user_id)
    .execute(db)
    .await
    .map_err(|e| SyncConflict {
        id: change.id,
        entity_type: SyncEntityType::Message,
        action: SyncAction::Delete,
        reason: format!("Database error: {e}"),
        server_data: None,
    })?;

    if result.rows_affected() == 0 {
        // Idempotent: treat already-deleted as accepted
        return Ok(SyncChangeResult {
            id: change.id,
            entity_type: SyncEntityType::Message,
            action: SyncAction::Delete,
            server_id: None,
        });
    }

    Ok(SyncChangeResult {
        id: change.id,
        entity_type: SyncEntityType::Message,
        action: SyncAction::Delete,
        server_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_pull_request_deserialize() {
        let json = r#"{
            "last_sync_timestamp": "2026-02-10T12:00:00Z",
            "conversation_ids": []
        }"#;
        let req: SyncPullRequest = serde_json::from_str(json).unwrap();
        assert!(req.conversation_ids.is_empty());
    }

    #[test]
    fn test_sync_push_request_deserialize() {
        let json = r#"{
            "changes": [
                {
                    "type": "conversation",
                    "action": "create",
                    "id": "550e8400-e29b-41d4-a716-446655440000",
                    "data": {"title": "My Chat"},
                    "client_timestamp": "2026-02-10T12:00:00Z"
                },
                {
                    "type": "message",
                    "action": "create",
                    "id": "660e8400-e29b-41d4-a716-446655440000",
                    "data": {
                        "conversation_id": "550e8400-e29b-41d4-a716-446655440000",
                        "role": "user",
                        "content": "Hello"
                    }
                }
            ]
        }"#;
        let req: SyncPushRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.changes.len(), 2);
        assert_eq!(req.changes[0].entity_type, SyncEntityType::Conversation);
        assert_eq!(req.changes[1].entity_type, SyncEntityType::Message);
    }

    #[test]
    fn test_sync_entity_types() {
        let conv: SyncEntityType = serde_json::from_str(r#""conversation""#).unwrap();
        assert_eq!(conv, SyncEntityType::Conversation);
        let msg: SyncEntityType = serde_json::from_str(r#""message""#).unwrap();
        assert_eq!(msg, SyncEntityType::Message);
    }

    #[test]
    fn test_sync_actions() {
        let create: SyncAction = serde_json::from_str(r#""create""#).unwrap();
        assert_eq!(create, SyncAction::Create);
        let update: SyncAction = serde_json::from_str(r#""update""#).unwrap();
        assert_eq!(update, SyncAction::Update);
        let delete: SyncAction = serde_json::from_str(r#""delete""#).unwrap();
        assert_eq!(delete, SyncAction::Delete);
    }

    #[test]
    fn test_sync_pull_response_serialize() {
        let resp = SyncPullResponse {
            conversations: vec![],
            messages: vec![],
            deleted: DeletedIds::default(),
            server_timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("server_timestamp"));
        assert!(json.contains("conversations"));
    }

    #[test]
    fn test_conflict_serialize() {
        let conflict = SyncConflict {
            id: Uuid::new_v4(),
            entity_type: SyncEntityType::Conversation,
            action: SyncAction::Update,
            reason: "Server version is newer".into(),
            server_data: Some(serde_json::json!({"title": "Server Title"})),
        };
        let json = serde_json::to_string(&conflict).unwrap();
        assert!(json.contains("Server version is newer"));
        assert!(json.contains("Server Title"));
    }
}
