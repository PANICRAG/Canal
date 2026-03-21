//! Graph observer for execution tracing and metrics.
//!
//! Observers receive callbacks at each graph lifecycle event,
//! enabling tracing, metrics collection, and custom monitoring.

use std::time::Duration;

use async_trait::async_trait;

use super::error::{GraphError, NodeId};
use super::GraphState;

/// Observer trait for receiving graph execution events.
///
/// All methods have default no-op implementations, so observers
/// can selectively override only the events they care about.
#[async_trait]
pub trait GraphObserver<S: GraphState>: Send + Sync {
    /// Called when graph execution starts.
    async fn on_graph_start(&self, _graph_execution_id: &str, _state: &S) {}

    /// Called when a node is about to execute.
    async fn on_node_enter(&self, _graph_execution_id: &str, _node_id: &NodeId, _state: &S) {}

    /// Called after a node has completed successfully.
    async fn on_node_exit(
        &self,
        _graph_execution_id: &str,
        _node_id: &NodeId,
        _state: &S,
        _duration: Duration,
    ) {
    }

    /// Called when a node execution fails.
    async fn on_node_error(
        &self,
        _graph_execution_id: &str,
        _node_id: &NodeId,
        _error: &GraphError,
    ) {
    }

    /// Called when an edge is traversed.
    async fn on_edge_traverse(
        &self,
        _graph_execution_id: &str,
        _from: &NodeId,
        _to: &NodeId,
        _label: &str,
    ) {
    }

    /// Called when graph execution completes.
    async fn on_graph_complete(
        &self,
        _graph_execution_id: &str,
        _state: &S,
        _total_duration: Duration,
    ) {
    }

    /// Called when a checkpoint is saved.
    async fn on_checkpoint(
        &self,
        _graph_execution_id: &str,
        _node_id: &NodeId,
        _checkpoint_id: &str,
    ) {
    }

    // ── A23 parallel events ──

    /// Called when a parallel node completes with partial results (ContinueOnError).
    async fn on_parallel_partial(
        &self,
        _exec_id: &str,
        _node_id: &NodeId,
        _succeeded: usize,
        _failed: usize,
    ) {
    }

    /// Called when a parallel branch fails (ContinueOnError or RetryThenFail).
    async fn on_parallel_branch_failed(
        &self,
        _exec_id: &str,
        _node_id: &NodeId,
        _branch_id: &NodeId,
        _error: &str,
    ) {
    }

    // ── A23 DAG events ──

    /// Called when a DAG execution wave starts.
    async fn on_dag_wave_start(&self, _exec_id: &str, _wave_index: usize, _node_ids: &[NodeId]) {}

    /// Called when a DAG execution wave completes.
    async fn on_dag_wave_complete(&self, _exec_id: &str, _wave_index: usize, _duration: Duration) {}

    // ── A23 memory events ──

    /// Called after memory is hydrated from UnifiedMemoryStore.
    async fn on_memory_hydrated(&self, _exec_id: &str, _entries_loaded: usize) {}

    /// Called after execution results are flushed to UnifiedMemoryStore.
    async fn on_memory_flushed(&self, _exec_id: &str, _entries_persisted: usize) {}

    // ── A23 template/mode events ──

    /// Called when a template is selected for execution.
    async fn on_template_selected(&self, _exec_id: &str, _template_id: &str, _reason: &str) {}

    /// Called when auto-mode falls back to a different mode.
    async fn on_auto_mode_fallback(&self, _exec_id: &str, _from: &str, _to: &str, _reason: &str) {}

    // ── A23 budget events ──

    /// Called when a node's token consumption approaches the budget limit.
    async fn on_budget_warning(&self, _exec_id: &str, _node_id: &NodeId) {}

    /// Called when a node's token consumption exceeds the budget limit.
    async fn on_budget_exceeded(&self, _exec_id: &str, _node_id: &NodeId) {}
}

/// A tracing-based observer that emits structured spans.
pub struct TracingObserver;

impl TracingObserver {
    /// Create a new tracing observer.
    pub fn new() -> Self {
        Self
    }
}

