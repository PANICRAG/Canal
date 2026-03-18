//! Project management handlers.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use devtools_core::types::Project;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    pub service_type: String,
    pub endpoint: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

/// POST /v1/projects — create a new project.
pub async fn create_project(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let id = req
        .name
        .to_lowercase()
        .replace(' ', "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect::<String>();
    let api_key = format!("pk_proj_{}_{}", id, Uuid::new_v4().simple());

    let project = Project {
        id: id.clone(),
        name: req.name,
        service_type: req.service_type,
        endpoint: req.endpoint,
        api_key: api_key.clone(),
        created_at: Utc::now(),
        metadata: req.metadata,
    };

    state
        .devtools
        .create_project(project.clone())
        .await
        .map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": id,
            "api_key": api_key,
            "project": project,
        })),
    ))
}

/// GET /v1/projects — list all projects.
pub async fn list_projects(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    let projects = state
        .devtools
        .list_projects()
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "data": projects,
        "count": projects.len(),
    })))
}

/// GET /v1/projects/{id} — get a project.
pub async fn get_project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let project = state
        .devtools
        .get_project(&id)
        .await
        .map_err(ApiError::from)?;

    match project {
        Some(p) => Ok(Json(serde_json::json!(p))),
        None => Err(ApiError::from(
            devtools_core::DevtoolsError::ProjectNotFound { id },
        )),
    }
}

/// DELETE /v1/projects/{id} — delete a project.
pub async fn delete_project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    state
        .devtools
        .delete_project(&id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({"status": "deleted", "id": id})))
}
