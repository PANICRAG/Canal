//! Session management API endpoints

use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::ApiError, middleware::auth::AuthContext, state::AppState};

/// Create the session routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_sessions))
        .route("/{session_id}", get(get_session))
        .route("/{session_id}/pause", post(pause_session))
        .route("/{session_id}/resume", post(resume_session))
        .route("/{session_id}", delete(terminate_session))
        .route("/{session_id}/checkpoints", get(list_checkpoints))
        .route("/{session_id}/checkpoints", post(create_checkpoint))
        .route(
            "/{session_id}/checkpoints/{checkpoint_id}",
            get(get_checkpoint),
        )
        .route(
            "/{session_id}/checkpoints/{checkpoint_id}/restore",
            post(restore_checkpoint),
        )
        .route(
            "/{session_id}/checkpoints/{checkpoint_id}",
            delete(delete_checkpoint),
        )
        .route("/{session_id}/files", get(list_file_changes))
}

// ============ Response Types ============

/// Session info response
#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub id: Uuid,
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub status: String,
    pub container_id: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
    pub last_message_at: Option<String>,
    pub idle_minutes: i64,
    pub expires_at: Option<String>,
}

/// Sessions list response
#[derive(Debug, Serialize)]
pub struct SessionsListResponse {
    pub sessions: Vec<SessionResponse>,
    pub count: usize,
}

/// Checkpoint response
#[derive(Debug, Serialize)]
pub struct CheckpointResponse {
    pub id: Uuid,
    pub session_id: Uuid,
    pub name: Option<String>,
    pub checkpoint_type: String,
    pub created_at: String,
    pub workspace_file_count: Option<i32>,
}

/// Checkpoints list response
#[derive(Debug, Serialize)]
pub struct CheckpointsListResponse {
    pub checkpoints: Vec<CheckpointResponse>,
    pub count: usize,
}

/// File change response
#[derive(Debug, Serialize)]
pub struct FileChangeResponse {
    pub id: Uuid,
    pub file_path: String,
    pub change_type: String,
    pub file_size: i64,
    pub created_at: String,
}

/// File changes list response
#[derive(Debug, Serialize)]
pub struct FileChangesListResponse {
    pub changes: Vec<FileChangeResponse>,
    pub count: usize,
}

// ============ Request Types ============

/// Browser mode for session
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BrowserMode {
    #[default]
    Native,
    Cloud,
}

/// Create session request
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    /// Browser mode for the session
    #[serde(default)]
    pub browser_mode: BrowserMode,
    /// Cloud endpoint URL (required when browser_mode is Cloud)
    pub cloud_endpoint: Option<String>,
}

/// Create checkpoint request
#[derive(Debug, Deserialize)]
pub struct CreateCheckpointRequest {
    pub name: Option<String>,
    #[serde(default)]
    pub include_workspace: bool,
}

// ============ Handlers ============

