//! HTTP request handlers for devtools-server.

pub mod alerts;
pub mod database;
pub mod infrastructure;
pub mod ingest;
pub mod logs;
pub mod metrics;
pub mod projects;
pub mod sessions;
pub mod sse;
pub mod traces;

use axum::extract::State;
use axum::Json;
use std::sync::Arc;

use crate::state::AppState;

/// GET /v1/health — health check (no auth required)
pub async fn health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "devtools-server",
        "version": env!("CARGO_PKG_VERSION"),
        "port": state.config.port,
    }))
}