impl Default for TracingObserver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<S: GraphState> GraphObserver<S> for TracingObserver {
    async fn on_graph_start(&self, graph_execution_id: &str, _state: &S) {
        tracing::info!(
            graph_execution_id = %graph_execution_id,
            "graph execution started"
        );
    }

    async fn on_node_enter(&self, graph_execution_id: &str, node_id: &NodeId, _state: &S) {
        tracing::info!(
            graph_execution_id = %graph_execution_id,
            node_id = %node_id,
            "entering node"
        );
    }

    async fn on_node_exit(
        &self,
        graph_execution_id: &str,
        node_id: &NodeId,
        _state: &S,
        duration: Duration,
    ) {
        tracing::info!(
            graph_execution_id = %graph_execution_id,
            node_id = %node_id,
            duration_ms = duration.as_millis() as u64,
            "node completed"
        );
    }

    async fn on_node_error(&self, graph_execution_id: &str, node_id: &NodeId, error: &GraphError) {
        tracing::error!(
            graph_execution_id = %graph_execution_id,
            node_id = %node_id,
            error = %error,
            "node execution failed"
        );
    }

    async fn on_edge_traverse(
        &self,
        graph_execution_id: &str,
        from: &NodeId,
        to: &NodeId,
        label: &str,
    ) {
        tracing::debug!(
            graph_execution_id = %graph_execution_id,
            from = %from,
            to = %to,
            label = %label,
            "edge traversed"
        );
    }

    async fn on_graph_complete(
        &self,
        graph_execution_id: &str,
        _state: &S,
        total_duration: Duration,
    ) {
        tracing::info!(
            graph_execution_id = %graph_execution_id,
            total_duration_ms = total_duration.as_millis() as u64,
            "graph execution completed"
        );
    }

    async fn on_checkpoint(&self, graph_execution_id: &str, node_id: &NodeId, checkpoint_id: &str) {
        tracing::debug!(
            graph_execution_id = %graph_execution_id,
            node_id = %node_id,
            checkpoint_id = %checkpoint_id,
            "checkpoint saved"
        );
    }
}

/// A no-op observer that discards all events. Used as default.
pub struct NoOpObserver;

#[async_trait]
impl<S: GraphState> GraphObserver<S> for NoOpObserver {}

/// Composite observer that dispatches to multiple observers.
pub struct CompositeObserver<S: GraphState> {
    observers: Vec<Box<dyn GraphObserver<S>>>,
}

impl<S: GraphState> CompositeObserver<S> {
    /// Create a new composite observer.
    pub fn new() -> Self {
        Self {
            observers: Vec::new(),
        }
    }

    /// Add an observer.
    pub fn add(mut self, observer: impl GraphObserver<S> + 'static) -> Self {
        self.observers.push(Box::new(observer));
        self
    }
}

impl<S: GraphState> Default for CompositeObserver<S> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<S: GraphState> GraphObserver<S> for CompositeObserver<S> {
    async fn on_graph_start(&self, id: &str, state: &S) {
        for obs in &self.observers {
            obs.on_graph_start(id, state).await;
        }
    }

    async fn on_node_enter(&self, id: &str, node_id: &NodeId, state: &S) {
        for obs in &self.observers {
            obs.on_node_enter(id, node_id, state).await;
        }
    }

    async fn on_node_exit(&self, id: &str, node_id: &NodeId, state: &S, duration: Duration) {
        for obs in &self.observers {
            obs.on_node_exit(id, node_id, state, duration).await;
        }
    }

    async fn on_node_error(&self, id: &str, node_id: &NodeId, error: &GraphError) {
        for obs in &self.observers {
            obs.on_node_error(id, node_id, error).await;
        }
    }

    async fn on_edge_traverse(&self, id: &str, from: &NodeId, to: &NodeId, label: &str) {
        for obs in &self.observers {
            obs.on_edge_traverse(id, from, to, label).await;
        }
    }

    async fn on_graph_complete(&self, id: &str, state: &S, total: Duration) {
        for obs in &self.observers {
            obs.on_graph_complete(id, state, total).await;
        }
    }

    async fn on_checkpoint(&self, id: &str, node_id: &NodeId, cp_id: &str) {
        for obs in &self.observers {
            obs.on_checkpoint(id, node_id, cp_id).await;
        }
    }

