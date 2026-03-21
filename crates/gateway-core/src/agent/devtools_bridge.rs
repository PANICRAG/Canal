//! Bridge between gateway-core agent execution and devtools-core.
//!
//! Translates agent loop events into devtools observations (Span, Generation, Event).
//! Can operate in embedded mode (direct `DevtoolsService` call) or remote mode
//! (HTTP calls to a standalone devtools-server).
//!
//! # Feature Gate
//!
//! This module is behind `#[cfg(feature = "devtools")]`.

use chrono::Utc;
use devtools_core::{
    DevtoolsService, EventData, GenerationData, Observation, ObservationLevel, ObservationStatus,
    SpanData, Trace, TraceStatus,
};
use serde_json::Value;
use std::sync::Arc;
use tracing::warn;

/// Adapter that translates agent loop events into devtools observations.
///
/// Created per-service and shared across agent executions.
pub struct DevtoolsBridge {
    service: Arc<DevtoolsService>,
    project_id: String,
}

impl DevtoolsBridge {
    /// Create a new bridge with a direct service reference (embedded mode).
    pub fn new(service: Arc<DevtoolsService>, project_id: &str) -> Self {
        Self {
            service,
            project_id: project_id.to_string(),
        }
    }

    /// Create a trace for a new agent execution.
    pub async fn start_trace(
        &self,
        session_id: &str,
        execution_id: &str,
        name: Option<&str>,
        input: Value,
    ) {
        let trace = Trace {
            id: execution_id.to_string(),
            project_id: self.project_id.clone(),
            session_id: Some(session_id.to_string()),
            name: name.map(|n| n.to_string()),
            user_id: None,
            start_time: Utc::now(),
            end_time: None,
            input: Some(input),
            output: None,
            metadata: serde_json::Map::new(),
            tags: vec![],
            status: TraceStatus::Running,
            total_tokens: 0,
            total_cost_usd: 0.0,
            observation_count: 0,
        };

        if let Err(e) = self.service.ingest_trace(trace).await {
            warn!("Failed to start devtools trace: {}", e);
        }
    }

    /// Complete a trace when execution finishes.
    pub async fn end_trace(&self, execution_id: &str, output: Value, status: TraceStatus) {
        let update = devtools_core::TraceUpdate {
            status: Some(status),
            end_time: Some(Utc::now()),
            output: Some(output),
            ..Default::default()
        };

        if let Err(e) = self.service.update_trace(execution_id, update).await {
            warn!("Failed to end devtools trace: {}", e);
        }
    }

    /// Record an agent loop step as a Span.
    pub async fn record_step(
        &self,
        execution_id: &str,
        step_name: &str,
        input: Option<Value>,
        parent_id: Option<&str>,
    ) -> String {
        let span_id = format!(
            "{}-{}-{}",
            execution_id,
            step_name,
            Utc::now().timestamp_millis()
        );

        let obs = Observation::Span(SpanData {
            id: span_id.clone(),
            trace_id: execution_id.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            name: step_name.to_string(),
            start_time: Utc::now(),
            end_time: None,
            input,
            output: None,
            metadata: serde_json::Map::new(),
            status: ObservationStatus::Running,
            level: ObservationLevel::Info,
            service_name: None,
        });

        if let Err(e) = self.service.ingest_observation(obs).await {
            warn!("Failed to record devtools step: {}", e);
        }

        span_id
    }

    /// Complete a step span.
    pub async fn complete_step(
        &self,
        span_id: &str,
        output: Option<Value>,
        status: ObservationStatus,
    ) {
        let update = devtools_core::ObservationUpdate {
            status: Some(status),
            end_time: Some(Utc::now()),
            output,
            ..Default::default()
        };

        if let Err(e) = self.service.update_observation(span_id, update).await {
            warn!("Failed to complete devtools step: {}", e);
        }
    }

