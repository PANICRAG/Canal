//! Container management API endpoints

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::{error::ApiError, middleware::auth::AuthContext, state::AppState};

/// Create the container routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_containers))
        .route("/{id}", get(get_container))
        .route("/{id}", delete(delete_container))
        .route("/{id}/pause", post(pause_container))
        .route("/{id}/resume", post(resume_container))
}

// ============ Response Types ============

/// Container information response
#[derive(Debug, Serialize)]
pub struct ContainerInfo {
    pub id: Uuid,
    pub session_id: Option<Uuid>,
    pub status: String,
    pub pod_name: String,
    pub grpc_endpoint: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
}

/// Container list response
#[derive(Debug, Serialize)]
pub struct ContainerListResponse {
    pub containers: Vec<ContainerInfo>,
    pub count: usize,
}

// ============ Handlers ============

/// List user's containers
/// GET /api/containers
pub async fn list_containers(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<ContainerListResponse>, ApiError> {
    let orchestrator = state.require_orchestrator()?;

    let user_id = auth.user_id;

    let containers = orchestrator
        .list_user_containers(user_id)
        .await
        .map_err(ApiError::from)?;

    let container_infos: Vec<ContainerInfo> = containers
        .into_iter()
        .map(|c| ContainerInfo {
            id: c.id,
            session_id: c.session_id,
            status: c.status.to_string(),
            pod_name: c.pod_name,
            grpc_endpoint: c.grpc_endpoint,
            created_at: c.created_at,
            last_activity: c.last_activity,
        })
        .collect();

    let count = container_infos.len();

    Ok(Json(ContainerListResponse {
        containers: container_infos,
        count,
    }))
}

/// Get container status
/// GET /api/containers/:id
pub async fn get_container(
    State(state): State<AppState>,
    Path(container_id): Path<Uuid>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<ContainerInfo>, ApiError> {
    let orchestrator = state.require_orchestrator()?;

    let container = orchestrator
        .get_container(&container_id)
        .await
        .map_err(ApiError::from)?;

    // R8-C5: Verify ownership — only container owner or admin can access
    if container.user_id != auth.user_id && !auth.is_admin() {
        return Err(ApiError::not_found("Container not found"));
    }

    Ok(Json(ContainerInfo {
        id: container.id,
        session_id: container.session_id,
        status: container.status.to_string(),
        pod_name: container.pod_name,
        grpc_endpoint: container.grpc_endpoint,
        created_at: container.created_at,
        last_activity: container.last_activity,
    }))
}

/// Terminate a container
/// DELETE /api/containers/:id
pub async fn delete_container(
    State(state): State<AppState>,
    Path(container_id): Path<Uuid>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<StatusCode, ApiError> {
    let orchestrator = state.require_orchestrator()?;

    // R8-C5: Verify ownership before deletion
    let container = orchestrator
        .get_container(&container_id)
        .await
        .map_err(ApiError::from)?;

    if container.user_id != auth.user_id && !auth.is_admin() {
        return Err(ApiError::not_found("Container not found"));
    }

    orchestrator
        .destroy_container(&container_id)
        .await
        .map_err(ApiError::from)?;

    tracing::info!(container_id = %container_id, user_id = %auth.user_id, "Container terminated via API");

    Ok(StatusCode::NO_CONTENT)
}

/// Pause a container (delete pod, keep PVC)
/// POST /api/containers/:id/pause
pub async fn pause_container(
    State(state): State<AppState>,
    Path(container_id): Path<Uuid>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<ContainerInfo>, ApiError> {
    let orchestrator = state.require_orchestrator()?;

    // R8-C5: Verify ownership before pause
    {
        let container = orchestrator
            .get_container(&container_id)
            .await
            .map_err(ApiError::from)?;
        if container.user_id != auth.user_id && !auth.is_admin() {
            return Err(ApiError::not_found("Container not found"));
        }
    }

    let container = orchestrator
        .pause_container(&container_id)
        .await
        .map_err(ApiError::from)?;

    tracing::info!(container_id = %container_id, user_id = %auth.user_id, "Container paused via API");

    Ok(Json(ContainerInfo {
        id: container.id,
        session_id: container.session_id,
        status: container.status.to_string(),
        pod_name: container.pod_name,
        grpc_endpoint: container.grpc_endpoint,
        created_at: container.created_at,
        last_activity: container.last_activity,
    }))
}

/// Resume a paused container
/// POST /api/containers/:id/resume
pub async fn resume_container(
    State(state): State<AppState>,
    Path(container_id): Path<Uuid>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<ContainerInfo>, ApiError> {
    let orchestrator = state.require_orchestrator()?;

    // R8-C5: Verify ownership before resume
    {
        let container = orchestrator
            .get_container(&container_id)
            .await
            .map_err(ApiError::from)?;
        if container.user_id != auth.user_id && !auth.is_admin() {
            return Err(ApiError::not_found("Container not found"));
        }
    }

    let container = orchestrator
        .resume_container(&container_id)
        .await
        .map_err(ApiError::from)?;

    tracing::info!(container_id = %container_id, user_id = %auth.user_id, "Container resumed via API");

    Ok(Json(ContainerInfo {
        id: container.id,
        session_id: container.session_id,
        status: container.status.to_string(),
        pod_name: container.pod_name,
        grpc_endpoint: container.grpc_endpoint,
        created_at: container.created_at,
        last_activity: container.last_activity,
    }))
}
