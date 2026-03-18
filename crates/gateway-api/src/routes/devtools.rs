//! DevTools query, status, and SSE endpoints.
//!
//! Mirrors the standalone devtools-server's `/v1/*` API surface so the
//! Tauri frontend can query traces directly from gateway-api without
//! requiring a separate devtools-server process.
//!
//! # Feature Gate
//!
//! This module is behind `#[cfg(feature = "devtools")]`.

use axum::{
    extract::{Path, Query, State},
    response::sse::{Event, Sse},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use std::convert::Infallible;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::state::AppState;

/// Create the devtools routes.
///
/// Mounts at `/api/devtools` and exposes both the status endpoint and
/// a `/v1/*` sub-tree that mirrors the standalone devtools-server API.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Status endpoint
        .route("/status", get(devtools_status))
        // Health endpoint (mirrors devtools-server /v1/health)
        .route("/v1/health", get(devtools_health))
        // Trace endpoints
        .route("/v1/traces", get(list_traces))
        .route("/v1/traces/{id}", get(get_trace))
        .route("/v1/traces/{id}/export", get(export_trace))
        // Session endpoints
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/sessions/{id}/traces", get(get_session_traces))
        // Metrics endpoint
        .route("/v1/metrics/summary", get(get_metrics_summary))
        // Project endpoints
        .route("/v1/projects", get(list_projects).post(create_project))
        .route("/v1/projects/{id}", get(get_project).delete(delete_project))
        // SSE endpoints
        .route("/v1/sse/traces/{id}", get(sse_trace))
        .route("/v1/sse/global", get(sse_global))
}

// ── Status / Health ─────────────────────────────────────────────────────

/// GET /api/devtools/status — Report DevTools subsystem status.
async fn devtools_status() -> Json<serde_json::Value> {
    let langfuse_enabled =
        cfg!(feature = "langfuse") && std::env::var("LANGFUSE_PUBLIC_KEY").is_ok();

    Json(serde_json::json!({
        "devtools_enabled": true,
        "langfuse_enabled": langfuse_enabled,
        "langfuse_host": std::env::var("LANGFUSE_HOST").unwrap_or_default(),
    }))
}

/// GET /api/devtools/v1/health — Health check (compatible with devtools-server).
async fn devtools_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "gateway-api/devtools",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

// ── Traces ──────────────────────────────────────────────────────────────

