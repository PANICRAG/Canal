//! DevTools observer that bridges GraphObserver events to DevtoolsBridge.
//!
//! Translates graph lifecycle events (node enter/exit, graph start/complete)
//! into devtools traces and spans for the LLM observability system.
//!
//! # Feature Gate
//!
//! This module is behind `#[cfg(feature = "devtools")]`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::error::{GraphError, NodeId};
use super::observer::GraphObserver;
use super::GraphState;
use crate::agent::devtools_bridge::DevtoolsBridge;

/// A `GraphObserver` that forwards events to `DevtoolsBridge` for observability.
///
/// Creates a devtools trace on graph start, spans on node enter/exit,
/// and completes the trace on graph end.
pub struct DevtoolsObserver {
    bridge: Arc<DevtoolsBridge>,
    session_id: String,
    /// Maps node_id → span_id for correlating enter/exit events.
    span_id_map: Arc<Mutex<HashMap<String, String>>>,
}

impl DevtoolsObserver {
    /// Create a new observer for a specific session.
    pub fn new(bridge: Arc<DevtoolsBridge>, session_id: impl Into<String>) -> Self {
        Self {
            bridge,
            session_id: session_id.into(),
            span_id_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl<S: GraphState> GraphObserver<S> for DevtoolsObserver {
    async fn on_graph_start(&self, exec_id: &str, _state: &S) {
        self.bridge
            .start_trace(
                &self.session_id,
                exec_id,
                Some("graph-execution"),
                serde_json::json!({"session_id": self.session_id}),
            )
            .await;
    }

    async fn on_node_enter(&self, exec_id: &str, node_id: &NodeId, _state: &S) {
        let span_id = self
            .bridge
            .record_step(
                exec_id,
                &format!("node.{}", node_id),
                Some(serde_json::json!({"node_id": node_id.to_string()})),
                None,
            )
            .await;

        let mut map = self.span_id_map.lock().await;
        map.insert(node_id.to_string(), span_id);
    }

    async fn on_node_exit(
        &self,
        _exec_id: &str,
        node_id: &NodeId,
        _state: &S,
        _duration: Duration,
    ) {
        let span_id = {
            let map = self.span_id_map.lock().await;
            map.get(&node_id.to_string()).cloned()
        };

        if let Some(span_id) = span_id {
            self.bridge
                .complete_step(
                    &span_id,
                    Some(serde_json::json!({"status": "completed"})),
                    devtools_core::ObservationStatus::Completed,
                )
                .await;
        }
    }

    async fn on_node_error(&self, exec_id: &str, node_id: &NodeId, error: &GraphError) {
        // Complete the span with error if it exists
        let span_id = {
            let map = self.span_id_map.lock().await;
            map.get(&node_id.to_string()).cloned()
        };

        if let Some(span_id) = span_id {
            self.bridge
                .complete_step(
                    &span_id,
                    Some(serde_json::json!({"error": error.to_string()})),
                    devtools_core::ObservationStatus::Error,
                )
                .await;
        }

        // End the trace with error status
        self.bridge
            .end_trace(
                exec_id,
                serde_json::json!({"error": error.to_string(), "node_id": node_id.to_string()}),
                devtools_core::TraceStatus::Error,
            )
            .await;
    }

    async fn on_graph_complete(&self, exec_id: &str, _state: &S, _total_duration: Duration) {
        self.bridge
            .end_trace(
                exec_id,
                serde_json::json!({"status": "completed"}),
                devtools_core::TraceStatus::Completed,
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devtools_core::store::memory::{InMemoryEventBus, InMemoryTraceStore};
    use devtools_core::DevtoolsService;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct TestState {
        value: i32,
    }

    impl super::super::GraphState for TestState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
        }
    }

    fn make_observer() -> (DevtoolsObserver, Arc<DevtoolsService>) {
        let store = Arc::new(InMemoryTraceStore::new(100));
        let bus = Arc::new(InMemoryEventBus::new());
        let service = Arc::new(DevtoolsService::new(store, bus));
        let bridge = Arc::new(DevtoolsBridge::new(service.clone(), "test-project"));
        let observer = DevtoolsObserver::new(bridge, "test-session");
        (observer, service)
    }

    #[tokio::test]
    async fn test_graph_start_creates_trace() {
        let (observer, service) = make_observer();
        let state = TestState { value: 1 };

        let obs: &dyn GraphObserver<TestState> = &observer;
        obs.on_graph_start("exec-1", &state).await;

        let trace = service.get_trace("exec-1").await.unwrap();
        assert!(trace.is_some());
        let trace = trace.unwrap();
        assert_eq!(trace.id, "exec-1");
        assert_eq!(trace.session_id, Some("test-session".into()));
        assert_eq!(trace.status, devtools_core::TraceStatus::Running);
    }

    #[tokio::test]
    async fn test_node_enter_creates_span() {
        let (observer, service) = make_observer();
        let state = TestState { value: 1 };

        let obs: &dyn GraphObserver<TestState> = &observer;
        obs.on_graph_start("exec-1", &state).await;
        obs.on_node_enter("exec-1", &"classify".to_string(), &state)
            .await;

        let tree = service.get_trace_tree("exec-1").await.unwrap();
        assert!(!tree.observations.is_empty());
        // Verify the span name includes the node id
        let span = &tree.observations[0];
        assert!(span.id().contains("exec-1"));
    }

    #[tokio::test]
    async fn test_node_exit_completes_span() {
        let (observer, service) = make_observer();
        let state = TestState { value: 1 };

        let obs: &dyn GraphObserver<TestState> = &observer;
        obs.on_graph_start("exec-1", &state).await;
        obs.on_node_enter("exec-1", &"classify".to_string(), &state)
            .await;
        obs.on_node_exit(
            "exec-1",
            &"classify".to_string(),
            &state,
            Duration::from_millis(500),
        )
        .await;

        let tree = service.get_trace_tree("exec-1").await.unwrap();
        assert!(!tree.observations.is_empty());
    }

    #[tokio::test]
    async fn test_graph_complete_ends_trace() {
        let (observer, service) = make_observer();
        let state = TestState { value: 1 };

        let obs: &dyn GraphObserver<TestState> = &observer;
        obs.on_graph_start("exec-1", &state).await;
        obs.on_graph_complete("exec-1", &state, Duration::from_secs(1))
            .await;

        let trace = service.get_trace("exec-1").await.unwrap().unwrap();
        assert_eq!(trace.status, devtools_core::TraceStatus::Completed);
        assert!(trace.end_time.is_some());
    }

    #[tokio::test]
    async fn test_node_error_ends_trace_with_error() {
        let (observer, service) = make_observer();
        let state = TestState { value: 1 };
        let error = GraphError::NodeTimeout {
            node_id: "execute".to_string(),
            timeout: Duration::from_secs(30),
        };

        let obs: &dyn GraphObserver<TestState> = &observer;
        obs.on_graph_start("exec-1", &state).await;
        obs.on_node_enter("exec-1", &"execute".to_string(), &state)
            .await;
        obs.on_node_error("exec-1", &"execute".to_string(), &error)
            .await;

        let trace = service.get_trace("exec-1").await.unwrap().unwrap();
        assert_eq!(trace.status, devtools_core::TraceStatus::Error);
    }

    #[tokio::test]
    async fn test_full_graph_lifecycle() {
        let (observer, service) = make_observer();
        let state = TestState { value: 1 };

        let obs: &dyn GraphObserver<TestState> = &observer;

        // Full lifecycle
        obs.on_graph_start("exec-1", &state).await;
        obs.on_node_enter("exec-1", &"classify".to_string(), &state)
            .await;
        obs.on_node_exit(
            "exec-1",
            &"classify".to_string(),
            &state,
            Duration::from_millis(400),
        )
        .await;
        obs.on_node_enter("exec-1", &"execute".to_string(), &state)
            .await;
        obs.on_node_exit(
            "exec-1",
            &"execute".to_string(),
            &state,
            Duration::from_millis(2100),
        )
        .await;
        obs.on_graph_complete("exec-1", &state, Duration::from_millis(2500))
            .await;

        // Verify trace
        let trace = service.get_trace("exec-1").await.unwrap().unwrap();
        assert_eq!(trace.status, devtools_core::TraceStatus::Completed);

        // Verify observations (2 spans)
        let tree = service.get_trace_tree("exec-1").await.unwrap();
        assert_eq!(tree.observations.len(), 2);
    }
}
