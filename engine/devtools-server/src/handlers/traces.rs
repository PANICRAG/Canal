//! GET trace query handlers.

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::Json;
use std::sync::Arc;

use devtools_core::filter::TraceFilter;

use crate::error::ApiError;
use crate::state::AppState;

/// GET /v1/traces — list traces with optional filters.
pub async fn list_traces(
    State(state): State<Arc<AppState>>,
    Query(filter): Query<TraceFilter>,
) -> Result<impl IntoResponse, ApiError> {
    let traces = state
        .devtools
        .list_traces(filter)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({
        "data": traces,
        "count": traces.len(),
    })))
}

/// GET /v1/traces/{id} — get a trace with its observation tree.
pub async fn get_trace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let tree = state
        .devtools
        .get_trace_tree(&id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(tree))
}

/// GET /v1/traces/{id}/export — export a complete trace as JSON.
pub async fn export_trace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let export = state
        .devtools
        .export_trace(&id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(export))
}