/// GET /api/devtools/v1/traces — List traces with optional filters.
pub async fn list_traces(
    State(state): State<AppState>,
    Query(filter): Query<devtools_core::filter::TraceFilter>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let traces = state
        .devtools_service
        .list_traces(filter)
        .await
        .map_err(|e| {
            tracing::warn!("list_traces error: {}", e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let count = traces.len();
    Ok(Json(serde_json::json!({
        "data": traces,
        "count": count,
    })))
}

/// GET /api/devtools/v1/traces/{id} — Get a trace with its observation tree.
pub async fn get_trace(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let tree = state
        .devtools_service
        .get_trace_tree(&id)
        .await
        .map_err(|e| {
            tracing::warn!("get_trace error: {}", e);
            match e {
                devtools_core::DevtoolsError::TraceNotFound { .. } => {
                    axum::http::StatusCode::NOT_FOUND
                }
                _ => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            }
        })?;
    Ok(Json(serde_json::to_value(&tree).unwrap_or_default()))
}

/// GET /api/devtools/v1/traces/{id}/export — Export a complete trace as JSON.
async fn export_trace(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let export = state
        .devtools_service
        .export_trace(&id)
        .await
        .map_err(|e| {
            tracing::warn!("export_trace error: {}", e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(export))
}

// ── Sessions ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SessionListQuery {
    project_id: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    50
}

/// GET /api/devtools/v1/sessions — List sessions.
async fn list_sessions(
    State(state): State<AppState>,
    Query(query): Query<SessionListQuery>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let sessions = state
        .devtools_service
        .list_sessions(query.project_id.as_deref(), query.limit)
        .await
        .map_err(|e| {
            tracing::warn!("list_sessions error: {}", e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let count = sessions.len();
    Ok(Json(serde_json::json!({
        "data": sessions,
        "count": count,
    })))
}

/// GET /api/devtools/v1/sessions/{id}/traces — Get all traces in a session.
async fn get_session_traces(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let traces = state
        .devtools_service
        .get_session_traces(&id)
        .await
        .map_err(|e| {
            tracing::warn!("get_session_traces error: {}", e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let count = traces.len();
    Ok(Json(serde_json::json!({
        "data": traces,
        "count": count,
    })))
}

// ── Metrics ─────────────────────────────────────────────────────────────

/// GET /api/devtools/v1/metrics/summary — Aggregated metrics.
async fn get_metrics_summary(
    State(state): State<AppState>,
    Query(filter): Query<devtools_core::filter::MetricsFilter>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let metrics = state
        .devtools_service
        .get_metrics(filter)
        .await
        .map_err(|e| {
            tracing::warn!("get_metrics error: {}", e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(serde_json::to_value(&metrics).unwrap_or_default()))
}

// ── Projects ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateProjectRequest {
    name: String,
    service_type: String,
    endpoint: Option<String>,
    #[serde(default)]
    metadata: serde_json::Map<String, serde_json::Value>,
}

/// POST /api/devtools/v1/projects — Create a new project.
async fn create_project(
    State(state): State<AppState>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<(axum::http::StatusCode, Json<serde_json::Value>), axum::http::StatusCode> {
    let id = req
        .name
        .to_lowercase()
        .replace(' ', "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect::<String>();
    let api_key = format!("pk_proj_{}_{}", id, uuid::Uuid::new_v4().simple());

    let project = devtools_core::types::Project {
        id: id.clone(),
        name: req.name,
        service_type: req.service_type,
        endpoint: req.endpoint,
        api_key: api_key.clone(),
        created_at: Utc::now(),
        metadata: req.metadata,
    };

    state
        .devtools_service
        .create_project(project.clone())
        .await
        .map_err(|e| {
            tracing::warn!("create_project error: {}", e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(serde_json::json!({
            "id": id,
            "api_key": api_key,
            "data": project,
        })),
    ))
}

/// GET /api/devtools/v1/projects — List all projects.
async fn list_projects(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let projects = state.devtools_service.list_projects().await.map_err(|e| {
        tracing::warn!("list_projects error: {}", e);
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let count = projects.len();
    Ok(Json(serde_json::json!({
        "data": projects,
        "count": count,
    })))
}

/// GET /api/devtools/v1/projects/{id} — Get a project.
async fn get_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let project = state.devtools_service.get_project(&id).await.map_err(|e| {
        tracing::warn!("get_project error: {}", e);
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    })?;

    match project {
        Some(p) => Ok(Json(serde_json::to_value(&p).unwrap_or_default())),
        None => Err(axum::http::StatusCode::NOT_FOUND),
    }
}

/// DELETE /api/devtools/v1/projects/{id} — Delete a project.
async fn delete_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    state
        .devtools_service
        .delete_project(&id)
        .await
        .map_err(|e| {
            tracing::warn!("delete_project error: {}", e);
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(serde_json::json!({"status": "deleted", "id": id})))
}

// ── SSE (Server-Sent Events) ────────────────────────────────────────────

/// GET /api/devtools/v1/sse/traces/{id} — SSE stream for a specific trace.
async fn sse_trace(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.devtools_service.subscribe_trace(&id).await;
    let stream = ReceiverStream::new(rx).map(|obs| {
        let data = serde_json::to_string(&obs).unwrap_or_default();
        Ok::<_, Infallible>(Event::default().event("observation").data(data))
    });
    Sse::new(stream)
}

/// GET /api/devtools/v1/sse/global — SSE stream for global trace events.
async fn sse_global(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.devtools_service.subscribe_global().await;
    let stream = ReceiverStream::new(rx).map(|event| {
        let data = serde_json::to_string(&event).unwrap_or_default();
        let event_type = match &event {
            devtools_core::TraceEvent::TraceCreated { .. } => "trace_created",
            devtools_core::TraceEvent::TraceUpdated { .. } => "trace_updated",
            devtools_core::TraceEvent::ObservationCreated { .. } => "observation_created",
        };
        Ok::<_, Infallible>(Event::default().event(event_type).data(data))
    });
    Sse::new(stream)
}
