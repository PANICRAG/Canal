//! Artifacts endpoints

use axum::{
    extract::{Path, State},
    routing::{delete, get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::ApiError, state::AppState};

/// Create the artifacts routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_artifacts))
        .route("/{id}", get(get_artifact))
        .route("/{id}", delete(delete_artifact))
        .route("/{id}/approve", post(approve_artifact))
        .route("/{id}/reject", post(reject_artifact))
        .route("/{id}/content", patch(update_content))
        .route("/session/{session_id}", get(list_by_session))
        .route("/message/{message_id}", get(list_by_message))
}

/// Artifact summary for API
#[derive(Debug, Serialize)]
pub struct ApiArtifact {
    pub id: Uuid,
    pub session_id: String,
    pub message_id: String,
    pub artifact_type: String,
    pub title: String,
    pub content: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub actions: Vec<ApiArtifactAction>,
}

/// Artifact action for API
#[derive(Debug, Serialize)]
pub struct ApiArtifactAction {
    pub id: String,
    pub action_type: String,
    pub label: String,
    pub requires_confirmation: bool,
}

/// Artifacts list response
#[derive(Debug, Serialize)]
pub struct ArtifactsResponse {
    pub artifacts: Vec<ApiArtifact>,
    pub count: usize,
}

/// List all artifacts
pub async fn list_artifacts(State(state): State<AppState>) -> Json<ArtifactsResponse> {
    let store = state.artifact_store.read().await;

    let artifacts: Vec<ApiArtifact> = store
        .list_all()
        .await
        .into_iter()
        .map(|a| ApiArtifact {
            id: a.id,
            session_id: a.session_id.clone(),
            message_id: a.message_id.clone(),
            artifact_type: format!("{:?}", a.artifact_type),
            title: a.title.clone(),
            content: serde_json::to_value(&a.content).unwrap_or_default(),
            created_at: a.created_at,
            updated_at: a.updated_at,
            actions: a
                .actions
                .iter()
                .map(|act| ApiArtifactAction {
                    id: act.id.clone(),
                    action_type: format!("{:?}", act.action_type),
                    label: act.label.clone(),
                    requires_confirmation: act.requires_confirmation,
                })
                .collect(),
        })
        .collect();

    let count = artifacts.len();
    Json(ArtifactsResponse { artifacts, count })
}

/// Get a specific artifact
pub async fn get_artifact(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiArtifact>, ApiError> {
    let store = state.artifact_store.read().await;

    let artifact = store
        .get(id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("Artifact not found: {}", id)))?;

    Ok(Json(ApiArtifact {
        id: artifact.id,
        session_id: artifact.session_id.clone(),
        message_id: artifact.message_id.clone(),
        artifact_type: format!("{:?}", artifact.artifact_type),
        title: artifact.title.clone(),
        content: serde_json::to_value(&artifact.content).unwrap_or_default(),
        created_at: artifact.created_at,
        updated_at: artifact.updated_at,
        actions: artifact
            .actions
            .iter()
            .map(|act| ApiArtifactAction {
                id: act.id.clone(),
                action_type: format!("{:?}", act.action_type),
                label: act.label.clone(),
                requires_confirmation: act.requires_confirmation,
            })
            .collect(),
    }))
}

/// Delete an artifact
pub async fn delete_artifact(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store = state.artifact_store.read().await;

    let deleted = store
        .delete(id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    if deleted {
        Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
    } else {
        Err(ApiError::not_found(format!("Artifact not found: {}", id)))
    }
}

/// Approval request
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct ApprovalRequest {
    #[serde(default)]
    pub comment: Option<String>,
}

/// Approve an artifact (for approval request artifacts)
pub async fn approve_artifact(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(_request): Json<ApprovalRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store = state.artifact_store.read().await;

    store
        .update_approval(id, true)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    tracing::info!(artifact_id = %id, "Artifact approved");
    Ok(Json(serde_json::json!({ "approved": true, "id": id })))
}

/// Reject an artifact (for approval request artifacts)
pub async fn reject_artifact(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(_request): Json<ApprovalRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store = state.artifact_store.read().await;

    store
        .update_approval(id, false)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    tracing::info!(artifact_id = %id, "Artifact rejected");
    Ok(Json(serde_json::json!({ "rejected": true, "id": id })))
}

/// Update content request
#[derive(Debug, Deserialize)]
pub struct UpdateContentRequest {
    pub content: serde_json::Value,
}

/// Update artifact content
pub async fn update_content(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<UpdateContentRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let store = state.artifact_store.read().await;

    // Try to deserialize into ArtifactContent
    let content: gateway_core::artifacts::ArtifactContent = serde_json::from_value(request.content)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    store
        .update_content(id, content)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    tracing::info!(artifact_id = %id, "Artifact content updated");
    Ok(Json(serde_json::json!({ "updated": true, "id": id })))
}

/// List artifacts by session
pub async fn list_by_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<ArtifactsResponse>, ApiError> {
    let store = state.artifact_store.read().await;

    let artifacts: Vec<ApiArtifact> = store
        .list_by_session(&session_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .into_iter()
        .map(|a| ApiArtifact {
            id: a.id,
            session_id: a.session_id.clone(),
            message_id: a.message_id.clone(),
            artifact_type: format!("{:?}", a.artifact_type),
            title: a.title.clone(),
            content: serde_json::to_value(&a.content).unwrap_or_default(),
            created_at: a.created_at,
            updated_at: a.updated_at,
            actions: a
                .actions
                .iter()
                .map(|act| ApiArtifactAction {
                    id: act.id.clone(),
                    action_type: format!("{:?}", act.action_type),
                    label: act.label.clone(),
                    requires_confirmation: act.requires_confirmation,
                })
                .collect(),
        })
        .collect();

    let count = artifacts.len();
    Ok(Json(ArtifactsResponse { artifacts, count }))
}

/// List artifacts by message
pub async fn list_by_message(
    State(state): State<AppState>,
    Path(message_id): Path<String>,
) -> Result<Json<ArtifactsResponse>, ApiError> {
    let store = state.artifact_store.read().await;

    let artifacts: Vec<ApiArtifact> = store
        .list_by_message(&message_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .into_iter()
        .map(|a| ApiArtifact {
            id: a.id,
            session_id: a.session_id.clone(),
            message_id: a.message_id.clone(),
            artifact_type: format!("{:?}", a.artifact_type),
            title: a.title.clone(),
            content: serde_json::to_value(&a.content).unwrap_or_default(),
            created_at: a.created_at,
            updated_at: a.updated_at,
            actions: a
                .actions
                .iter()
                .map(|act| ApiArtifactAction {
                    id: act.id.clone(),
                    action_type: format!("{:?}", act.action_type),
                    label: act.label.clone(),
                    requires_confirmation: act.requires_confirmation,
                })
                .collect(),
        })
        .collect();

    let count = artifacts.len();
    Ok(Json(ArtifactsResponse { artifacts, count }))
}
