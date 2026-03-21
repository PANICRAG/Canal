//! Storage and event bus trait abstractions.
//!
//! All traits are async + Send + Sync for use with tokio and Arc-based sharing.

use async_trait::async_trait;

use crate::error::DevtoolsError;
use crate::filter::{MetricsFilter, ObservationUpdate, TraceFilter, TraceUpdate};
use crate::types::*;

type Result<T> = std::result::Result<T, DevtoolsError>;

/// Pluggable storage backend for traces and observations.
#[async_trait]
pub trait TraceStore: Send + Sync {
    // ── Ingest ──────────────────────────────────────────────────────────

    /// Store a new trace.
    async fn ingest_trace(&self, trace: Trace) -> Result<()>;

    /// Store a new observation (span, generation, or event).
    async fn ingest_observation(&self, obs: Observation) -> Result<()>;

    /// Update an existing trace (status, end_time, output, etc.).
    async fn update_trace(&self, id: &str, update: TraceUpdate) -> Result<()>;

    /// Update an existing observation.
    async fn update_observation(&self, id: &str, update: ObservationUpdate) -> Result<()>;

    // ── Query: Traces ───────────────────────────────────────────────────

    /// Get a single trace by ID.
    async fn get_trace(&self, id: &str) -> Result<Option<Trace>>;

    /// List traces matching a filter (paginated).
    async fn list_traces(&self, filter: TraceFilter) -> Result<Vec<Trace>>;

    /// Get all observations for a trace.
    async fn get_trace_observations(&self, trace_id: &str) -> Result<Vec<Observation>>;

    // ── Query: Sessions ─────────────────────────────────────────────────

    /// Get a session by ID.
    async fn get_session(&self, id: &str) -> Result<Option<Session>>;

    /// List recent sessions.
    async fn list_sessions(&self, project_id: Option<&str>, limit: usize) -> Result<Vec<Session>>;

    /// Get all traces belonging to a session.
    async fn get_session_traces(&self, session_id: &str) -> Result<Vec<Trace>>;

    // ── Query: Metrics ──────────────────────────────────────────────────

    /// Compute aggregated metrics for a filter scope.
    async fn get_metrics_summary(&self, filter: MetricsFilter) -> Result<MetricsSummary>;

    // ── Projects ────────────────────────────────────────────────────────

    /// Create a new project.
    async fn create_project(&self, project: Project) -> Result<()>;

    /// Get a project by ID.
    async fn get_project(&self, id: &str) -> Result<Option<Project>>;

    /// List all projects.
    async fn list_projects(&self) -> Result<Vec<Project>>;

    /// Delete a project and all its data.
    async fn delete_project(&self, id: &str) -> Result<()>;

    /// Resolve a project API key to a project ID.
    async fn resolve_project_key(&self, api_key: &str) -> Result<Option<String>>;
}

/// Pluggable exporter for sending traces to external systems (e.g., Langfuse).
///
/// Exporters receive data fire-and-forget after the store has persisted it.
/// Errors should be logged internally, not propagated.
#[async_trait]
pub trait TraceExporter: Send + Sync {
    /// Export a newly created trace.
    async fn export_trace(&self, trace: &Trace) -> Result<()>;

    /// Export a newly created observation.
    async fn export_observation(&self, obs: &Observation) -> Result<()>;

    /// Export a trace update.
    async fn export_trace_update(&self, id: &str, update: &TraceUpdate) -> Result<()>;

    /// Export an observation update.
    async fn export_observation_update(&self, id: &str, update: &ObservationUpdate) -> Result<()>;

    /// Flush any buffered data to the external system.
    async fn flush(&self) -> Result<()>;
}

/// Real-time event bus for publishing and subscribing to trace updates.
#[async_trait]
pub trait EventBus: Send + Sync {
    /// Subscribe to real-time observation updates for a specific trace.
    async fn subscribe_trace(&self, trace_id: &str) -> tokio::sync::mpsc::Receiver<Observation>;

    /// Subscribe to global trace-level events (new traces, updates).
    async fn subscribe_global(&self) -> tokio::sync::mpsc::Receiver<TraceEvent>;

    /// Publish a trace event to all relevant subscribers.
    async fn publish_trace_event(&self, event: TraceEvent);

    /// Publish an observation to trace-specific subscribers.
    async fn publish_observation(&self, trace_id: &str, obs: Observation);
}
