//! Debug Dashboard API endpoints (dev-only)
//!
//! Provides debug observability endpoints for the frontend debug dashboard.
//! All routes are gated behind a runtime `DEV_MODE` check — they return 404
//! in production even when the `context-engineering` feature is compiled in.
//!
//! # Endpoints
//!
//! - `GET /debug/flags` - Current context resolver feature flags
//! - `GET /debug/trace/:session_id` - JSONL conversation trace for a session
//! - `GET /debug/executions` - List recent executions (requires `graph` feature)
//! - `GET /debug/executions/active` - List active (running) executions
//! - `GET /debug/executions/:id` - Get a single execution record
//! - `GET /debug/executions/:id/events` - Get events with offset/type filtering
//! - `GET /debug/executions/:id/stream` - SSE stream for a single execution
//! - `GET /debug/executions/stream` - SSE global execution stream

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    middleware,
    response::sse::{Event, Sse},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Check if dev mode is enabled (env `DEV_MODE=true` or `DEV_MODE=1`).
fn is_dev_mode() -> bool {
    std::env::var("DEV_MODE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

/// Middleware that rejects requests with 404 when not in dev mode.
async fn dev_mode_middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    if !is_dev_mode() {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(next.run(req).await)
}

/// Create the debug routes (all behind dev-mode middleware).
pub fn debug_routes() -> Router<AppState> {
    let mut router = Router::new()
        .route("/flags", get(get_flags))
        .route("/trace/{session_id}", get(get_trace));

    // Execution store endpoints (requires graph feature)
    #[cfg(feature = "graph")]
    {
        router = router
            .route("/executions", get(list_executions))
            .route("/executions/active", get(list_active_executions))
            .route("/executions/stream", get(global_execution_stream))
            .route("/executions/{id}", get(get_execution))
            .route("/executions/{id}/events", get(get_execution_events))
            .route("/executions/{id}/stream", get(execution_stream))
            .route("/routing-history", get(routing_history));
    }

    router.layer(middleware::from_fn(dev_mode_middleware))
}

// ============ Existing Handlers ============

/// Return current `ContextResolverFlags` from AppState.
///
/// Response: `{ "scoring_rollout_pct": 0, "memory_dual_write": false, ... }`
async fn get_flags(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(&*state.context_resolver_flags).unwrap_or_default())
}

/// Query parameters for the trace endpoint.
#[derive(Debug, Deserialize)]
pub struct TraceQuery {
    /// Skip first N lines, return only new events (for incremental loading).
    offset: Option<usize>,
}

/// Response for the trace endpoint.
#[derive(Debug, Serialize)]
pub struct TraceResponse {
    /// Parsed trace events (JSON objects from JSONL lines).
    pub events: Vec<serde_json::Value>,
    /// Total number of lines in the trace file.
    pub total_lines: usize,
    /// The offset that was applied.
    pub offset: usize,
}

/// Read a JSONL trace file for the given session_id with optional offset.
///
/// Path traversal protection: rejects session_ids containing `..`, `/`, or `\`.
async fn get_trace(
    Path(session_id): Path<String>,
    Query(query): Query<TraceQuery>,
) -> Result<Json<TraceResponse>, StatusCode> {
    // R4-M: Path traversal protection — also reject null bytes and URL-encoded variants
    let decoded = urlencoding::decode(&session_id).unwrap_or_default();
    if decoded.contains("..")
        || decoded.contains('/')
        || decoded.contains('\\')
        || decoded.contains('\0')
        || decoded.is_empty()
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    let trace_dir = std::env::var("TRACE_DIR").unwrap_or_else(|_| ".canal/traces".into());
    let path = std::path::PathBuf::from(&trace_dir).join(format!("{}.jsonl", session_id));

    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let all_lines: Vec<&str> = content.lines().collect();
    let total_lines = all_lines.len();
    let offset = query.offset.unwrap_or(0);

    let events: Vec<serde_json::Value> = all_lines
        .into_iter()
        .skip(offset)
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    Ok(Json(TraceResponse {
        events,
        total_lines,
        offset,
    }))
}

// ============ Execution Store Handlers (A22) ============

/// Query parameters for listing executions.
#[cfg(feature = "graph")]
#[derive(Debug, Deserialize)]
pub struct ListExecutionsQuery {
    /// Maximum number of executions to return (default: 20).
    limit: Option<usize>,
}

/// Query parameters for the SSE stream.
#[cfg(feature = "graph")]
#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    /// Number of recent events to replay before streaming.
    replay: Option<usize>,
}

/// List recent executions.
///
/// `GET /debug/executions?limit=20`
#[cfg(feature = "graph")]
async fn list_executions(
    State(state): State<AppState>,
    Query(query): Query<ListExecutionsQuery>,
) -> Json<serde_json::Value> {
    let limit = query.limit.unwrap_or(20);
    let summaries = state.execution_store.list_recent(limit).await;

    Json(serde_json::json!({
        "executions": summaries,
        "count": summaries.len(),
    }))
}

/// List active (running) executions.
///
/// `GET /debug/executions/active`
#[cfg(feature = "graph")]
async fn list_active_executions(State(state): State<AppState>) -> Json<serde_json::Value> {
    let active = state.execution_store.list_active();

    Json(serde_json::json!({
        "active": active,
        "count": active.len(),
    }))
}

/// Get a single execution record.
///
/// `GET /debug/executions/{id}`
#[cfg(feature = "graph")]
async fn get_execution(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let summary = state
        .execution_store
        .get_execution(&id)
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(serde_json::to_value(&summary).unwrap_or_default()))
}