/// List all sessions for the current user
pub async fn list_sessions(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<SessionsListResponse>, ApiError> {
    let user_id = auth.user_id;

    let sessions = state
        .session_repository
        .get_user_sessions(user_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let session_responses: Vec<SessionResponse> = sessions
        .into_iter()
        .map(|s| SessionResponse {
            id: s.id,
            session_id: s.session_id,
            user_id: s.user_id,
            status: s.status.to_string(),
            container_id: s.container_id,
            created_at: s.created_at.to_rfc3339(),
            updated_at: s.updated_at.to_rfc3339(),
            last_message_at: s.last_message_at.map(|t| t.to_rfc3339()),
            idle_minutes: s.idle_minutes(),
            expires_at: s.expires_at.map(|t| t.to_rfc3339()),
        })
        .collect();

    let count = session_responses.len();

    Ok(Json(SessionsListResponse {
        sessions: session_responses,
        count,
    }))
}

/// Get a specific session
pub async fn get_session(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<SessionResponse>, ApiError> {
    let session = state
        .session_repository
        .get_state(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("Session not found: {}", session_id)))?;

    // Ownership check: only session owner or admin can access
    if session.user_id != auth.user_id && !auth.is_admin() {
        return Err(ApiError::not_found("Session not found"));
    }

    Ok(Json(SessionResponse {
        id: session.id,
        session_id: session.session_id,
        user_id: session.user_id,
        status: session.status.to_string(),
        container_id: session.container_id,
        created_at: session.created_at.to_rfc3339(),
        updated_at: session.updated_at.to_rfc3339(),
        last_message_at: session.last_message_at.map(|t| t.to_rfc3339()),
        idle_minutes: session.idle_minutes(),
        expires_at: session.expires_at.map(|t| t.to_rfc3339()),
    }))
}

/// Pause a session
pub async fn pause_session(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<SessionResponse>, ApiError> {
    use gateway_core::session::SessionStatus;

    // Ownership check before mutation
    let existing = state
        .session_repository
        .get_state(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    if existing.user_id != auth.user_id && !auth.is_admin() {
        return Err(ApiError::not_found("Session not found"));
    }

    let session = state
        .session_repository
        .update_status(session_id, SessionStatus::Paused)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    tracing::info!(session_id = %session_id, "Session paused");

    Ok(Json(SessionResponse {
        id: session.id,
        session_id: session.session_id,
        user_id: session.user_id,
        status: session.status.to_string(),
        container_id: session.container_id,
        created_at: session.created_at.to_rfc3339(),
        updated_at: session.updated_at.to_rfc3339(),
        last_message_at: session.last_message_at.map(|t| t.to_rfc3339()),
        idle_minutes: session.idle_minutes(),
        expires_at: session.expires_at.map(|t| t.to_rfc3339()),
    }))
}

/// Resume a paused session
pub async fn resume_session(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<SessionResponse>, ApiError> {
    // Ownership check before mutation
    let existing = state
        .session_repository
        .get_state(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    if existing.user_id != auth.user_id && !auth.is_admin() {
        return Err(ApiError::not_found("Session not found"));
    }

    let session = state
        .session_repository
        .resume_session(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    tracing::info!(session_id = %session_id, "Session resumed");

    Ok(Json(SessionResponse {
        id: session.id,
        session_id: session.session_id,
        user_id: session.user_id,
        status: session.status.to_string(),
        container_id: session.container_id,
        created_at: session.created_at.to_rfc3339(),
        updated_at: session.updated_at.to_rfc3339(),
        last_message_at: session.last_message_at.map(|t| t.to_rfc3339()),
        idle_minutes: session.idle_minutes(),
        expires_at: session.expires_at.map(|t| t.to_rfc3339()),
    }))
}

/// Terminate a session
pub async fn terminate_session(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Ownership check before deletion
    let existing = state
        .session_repository
        .get_state(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    if existing.user_id != auth.user_id && !auth.is_admin() {
        return Err(ApiError::not_found("Session not found"));
    }

    state
        .session_repository
        .terminate_session(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    tracing::info!(session_id = %session_id, "Session terminated");

    Ok(Json(serde_json::json!({
        "terminated": true,
        "session_id": session_id
    })))
}

/// List checkpoints for a session
pub async fn list_checkpoints(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<CheckpointsListResponse>, ApiError> {
    // Ownership check
    let session = state
        .session_repository
        .get_state(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    if session.user_id != auth.user_id && !auth.is_admin() {
        return Err(ApiError::not_found("Session not found"));
    }

    let checkpoints = state
        .checkpoint_manager
        .list_checkpoints(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let checkpoint_responses: Vec<CheckpointResponse> = checkpoints
        .into_iter()
        .map(|c| CheckpointResponse {
            id: c.id,
            session_id: c.session_id,
            name: c.checkpoint_name,
            checkpoint_type: c.checkpoint_type.to_string(),
            created_at: c.created_at.to_rfc3339(),
            workspace_file_count: c.workspace_file_count,
        })
        .collect();

    let count = checkpoint_responses.len();

    Ok(Json(CheckpointsListResponse {
        checkpoints: checkpoint_responses,
        count,
    }))
}

/// Create a checkpoint
pub async fn create_checkpoint(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(session_id): Path<Uuid>,
    Json(request): Json<CreateCheckpointRequest>,
) -> Result<Json<CheckpointResponse>, ApiError> {
    use gateway_core::session::checkpoint::{
        CheckpointType, ConversationSnapshot, CreateCheckpointRequest as CoreRequest,
    };

    let user_id = auth.user_id;

    let checkpoint = state
        .checkpoint_manager
        .create_checkpoint(CoreRequest {
            session_id,
            user_id,
            name: request.name,
            checkpoint_type: CheckpointType::Manual,
            conversation_state: ConversationSnapshot {
                messages: vec![],
                context: None,
                active_tools: vec![],
                metadata: serde_json::json!({}),
            },
            include_workspace: request.include_workspace,
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(CheckpointResponse {
        id: checkpoint.id,
        session_id: checkpoint.session_id,
        name: checkpoint.checkpoint_name,
        checkpoint_type: checkpoint.checkpoint_type.to_string(),
        created_at: checkpoint.created_at.to_rfc3339(),
        workspace_file_count: checkpoint.workspace_file_count,
    }))
}

/// Get a checkpoint
pub async fn get_checkpoint(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path((session_id, checkpoint_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<CheckpointResponse>, ApiError> {
    // Ownership check
    let session = state
        .session_repository
        .get_state(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    if session.user_id != auth.user_id && !auth.is_admin() {
        return Err(ApiError::not_found("Session not found"));
    }

    let checkpoint = state
        .checkpoint_manager
        .get_checkpoint(checkpoint_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("Checkpoint not found: {}", checkpoint_id)))?;

    if checkpoint.session_id != session_id {
        return Err(ApiError::not_found("Checkpoint not found for this session"));
    }

    Ok(Json(CheckpointResponse {
        id: checkpoint.id,
        session_id: checkpoint.session_id,
        name: checkpoint.checkpoint_name,
        checkpoint_type: checkpoint.checkpoint_type.to_string(),
        created_at: checkpoint.created_at.to_rfc3339(),
        workspace_file_count: checkpoint.workspace_file_count,
    }))
}

/// Restore a checkpoint
pub async fn restore_checkpoint(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path((session_id, checkpoint_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Ownership check
    let session = state
        .session_repository
        .get_state(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    if session.user_id != auth.user_id && !auth.is_admin() {
        return Err(ApiError::not_found("Session not found"));
    }

    let snapshot = state
        .checkpoint_manager
        .restore_checkpoint(checkpoint_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "restored": true,
        "checkpoint_id": checkpoint_id,
        "session_id": session_id,
        "message_count": snapshot.messages.len()
    })))
}

/// Delete a checkpoint
pub async fn delete_checkpoint(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path((session_id, checkpoint_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Ownership check
    let session = state
        .session_repository
        .get_state(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    if session.user_id != auth.user_id && !auth.is_admin() {
        return Err(ApiError::not_found("Session not found"));
    }

    let deleted = state
        .checkpoint_manager
        .delete_checkpoint(checkpoint_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "deleted": deleted,
        "checkpoint_id": checkpoint_id
    })))
}

/// List file changes for a session
pub async fn list_file_changes(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<FileChangesListResponse>, ApiError> {
    // Ownership check
    let session = state
        .session_repository
        .get_state(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    if session.user_id != auth.user_id && !auth.is_admin() {
        return Err(ApiError::not_found("Session not found"));
    }

    let changes = state
        .session_repository
        .get_file_changes(session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    let change_responses: Vec<FileChangeResponse> = changes
        .into_iter()
        .map(|c| FileChangeResponse {
            id: c.id,
            file_path: c.file_path,
            change_type: c.change_type,
            file_size: c.file_size,
            created_at: c.created_at.to_rfc3339(),
        })
        .collect();

    let count = change_responses.len();

    Ok(Json(FileChangesListResponse {
        changes: change_responses,
        count,
    }))
}
