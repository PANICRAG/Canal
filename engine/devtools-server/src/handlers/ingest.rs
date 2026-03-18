//! POST ingest handlers — receive traces and observations.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use std::sync::Arc;

use devtools_core::types::{IngestBatch, Observation, Trace};

use crate::error::ApiError;
use crate::state::AppState;

/// POST /v1/traces — create or update a trace.
pub async fn create_trace(
    State(state): State<Arc<AppState>>,
    Json(trace): Json<Trace>,
) -> Result<impl IntoResponse, ApiError> {
    state
        .devtools
        .ingest_trace(trace)
        .await
        .map_err(ApiError::from)?;
    Ok((StatusCode::OK, Json(serde_json::json!({"status": "ok"}))))
}

/// POST /v1/observations — create or update an observation.
pub async fn create_observation(
    State(state): State<Arc<AppState>>,
    Json(obs): Json<Observation>,
) -> Result<impl IntoResponse, ApiError> {
    state
        .devtools
        .ingest_observation(obs)
        .await
        .map_err(ApiError::from)?;
    Ok((StatusCode::OK, Json(serde_json::json!({"status": "ok"}))))
}

/// POST /v1/ingest — batch ingest traces and observations.
pub async fn batch_ingest(
    State(state): State<Arc<AppState>>,
    Json(batch): Json<IngestBatch>,
) -> Result<impl IntoResponse, ApiError> {
    state
        .devtools
        .ingest_batch(batch)
        .await
        .map_err(ApiError::from)?;
    Ok((StatusCode::OK, Json(serde_json::json!({"status": "ok"}))))
}
