//! Recording observer that bridges GraphObserver events to ExecutionStore.
//!
//! Implements all GraphObserver methods (including A23 extensions) and
//! writes each event to the ExecutionStore for persistence and SSE streaming.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use super::error::{GraphError, NodeId};
use super::execution_store::{EventPayload, ExecutionStore};
use super::observer::GraphObserver;
use super::GraphState;

/// A `GraphObserver` that records all events to an `ExecutionStore`.
pub struct RecordingObserver {
    store: Arc<ExecutionStore>,
    execution_id: String,
}

impl RecordingObserver {
    /// Create a new recording observer for a specific execution.
    pub fn new(store: Arc<ExecutionStore>, execution_id: impl Into<String>) -> Self {
        Self {
            store,
            execution_id: execution_id.into(),
        }
    }
}

#[async_trait]
impl<S: GraphState> GraphObserver<S> for RecordingObserver {
    async fn on_graph_start(&self, _graph_execution_id: &str, _state: &S) {
        self.store
            .append_event(&self.execution_id, EventPayload::GraphStarted)
            .await;
    }

    async fn on_node_enter(&self, _graph_execution_id: &str, node_id: &NodeId, _state: &S) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::NodeEntered {
                    node_id: node_id.clone(),
                },
            )
            .await;
    }

    async fn on_node_exit(
        &self,
        _graph_execution_id: &str,
        node_id: &NodeId,
        _state: &S,
        duration: Duration,
    ) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::NodeCompleted {
                    node_id: node_id.clone(),
                    duration_ms: duration.as_millis() as u64,
                },
            )
            .await;
    }

    async fn on_node_error(&self, _graph_execution_id: &str, node_id: &NodeId, error: &GraphError) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::NodeFailed {
                    node_id: node_id.clone(),
                    error: error.to_string(),
                },
            )
            .await;
    }

    async fn on_edge_traverse(
        &self,
        _graph_execution_id: &str,
        from: &NodeId,
        to: &NodeId,
        label: &str,
    ) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::EdgeTraversed {
                    from: from.clone(),
                    to: to.clone(),
                    label: label.to_string(),
                },
            )
            .await;
    }

    async fn on_graph_complete(
        &self,
        _graph_execution_id: &str,
        _state: &S,
        total_duration: Duration,
    ) {
        self.store
            .complete_execution(&self.execution_id, total_duration.as_millis() as u64)
            .await;
    }

    async fn on_checkpoint(
        &self,
        _graph_execution_id: &str,
        node_id: &NodeId,
        checkpoint_id: &str,
    ) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::CheckpointSaved {
                    node_id: node_id.clone(),
                    checkpoint_id: checkpoint_id.to_string(),
                },
            )
            .await;
    }

    // ── A23 events ──

    async fn on_parallel_partial(
        &self,
        _exec_id: &str,
        node_id: &NodeId,
        succeeded: usize,
        failed: usize,
    ) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::ParallelPartialComplete {
                    node_id: node_id.clone(),
                    succeeded,
                    failed,
                },
            )
            .await;
    }

    async fn on_parallel_branch_failed(
        &self,
        _exec_id: &str,
        node_id: &NodeId,
        branch_id: &NodeId,
        error: &str,
    ) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::ParallelBranchFailed {
                    node_id: node_id.clone(),
                    branch_id: branch_id.clone(),
                    error: error.to_string(),
                },
            )
            .await;
    }

    async fn on_dag_wave_start(&self, _exec_id: &str, wave_index: usize, node_ids: &[NodeId]) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::DagWaveStarted {
                    wave_index,
                    node_ids: node_ids.to_vec(),
                },
            )
            .await;
    }

    async fn on_dag_wave_complete(&self, _exec_id: &str, wave_index: usize, duration: Duration) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::DagWaveCompleted {
                    wave_index,
                    duration_ms: duration.as_millis() as u64,
                },
            )
            .await;
    }

    async fn on_memory_hydrated(&self, _exec_id: &str, entries_loaded: usize) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::MemoryHydrated { entries_loaded },
            )
            .await;
    }

    async fn on_memory_flushed(&self, _exec_id: &str, entries_persisted: usize) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::MemoryFlushed { entries_persisted },
            )
            .await;
    }

    async fn on_template_selected(&self, _exec_id: &str, template_id: &str, reason: &str) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::TemplateSelected {
                    template_id: template_id.to_string(),
                    reason: reason.to_string(),
                },
            )
            .await;
    }

    async fn on_auto_mode_fallback(&self, _exec_id: &str, from: &str, to: &str, reason: &str) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::AutoModeFallback {
                    from_mode: from.to_string(),
                    to_mode: to.to_string(),
                    reason: reason.to_string(),
                },
            )
            .await;
    }

    async fn on_budget_warning(&self, _exec_id: &str, node_id: &NodeId) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::BudgetWarning {
                    node_id: node_id.clone(),
                    consumed: 0,
                    limit: 0,
                    scope: "unknown".to_string(),
                },
            )
            .await;
    }

    async fn on_budget_exceeded(&self, _exec_id: &str, node_id: &NodeId) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::BudgetExceeded {
                    node_id: node_id.clone(),
                    consumed: 0,
                    limit: 0,
                    scope: "unknown".to_string(),
                },
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[tokio::test]
    async fn test_recording_observer_captures_events() {
        let store = Arc::new(ExecutionStore::new(10));
        store
            .start_execution(
                "exec_1",
                super::super::execution_store::ExecutionMode::Direct,
            )
            .await;

        let observer = RecordingObserver::new(store.clone(), "exec_1");
        let state = TestState { value: 42 };

        let obs: &dyn GraphObserver<TestState> = &observer;
        obs.on_graph_start("exec_1", &state).await;
        obs.on_node_enter("exec_1", &"node_a".to_string(), &state)
            .await;
        obs.on_node_exit(
            "exec_1",
            &"node_a".to_string(),
            &state,
            Duration::from_millis(100),
        )
        .await;

        let events = store.get_events("exec_1", 0, None);
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0].payload, EventPayload::GraphStarted));
        assert!(matches!(
            events[1].payload,
            EventPayload::NodeEntered { .. }
        ));
        assert!(matches!(
            events[2].payload,
            EventPayload::NodeCompleted { .. }
        ));
    }

    #[tokio::test]
    async fn test_recording_observer_a23_events() {
        let store = Arc::new(ExecutionStore::new(10));
        store
            .start_execution("exec_1", super::super::execution_store::ExecutionMode::Dag)
            .await;

        let observer = RecordingObserver::new(store.clone(), "exec_1");
        let state = TestState { value: 1 };

        let obs: &dyn GraphObserver<TestState> = &observer;
        obs.on_dag_wave_start("exec_1", 0, &["a".to_string(), "b".to_string()])
            .await;
        obs.on_dag_wave_complete("exec_1", 0, Duration::from_millis(200))
            .await;
        obs.on_parallel_partial("exec_1", &"par".to_string(), 2, 1)
            .await;
        obs.on_budget_warning("exec_1", &"node_x".to_string()).await;

        let events = store.get_events("exec_1", 0, None);
        assert_eq!(events.len(), 4);
        assert!(matches!(
            events[0].payload,
            EventPayload::DagWaveStarted { .. }
        ));
        assert!(matches!(
            events[1].payload,
            EventPayload::DagWaveCompleted { .. }
        ));
        assert!(matches!(
            events[2].payload,
            EventPayload::ParallelPartialComplete { .. }
        ));
        assert!(matches!(
            events[3].payload,
            EventPayload::BudgetWarning { .. }
        ));
    }
}