/// Query parameters for event retrieval.
#[cfg(feature = "graph")]
#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    /// Skip first N events.
    offset: Option<usize>,
    /// Maximum number of events to return.
    limit: Option<usize>,
    /// Filter by event type (comma-separated, e.g., "LlmRequest,ToolCall").
    #[serde(rename = "type")]
    event_type: Option<String>,
}

/// Get events for an execution with offset and optional type filtering.
///
/// `GET /debug/executions/{id}/events?offset=0&limit=100&type=LlmRequest,ToolCall`
#[cfg(feature = "graph")]
async fn get_execution_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<EventsQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit;
    let events = state.execution_store.get_events(&id, offset, limit);

    if events.is_empty() && state.execution_store.get_execution(&id).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Optional type filtering
    let events = if let Some(ref type_filter) = query.event_type {
        let types: Vec<&str> = type_filter.split(',').map(|s| s.trim()).collect();
        events
            .into_iter()
            .filter(|e| {
                let payload_json = serde_json::to_value(&e.payload).unwrap_or_default();
                if let Some(t) = payload_json.get("type").and_then(|v| v.as_str()) {
                    types.iter().any(|filter| t == *filter)
                } else {
                    false
                }
            })
            .collect()
    } else {
        events
    };

    Ok(Json(serde_json::json!({
        "events": events,
        "count": events.len(),
        "offset": offset,
    })))
}

/// SSE stream for a single execution (with replay).
///
/// `GET /debug/executions/{id}/stream`
///
/// Replays recent events then streams live updates.
#[cfg(feature = "graph")]
async fn execution_stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StreamQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>, StatusCode> {
    use futures::StreamExt;
    use tokio_stream::wrappers::ReceiverStream;

    let replay_count = query.replay.unwrap_or(50);

    // subscribe() is sync — returns (Receiver, Vec<replay_events>)
    let (rx, replay_events) = state.execution_store.subscribe(&id, replay_count);

    // If execution doesn't exist, the replay will be empty AND no live events
    // Check if the execution exists at all
    if state.execution_store.get_execution(&id).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Replay events first, then live stream
    let replay_stream = futures::stream::iter(replay_events).map(|event| {
        let data = serde_json::to_string(&event).unwrap_or_default();
        Ok(Event::default().data(data))
    });

    let live_stream = ReceiverStream::new(rx).map(|event| {
        let data = serde_json::to_string(&event).unwrap_or_default();
        Ok(Event::default().data(data))
    });

    Ok(Sse::new(replay_stream.chain(live_stream)))
}

/// SSE global execution stream.
///
/// `GET /debug/executions/stream`
///
/// Emits summary-level events for all executions.
#[cfg(feature = "graph")]
async fn global_execution_stream(
    State(state): State<AppState>,
) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
    use futures::StreamExt;
    use tokio_stream::wrappers::ReceiverStream;

    let rx = state.execution_store.subscribe_global().await;

    let stream = ReceiverStream::new(rx).map(|event| {
        let data = serde_json::to_string(&event).unwrap_or_default();
        Ok(Event::default().data(data))
    });

    Sse::new(stream)
}

// ============ A24: Routing History ============

/// Query parameters for routing history.
#[cfg(feature = "graph")]
#[derive(Debug, Deserialize)]
pub struct RoutingHistoryQuery {
    /// Maximum number of entries to return (default: 50).
    limit: Option<usize>,
}

/// A routing classification history entry.
#[cfg(feature = "graph")]
#[derive(Debug, Serialize)]
pub struct RoutingHistoryEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub session_id: String,
    pub source: String,
    pub category: String,
    pub routed_to: String,
    pub confidence: Option<f64>,
    pub reasoning: String,
    pub classification_ms: u64,
    pub cache_hit: bool,
}

/// List recent routing classification decisions.
///
/// `GET /debug/routing-history?limit=50`
///
/// Queries executions with "route-" prefix and extracts RoutingClassified events.
#[cfg(feature = "graph")]
async fn routing_history(
    State(state): State<AppState>,
    Query(query): Query<RoutingHistoryQuery>,
) -> Json<serde_json::Value> {
    let limit = query.limit.unwrap_or(50);
    // Fetch more than limit to account for non-routing executions
    let executions = state.execution_store.list_recent(limit * 3).await;

    let entries: Vec<RoutingHistoryEntry> = executions
        .iter()
        .filter(|e| e.execution_id.starts_with("route-"))
        .take(limit)
        .filter_map(|e| {
            let events = state
                .execution_store
                .get_events(&e.execution_id, 0, Some(1));
            events.first().and_then(|ev| {
                let payload_json = serde_json::to_value(&ev.payload).ok()?;
                if payload_json.get("type")?.as_str()? == "RoutingClassified" {
                    Some(RoutingHistoryEntry {
                        timestamp: ev.timestamp,
                        session_id: e.execution_id.strip_prefix("route-").unwrap_or("").into(),
                        source: payload_json
                            .get("source")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .into(),
                        category: payload_json
                            .get("category")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .into(),
                        routed_to: payload_json
                            .get("routed_to")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .into(),
                        confidence: payload_json.get("confidence").and_then(|v| v.as_f64()),
                        reasoning: payload_json
                            .get("reasoning")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .into(),
                        classification_ms: payload_json
                            .get("classification_ms")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        cache_hit: payload_json
                            .get("cache_hit")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    })
                } else {
                    None
                }
            })
        })
        .collect();

    Json(serde_json::json!({
        "routing_history": entries,
        "count": entries.len(),
    }))
}
