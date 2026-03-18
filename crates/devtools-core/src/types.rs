//! Core data model — Langfuse-style Trace/Observation/Session/Project hierarchy.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ============================================================================
// Observation — unified type in the trace tree
// ============================================================================

/// A single observation within a trace. Tagged union of Span, Generation, Event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "observation_type", rename_all = "snake_case")]
pub enum Observation {
    Span(SpanData),
    Generation(GenerationData),
    Event(EventData),
}

impl Observation {
    /// Return the observation ID regardless of variant.
    pub fn id(&self) -> &str {
        match self {
            Observation::Span(s) => &s.id,
            Observation::Generation(g) => &g.id,
            Observation::Event(e) => &e.id,
        }
    }

    /// Return the trace_id regardless of variant.
    pub fn trace_id(&self) -> &str {
        match self {
            Observation::Span(s) => &s.trace_id,
            Observation::Generation(g) => &g.trace_id,
            Observation::Event(e) => &e.trace_id,
        }
    }

    /// Return the parent_id regardless of variant.
    pub fn parent_id(&self) -> Option<&str> {
        match self {
            Observation::Span(s) => s.parent_id.as_deref(),
            Observation::Generation(g) => g.parent_id.as_deref(),
            Observation::Event(e) => e.parent_id.as_deref(),
        }
    }

    /// Return the start time of this observation.
    pub fn start_time(&self) -> DateTime<Utc> {
        match self {
            Observation::Span(s) => s.start_time,
            Observation::Generation(g) => g.start_time,
            Observation::Event(e) => e.time,
        }
    }

    /// Return the service name (for distributed tracing).
    pub fn service_name(&self) -> Option<&str> {
        match self {
            Observation::Span(s) => s.service_name.as_deref(),
            Observation::Generation(g) => g.service_name.as_deref(),
            Observation::Event(e) => e.service_name.as_deref(),
        }
    }
}

// ============================================================================
// Span — has duration, represents a work unit
// ============================================================================

/// A span represents a work unit with a start and optional end time.
///
/// Examples: "ANALYZE", "EXECUTE", "graph.node.classify"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanData {
    pub id: String,
    pub trace_id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub input: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    pub status: ObservationStatus,
    pub level: ObservationLevel,
    /// Service that created this span (for distributed tracing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
}

// ============================================================================
// Generation — LLM call with token/cost tracking
// ============================================================================

/// A generation represents an LLM call with model, token, and cost data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationData {
    pub id: String,
    pub trace_id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub model: String,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub input: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub total_tokens: i32,
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    pub status: ObservationStatus,
    /// Service that created this generation (for distributed tracing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
}

// ============================================================================
// Event — discrete, no duration
// ============================================================================

/// An event is a point-in-time occurrence without duration.
///
/// Examples: "tool.browser_click", "checkpoint.saved"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventData {
    pub id: String,
    pub trace_id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub time: DateTime<Utc>,
    pub input: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    pub level: ObservationLevel,
    /// Service that created this event (for distributed tracing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
}

// ============================================================================
// Trace — top-level container for one agent execution
// ============================================================================

/// A trace represents one complete agent execution (e.g., one user query).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    pub id: String,
    pub project_id: String,
    pub session_id: Option<String>,
    pub name: Option<String>,
    pub user_id: Option<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub input: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub status: TraceStatus,
    /// Aggregated total tokens across all generations.
    #[serde(default)]
    pub total_tokens: i64,
    /// Aggregated total cost in USD across all generations.
    #[serde(default)]
    pub total_cost_usd: f64,
    /// Count of observations in this trace.
    #[serde(default)]
    pub observation_count: usize,
}

// ============================================================================
// Session — multi-turn conversation container
// ============================================================================

/// A session groups multiple traces (e.g., a multi-turn conversation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

// ============================================================================
// Project — monitored service instance
// ============================================================================

/// A project represents a monitored service instance (like Langfuse's project).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub service_type: String,
    pub endpoint: Option<String>,
    pub api_key: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

// ============================================================================
// Enums
// ============================================================================

/// Status of a trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceStatus {
    Running,
    Completed,
    Error,
}

/// Status of an observation (span/generation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatus {
    Running,
    Completed,
    Error,
}

/// Severity level for observations and events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationLevel {
    Debug,
    Info,
    Warning,
    Error,
}

// ============================================================================
// Metrics
// ============================================================================

/// Aggregated metrics summary for a project or filter scope.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsSummary {
    pub total_traces: usize,
    pub total_observations: usize,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub avg_trace_duration_ms: f64,
    pub model_usage: Vec<ModelUsage>,
    pub traces_by_status: TracesByStatus,
}

/// Per-model usage breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model: String,
    pub call_count: usize,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
}

/// Trace count by status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TracesByStatus {
    pub running: usize,
    pub completed: usize,
    pub error: usize,
}

// ============================================================================
// Trace tree (query result)
// ============================================================================

/// A complete trace with its observation tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceTree {
    pub trace: Trace,
    pub observations: Vec<Observation>,
}

// ============================================================================
// Real-time events
// ============================================================================

