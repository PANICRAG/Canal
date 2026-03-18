//! GET session query handlers.

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct SessionListQuery {
    pub project_id: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    50
}

/// GET /v1/sessions — list sessions.
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SessionListQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let sessions = state
        .devtools
        .list_sessions(query.project_id.as_deref(), query.limit)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "data": sessions,
        "count": sessions.len(),
    })))
}

/// GET /v1/sessions/{id}/traces — get all traces in a session.
pub async fn get_session_traces(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let traces = state
        .devtools
        .get_session_traces(&id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "data": traces,
        "count": traces.len(),
    })))
}