    /// Record an LLM call as a Generation.
    pub async fn record_generation(
        &self,
        execution_id: &str,
        parent_id: Option<&str>,
        model: &str,
        input: Option<Value>,
        output: Option<Value>,
        input_tokens: i32,
        output_tokens: i32,
        cost_usd: Option<f64>,
    ) {
        let gen_id = format!("{}-gen-{}", execution_id, Utc::now().timestamp_millis());
        let total_tokens = input_tokens + output_tokens;

        let obs = Observation::Generation(GenerationData {
            id: gen_id,
            trace_id: execution_id.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            name: "llm-call".to_string(),
            model: model.to_string(),
            start_time: Utc::now(),
            end_time: Some(Utc::now()),
            input,
            output,
            input_tokens,
            output_tokens,
            total_tokens,
            cost_usd,
            metadata: serde_json::Map::new(),
            status: ObservationStatus::Completed,
            service_name: None,
        });

        if let Err(e) = self.service.ingest_observation(obs).await {
            warn!("Failed to record devtools generation: {}", e);
        }
    }

    /// Record a tool call as an Event.
    pub async fn record_tool_event(
        &self,
        execution_id: &str,
        parent_id: Option<&str>,
        tool_name: &str,
        input: Option<Value>,
        output: Option<Value>,
    ) {
        let event_id = format!("{}-evt-{}", execution_id, Utc::now().timestamp_millis());

        let obs = Observation::Event(EventData {
            id: event_id,
            trace_id: execution_id.to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
            name: format!("tool.{}", tool_name),
            time: Utc::now(),
            input,
            output,
            metadata: serde_json::Map::new(),
            level: ObservationLevel::Info,
            service_name: None,
        });

        if let Err(e) = self.service.ingest_observation(obs).await {
            warn!("Failed to record devtools tool event: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devtools_core::store::memory::{InMemoryEventBus, InMemoryTraceStore};

    fn test_bridge() -> DevtoolsBridge {
        let store = Arc::new(InMemoryTraceStore::new(100));
        let bus = Arc::new(InMemoryEventBus::new());
        let service = Arc::new(DevtoolsService::new(store, bus));
        DevtoolsBridge::new(service, "test-project")
    }

    #[tokio::test]
    async fn test_bridge_records_agent_loop_steps() {
        let bridge = test_bridge();

        // Start trace
        bridge
            .start_trace(
                "session-1",
                "exec-1",
                Some("test-query"),
                serde_json::json!({"query": "hello"}),
            )
            .await;

        // Record step
        let span_id = bridge
            .record_step(
                "exec-1",
                "ANALYZE",
                Some(serde_json::json!({"intent": "greet"})),
                None,
            )
            .await;

        // Record generation
        bridge
            .record_generation(
                "exec-1",
                Some(&span_id),
                "claude-sonnet",
                Some(serde_json::json!({"messages": []})),
                Some(serde_json::json!({"response": "hi"})),
                100,
                50,
                Some(0.001),
            )
            .await;

        // Record tool event
        bridge
            .record_tool_event(
                "exec-1",
                Some(&span_id),
                "browser_click",
                Some(serde_json::json!({"selector": "#btn"})),
                Some(serde_json::json!({"success": true})),
            )
            .await;

        // Complete step
        bridge
            .complete_step(
                &span_id,
                Some(serde_json::json!({"result": "analyzed"})),
                ObservationStatus::Completed,
            )
            .await;

        // End trace
        bridge
            .end_trace(
                "exec-1",
                serde_json::json!({"result": "done"}),
                TraceStatus::Completed,
            )
            .await;

        // Verify
        let trace = bridge.service.get_trace("exec-1").await.unwrap().unwrap();
        assert_eq!(trace.status, TraceStatus::Completed);
        assert!(trace.end_time.is_some());

        let tree = bridge.service.get_trace_tree("exec-1").await.unwrap();
        assert!(tree.observations.len() >= 3); // span + generation + event
    }

    #[tokio::test]
    async fn test_bridge_handles_errors_gracefully() {
        let bridge = test_bridge();

        // Ending a non-existent trace should not panic
        bridge
            .end_trace("nonexistent", serde_json::json!({}), TraceStatus::Error)
            .await;

        // Completing a non-existent step should not panic
        bridge
            .complete_step("nonexistent", None, ObservationStatus::Error)
            .await;
    }
}