    async fn on_parallel_partial(&self, id: &str, node_id: &NodeId, ok: usize, fail: usize) {
        for obs in &self.observers {
            obs.on_parallel_partial(id, node_id, ok, fail).await;
        }
    }

    async fn on_parallel_branch_failed(
        &self,
        id: &str,
        node_id: &NodeId,
        branch_id: &NodeId,
        err: &str,
    ) {
        for obs in &self.observers {
            obs.on_parallel_branch_failed(id, node_id, branch_id, err)
                .await;
        }
    }

    async fn on_dag_wave_start(&self, id: &str, wave_index: usize, node_ids: &[NodeId]) {
        for obs in &self.observers {
            obs.on_dag_wave_start(id, wave_index, node_ids).await;
        }
    }

    async fn on_dag_wave_complete(&self, id: &str, wave_index: usize, duration: Duration) {
        for obs in &self.observers {
            obs.on_dag_wave_complete(id, wave_index, duration).await;
        }
    }

    async fn on_memory_hydrated(&self, id: &str, entries: usize) {
        for obs in &self.observers {
            obs.on_memory_hydrated(id, entries).await;
        }
    }

    async fn on_memory_flushed(&self, id: &str, entries: usize) {
        for obs in &self.observers {
            obs.on_memory_flushed(id, entries).await;
        }
    }

    async fn on_template_selected(&self, id: &str, template_id: &str, reason: &str) {
        for obs in &self.observers {
            obs.on_template_selected(id, template_id, reason).await;
        }
    }

    async fn on_auto_mode_fallback(&self, id: &str, from: &str, to: &str, reason: &str) {
        for obs in &self.observers {
            obs.on_auto_mode_fallback(id, from, to, reason).await;
        }
    }

    async fn on_budget_warning(&self, id: &str, node_id: &NodeId) {
        for obs in &self.observers {
            obs.on_budget_warning(id, node_id).await;
        }
    }

    async fn on_budget_exceeded(&self, id: &str, node_id: &NodeId) {
        for obs in &self.observers {
            obs.on_budget_exceeded(id, node_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::sync::atomic::{AtomicU32, Ordering};

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct TestState {
        value: i32,
    }

    impl GraphState for TestState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
        }
    }

    struct CountingObserver {
        count: AtomicU32,
    }

    impl CountingObserver {
        fn new() -> Self {
            Self {
                count: AtomicU32::new(0),
            }
        }

        fn count(&self) -> u32 {
            self.count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl GraphObserver<TestState> for CountingObserver {
        async fn on_node_enter(&self, _id: &str, _node_id: &NodeId, _state: &TestState) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn test_tracing_observer_no_panic() {
        let obs = TracingObserver::new();
        let state = TestState { value: 1 };
        obs.on_graph_start("exec_1", &state).await;
        obs.on_node_enter("exec_1", &"n1".into(), &state).await;
        obs.on_node_exit("exec_1", &"n1".into(), &state, Duration::from_millis(100))
            .await;
        obs.on_graph_complete("exec_1", &state, Duration::from_secs(1))
            .await;
    }

    #[tokio::test]
    async fn test_noop_observer() {
        let obs = NoOpObserver;
        let state = TestState { value: 1 };
        // All methods should be no-ops
        obs.on_graph_start("x", &state).await;
        obs.on_node_enter("x", &"n".into(), &state).await;
    }

    #[tokio::test]
    async fn test_composite_observer() {
        let counter = std::sync::Arc::new(CountingObserver::new());
        // We need to use the counter through a wrapper because CompositeObserver takes ownership
        let counter_clone = counter.clone();

        struct SharedCounter(std::sync::Arc<CountingObserver>);

        #[async_trait]
        impl GraphObserver<TestState> for SharedCounter {
            async fn on_node_enter(&self, id: &str, node_id: &NodeId, state: &TestState) {
                self.0.on_node_enter(id, node_id, state).await;
            }
        }

        let composite = CompositeObserver::new()
            .add(SharedCounter(counter_clone.clone()))
            .add(SharedCounter(counter_clone.clone()));

        let state = TestState { value: 1 };
        composite.on_node_enter("exec", &"n1".into(), &state).await;

        // Each observer should have been called
        assert_eq!(counter.count(), 2);
    }
}