/// Events published on the global event bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum TraceEvent {
    TraceCreated { trace: Trace },
    TraceUpdated { trace: Trace },
    ObservationCreated { observation: Observation },
}

// ============================================================================
// Ingest batch
// ============================================================================

/// A batch ingest request containing multiple traces and observations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestBatch {
    #[serde(default)]
    pub traces: Vec<Trace>,
    #[serde(default)]
    pub observations: Vec<Observation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_serialization_roundtrip() {
        let trace = Trace {
            id: "tr-1".into(),
            project_id: "proj-1".into(),
            session_id: Some("sess-1".into()),
            name: Some("test-trace".into()),
            user_id: Some("user-1".into()),
            start_time: Utc::now(),
            end_time: None,
            input: Some(serde_json::json!({"query": "hello"})),
            output: None,
            metadata: serde_json::Map::new(),
            tags: vec!["test".into()],
            status: TraceStatus::Running,
            total_tokens: 0,
            total_cost_usd: 0.0,
            observation_count: 0,
        };

        let json = serde_json::to_string(&trace).unwrap();
        let deserialized: Trace = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "tr-1");
        assert_eq!(deserialized.status, TraceStatus::Running);
        assert_eq!(deserialized.tags, vec!["test"]);
    }

    #[test]
    fn test_observation_variants_serialize() {
        let span = Observation::Span(SpanData {
            id: "span-1".into(),
            trace_id: "tr-1".into(),
            parent_id: None,
            name: "ANALYZE".into(),
            start_time: Utc::now(),
            end_time: None,
            input: None,
            output: None,
            metadata: serde_json::Map::new(),
            status: ObservationStatus::Running,
            level: ObservationLevel::Info,
            service_name: None,
        });

        let gen = Observation::Generation(GenerationData {
            id: "gen-1".into(),
            trace_id: "tr-1".into(),
            parent_id: Some("span-1".into()),
            name: "llm-call".into(),
            model: "claude-sonnet-4-5-20250929".into(),
            start_time: Utc::now(),
            end_time: None,
            input: None,
            output: None,
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cost_usd: Some(0.001),
            metadata: serde_json::Map::new(),
            status: ObservationStatus::Completed,
            service_name: None,
        });

        let event = Observation::Event(EventData {
            id: "evt-1".into(),
            trace_id: "tr-1".into(),
            parent_id: Some("span-1".into()),
            name: "tool.browser_click".into(),
            time: Utc::now(),
            input: Some(serde_json::json!({"selector": "#btn"})),
            output: None,
            metadata: serde_json::Map::new(),
            level: ObservationLevel::Debug,
            service_name: None,
        });

        // Verify each variant serializes with the correct tag
        let span_json = serde_json::to_value(&span).unwrap();
        assert_eq!(span_json["observation_type"], "span");

        let gen_json = serde_json::to_value(&gen).unwrap();
        assert_eq!(gen_json["observation_type"], "generation");

        let evt_json = serde_json::to_value(&event).unwrap();
        assert_eq!(evt_json["observation_type"], "event");

        // Verify roundtrip
        let span_rt: Observation = serde_json::from_value(span_json).unwrap();
        assert_eq!(span_rt.id(), "span-1");

        let gen_rt: Observation = serde_json::from_value(gen_json).unwrap();
        assert_eq!(gen_rt.id(), "gen-1");

        let evt_rt: Observation = serde_json::from_value(evt_json).unwrap();
        assert_eq!(evt_rt.id(), "evt-1");
    }

    #[test]
    fn test_project_serialization() {
        let mut meta = serde_json::Map::new();
        meta.insert("env".into(), serde_json::json!("production"));

        let project = Project {
            id: "proj-1".into(),
            name: "Gateway API".into(),
            service_type: "gateway-api".into(),
            endpoint: Some("http://localhost:4000".into()),
            api_key: "pk_proj_gw_xxx".into(),
            created_at: Utc::now(),
            metadata: meta,
        };

        let json = serde_json::to_string(&project).unwrap();
        let deserialized: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "proj-1");
        assert_eq!(deserialized.service_type, "gateway-api");
        assert_eq!(deserialized.metadata["env"], "production");
    }

    #[test]
    fn test_observation_accessors() {
        let span = Observation::Span(SpanData {
            id: "s1".into(),
            trace_id: "t1".into(),
            parent_id: Some("p1".into()),
            name: "test".into(),
            start_time: Utc::now(),
            end_time: None,
            input: None,
            output: None,
            metadata: serde_json::Map::new(),
            status: ObservationStatus::Running,
            level: ObservationLevel::Info,
            service_name: None,
        });

        assert_eq!(span.id(), "s1");
        assert_eq!(span.trace_id(), "t1");
        assert_eq!(span.parent_id(), Some("p1"));
    }

    #[test]
    fn test_trace_status_serde() {
        let running = serde_json::to_string(&TraceStatus::Running).unwrap();
        assert_eq!(running, "\"running\"");
        let completed: TraceStatus = serde_json::from_str("\"completed\"").unwrap();
        assert_eq!(completed, TraceStatus::Completed);
    }
}
