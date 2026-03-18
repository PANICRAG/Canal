//! SSE stream handlers for real-time trace observation.

use axum::extract::{Path, State};
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::state::AppState;

/// GET /v1/sse/traces/{id} — SSE stream of observations for a specific trace.
pub async fn sse_trace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let rx = state.devtools.subscribe_trace(&id).await;
    let stream = ReceiverStream::new(rx).map(|obs| {
        let data = serde_json::to_string(&obs).unwrap_or_default();
        Ok::<_, Infallible>(Event::default().event("observation").data(data))
    });
    Sse::new(stream)
}

/// GET /v1/sse/global — SSE stream of global trace events.
pub async fn sse_global(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rx = state.devtools.subscribe_global().await;
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
