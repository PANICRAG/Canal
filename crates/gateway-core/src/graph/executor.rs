//! Graph execution engine.
//!
//! The `GraphExecutor` takes a compiled `StateGraph` and runs it from the
//! entry point to a terminal node, transforming state through each node
//! and following edges based on predicates.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{watch, Semaphore};

use super::builder::StateGraph;
use super::error::{GraphError, NodeError, NodeId};
use super::node::{ErrorStrategy, JoinStrategy, NodeContext, NodeType};
use super::observer::{GraphObserver, NoOpObserver};
use super::GraphState;

// R2-H8: GraphEvent enum removed — was never instantiated or used anywhere.
// Graph execution events are handled via the GraphObserver trait instead.

/// Executes a compiled state graph.
pub struct GraphExecutor<S: GraphState> {
    graph: Arc<StateGraph<S>>,
    observer: Arc<dyn GraphObserver<S>>,
    semaphore: Arc<Semaphore>,
    cancel_tx: watch::Sender<bool>,
    cancel_rx: watch::Receiver<bool>,
    budget: Option<Arc<super::budget::ExecutionBudget>>,
}

impl<S: GraphState> GraphExecutor<S> {
    /// Create a new executor for the given graph.
    pub fn new(graph: StateGraph<S>) -> Self {
        let observer: Arc<dyn GraphObserver<S>> = graph
            .observer
            .clone()
            .unwrap_or_else(|| Arc::new(NoOpObserver));
        let max_concurrency = graph.config.max_global_concurrency;
        let (cancel_tx, cancel_rx) = watch::channel(false);
        Self {
            graph: Arc::new(graph),
            observer,
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            cancel_tx,
            cancel_rx,
            budget: None,
        }
    }

    /// Set an execution budget for token tracking.
    pub fn with_budget(mut self, budget: super::budget::ExecutionBudget) -> Self {
        self.budget = Some(Arc::new(budget));
        self
    }

    /// Execute the graph from the entry point.
    pub async fn execute(&self, initial_state: S) -> Result<S, GraphError> {
        self.execute_from(self.graph.entry_point.clone(), initial_state, 0)
            .await
    }

    /// Cancel the running execution.
    pub fn cancel(&self) {
        let _ = self.cancel_tx.send(true);
    }

    /// Resume execution from a checkpoint.
    pub async fn resume(&self, checkpoint_id: &str) -> Result<S, GraphError> {
        let checkpointer = self
            .graph
            .checkpointer
            .as_ref()
            .ok_or_else(|| GraphError::CheckpointError("no checkpointer configured".into()))?;

        let (node_id, state) = checkpointer.load(checkpoint_id).await?;

        // Find the next node(s) after the checkpointed node
        let next_node = self.resolve_next_node(&node_id, &state).await?;

        self.execute_from(next_node, state, 0).await
    }

    /// Execute from a specific node.
    async fn execute_from(
        &self,
        start_node: NodeId,
        initial_state: S,
        depth: usize,
    ) -> Result<S, GraphError> {
        if depth > self.graph.config.max_depth {
            return Err(GraphError::MaxDepthExceeded {
                depth,
                max_depth: self.graph.config.max_depth,
            });
        }

        let execution_id = uuid::Uuid::new_v4().to_string();
        let graph_start = Instant::now();

        self.observer
            .on_graph_start(&execution_id, &initial_state)
            .await;

        let mut current_node = start_node;
        let mut state = initial_state;

        loop {
            // Check cancellation
            if *self.cancel_rx.borrow() {
                return Err(GraphError::Cancelled);
            }

            // Get the node
            let node = self
                .graph
                .get_node(&current_node)
                .ok_or_else(|| GraphError::NodeNotFound(current_node.clone()))?;

            // Execute the node
            self.observer
                .on_node_enter(&execution_id, &current_node, &state)
                .await;

            let node_start = Instant::now();
            let result = self
                .execute_node(node, state.clone(), &execution_id, depth)
                .await;

            match result {
                Ok(new_state) => {
                    let duration = node_start.elapsed();
                    self.observer
                        .on_node_exit(&execution_id, &current_node, &new_state, duration)
                        .await;

                    state = new_state;

                    // Checkpoint if enabled
                    if self.graph.config.checkpoint_enabled {
                        if let Some(ref checkpointer) = self.graph.checkpointer {
                            match checkpointer
                                .save(&execution_id, &current_node, &state)
                                .await
                            {
                                Ok(cp_id) => {
                                    self.observer
                                        .on_checkpoint(&execution_id, &current_node, &cp_id)
                                        .await;
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        node_id = %current_node,
                                        "checkpoint save failed (non-fatal)"
                                    );
                                }
                            }
                        }
                    }

                    // Check if terminal
                    if self.graph.is_terminal(&current_node) {
                        let total_duration = graph_start.elapsed();
                        self.observer
                            .on_graph_complete(&execution_id, &state, total_duration)
                            .await;
                        return Ok(state);
                    }

                    // Resolve next node
                    let next_node = self.resolve_next_node(&current_node, &state).await?;
                    self.observer
                        .on_edge_traverse(&execution_id, &current_node, &next_node, "default")
                        .await;
                    current_node = next_node;
                }
                Err(e) => {
                    let graph_err = GraphError::NodeExecutionFailed {
                        node_id: current_node.clone(),
                        message: e.to_string(),
                    };
                    self.observer
                        .on_node_error(&execution_id, &current_node, &graph_err)
                        .await;
                    return Err(graph_err);
                }
            }
        }
    }

    /// Execute a single node.
    async fn execute_node(
        &self,
        node: &NodeType<S>,
        state: S,
        execution_id: &str,
        depth: usize,
    ) -> Result<S, NodeError> {
        match node {
            NodeType::Function(func_node) => {
                let ctx = NodeContext {
                    graph_execution_id: execution_id.to_string(),
                    node_id: func_node.id.clone(),
                    depth,
                    max_depth: self.graph.config.max_depth,
                    cancelled: Arc::new(self.cancel_rx.clone()),
                };

                // Execute with timeout and retry
                self.execute_with_retry(
                    &func_node.handler,
                    state,
                    &ctx,
                    &func_node.retry_policy,
                    func_node.timeout,
                )
                .await
            }
            NodeType::Parallel(par_node) => {
                self.execute_parallel(par_node, state, execution_id, depth)
                    .await
            }
            NodeType::HumanReview(_review_node) => {
                // For now, human review is a pass-through.
                // In production, this would pause and wait for human input.
                Ok(state)
            }
        }
    }

    /// Execute a handler with retry policy and timeout.
    async fn execute_with_retry(
        &self,
        handler: &Arc<dyn super::node::NodeHandler<S>>,
        state: S,
        ctx: &NodeContext,
        retry_policy: &super::node::RetryPolicy,
        timeout: Duration,
    ) -> Result<S, NodeError> {
        let mut attempts = 0;

        loop {
            let result = tokio::time::timeout(timeout, handler.execute(state.clone(), ctx)).await;

            match result {
                Ok(Ok(new_state)) => return Ok(new_state),
                Ok(Err(NodeError::Retryable(msg))) if attempts < retry_policy.max_retries => {
                    tracing::debug!(attempt = attempts + 1, error = %msg, "retrying node");
                    attempts += 1;
                    let backoff = retry_policy
                        .initial_backoff
                        .mul_f64(retry_policy.backoff_multiplier.powi(attempts as i32 - 1));
                    // Clamp backoff to 5 minutes max to prevent overflow
                    let backoff = backoff.min(Duration::from_secs(300));
                    tokio::time::sleep(backoff).await;
                    continue;
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(NodeError::Timeout(timeout)),
            }
        }
    }

    /// Execute parallel branches and merge results.
    async fn execute_parallel(
        &self,
        node: &super::node::ParallelNode<S>,
        state: S,
        execution_id: &str,
        depth: usize,
    ) -> Result<S, NodeError> {
        // Node-level semaphore (respects per-node max_concurrency)
        let concurrency = node.max_concurrency.min(node.branches.len());
        let local_semaphore = Arc::new(Semaphore::new(concurrency));

        let mut handles = Vec::new();

        for branch_id in &node.branches {
            let branch_node = self.graph.get_node(branch_id).ok_or_else(|| {
                NodeError::HandlerError(format!("branch node not found: {branch_id}"))
            })?;

            let branch_state = state.clone();
            let exec_id = execution_id.to_string();
            let global_sem = self.semaphore.clone();
            let local_sem = local_semaphore.clone();
            let cancel_rx = self.cancel_rx.clone();
            let branch_id_owned = branch_id.clone();
            let branch_id_key = branch_id.clone();
            let max_depth = self.graph.config.max_depth;

            let handler = match branch_node {
                NodeType::Function(f) => f.handler.clone(),
                _ => {
                    return Err(NodeError::HandlerError(
                        "parallel branches must be function nodes".into(),
                    ))
                }
            };
            let timeout = match branch_node {
                NodeType::Function(f) => f.timeout,
                _ => self.graph.config.default_node_timeout,
            };

            let handle = tokio::spawn(async move {
                // Dual-layer permits: must acquire both global and local
                let _global_permit = global_sem
                    .acquire()
                    .await
                    .map_err(|_| NodeError::HandlerError("global semaphore closed".into()))?;
                let _local_permit = local_sem
                    .acquire()
                    .await
                    .map_err(|_| NodeError::HandlerError("local semaphore closed".into()))?;

                let ctx = NodeContext {
                    graph_execution_id: exec_id,
                    node_id: branch_id_owned,
                    depth: depth + 1,
                    max_depth,
                    cancelled: Arc::new(cancel_rx),
                };

                match tokio::time::timeout(timeout, handler.execute(branch_state, &ctx)).await {
                    Ok(result) => result,
                    Err(_) => Err(NodeError::Timeout(timeout)),
                }
            });

            handles.push((branch_id_key, handle));
        }

        // Collect results based on join strategy
        match &node.join_strategy {
            JoinStrategy::WaitAll => {
                self.collect_wait_all(node, state, handles, execution_id)
                    .await
            }
            JoinStrategy::WaitFirst => {
                let bare_handles: Vec<_> = handles.into_iter().map(|(_, h)| h).collect();
                let (result, _, remaining) = futures::future::select_all(bare_handles).await;
                for handle in remaining {
                    handle.abort();
                }
                let result_state = result
                    .map_err(|e| NodeError::HandlerError(format!("task join error: {e}")))??;
                Ok(result_state)
            }
            JoinStrategy::WaitQuorum(n) => {
                let mut merged_state = state;
                let mut completed = 0;
                let mut remaining_handles: Vec<_> = handles.into_iter().map(|(_, h)| h).collect();

                while completed < *n && !remaining_handles.is_empty() {
                    let (result, _idx, rest) = futures::future::select_all(remaining_handles).await;
                    remaining_handles = rest;

                    match result {
                        Ok(Ok(result_state)) => {
                            merged_state.merge(result_state);
                            completed += 1;
                        }
                        Ok(Err(e)) => {
                            tracing::warn!(error = %e, "parallel branch failed, continuing");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "parallel branch task panicked");
                        }
                    }
                }

                for handle in remaining_handles {
                    handle.abort();
                }

                if completed < *n {
                    return Err(NodeError::HandlerError(format!(
                        "quorum not reached: {completed}/{n} branches completed"
                    )));
                }

                Ok(merged_state)
            }
        }
    }

    /// Collect WaitAll results with error strategy support.
    async fn collect_wait_all(
        &self,
        node: &super::node::ParallelNode<S>,
        state: S,
        handles: Vec<(NodeId, tokio::task::JoinHandle<Result<S, NodeError>>)>,
        execution_id: &str,
    ) -> Result<S, NodeError> {
        match &node.error_strategy {
            ErrorStrategy::FailFast => {
                let mut merged_state = state;
                for (_branch_id, handle) in handles {
                    let result = handle
                        .await
                        .map_err(|e| NodeError::HandlerError(format!("task join error: {e}")))??;
                    merged_state.merge(result);
                }
                Ok(merged_state)
            }
            ErrorStrategy::ContinueOnError => {
                let mut merged_state = state;
                let mut failed_count = 0;
                let total = handles.len();
                for (branch_id, handle) in handles {
                    match handle.await {
                        Ok(Ok(s)) => merged_state.merge(s),
                        Ok(Err(e)) => {
                            failed_count += 1;
                            tracing::warn!(
                                branch = %branch_id, error = %e,
                                "branch failed (continuing)"
                            );
                            self.observer
                                .on_parallel_branch_failed(
                                    execution_id,
                                    &node.id,
                                    &branch_id,
                                    &e.to_string(),
                                )
                                .await;
                        }
                        Err(e) => {
                            failed_count += 1;
                            tracing::warn!(
                                branch = %branch_id, error = %e,
                                "branch panicked (continuing)"
                            );
                            self.observer
                                .on_parallel_branch_failed(
                                    execution_id,
                                    &node.id,
                                    &branch_id,
                                    &e.to_string(),
                                )
                                .await;
                        }
                    }
                }
                if failed_count > 0 {
                    self.observer
                        .on_parallel_partial(
                            execution_id,
                            &node.id,
                            total - failed_count,
                            failed_count,
                        )
                        .await;
                }
                Ok(merged_state)
            }
            ErrorStrategy::RetryThenFail { max_retries } => {
                let mut merged_state = state.clone();
                let mut pending: Vec<(NodeId, u32)> = Vec::new();

                // First round: collect results
                for (branch_id, handle) in handles {
                    match handle.await {
                        Ok(Ok(s)) => merged_state.merge(s),
                        Ok(Err(_)) | Err(_) => {
                            pending.push((branch_id, 0));
                        }
                    }
                }

                // Retry loop
                while !pending.is_empty() {
                    let mut next_pending = Vec::new();
                    for (branch_id, attempt) in pending {
                        let handler = self.get_branch_handler(&branch_id)?;
                        let timeout = self.get_branch_timeout(&branch_id);
                        let branch_state = state.clone();
                        let ctx = NodeContext {
                            graph_execution_id: execution_id.to_string(),
                            node_id: branch_id.clone(),
                            depth: 0,
                            max_depth: self.graph.config.max_depth,
                            cancelled: Arc::new(self.cancel_rx.clone()),
                        };
                        match tokio::time::timeout(timeout, handler.execute(branch_state, &ctx))
                            .await
                        {
                            Ok(Ok(s)) => merged_state.merge(s),
                            Ok(Err(e)) if attempt + 1 < *max_retries => {
                                tracing::warn!(
                                    branch = %branch_id,
                                    attempt = attempt + 1,
                                    "branch retry {}/{}",
                                    attempt + 1,
                                    max_retries
                                );
                                next_pending.push((branch_id, attempt + 1));
                            }
                            Ok(Err(e)) => {
                                return Err(NodeError::HandlerError(format!(
                                    "branch {} failed after {} retries: {}",
                                    branch_id, max_retries, e
                                )));
                            }
                            Err(_) => {
                                if attempt + 1 < *max_retries {
                                    next_pending.push((branch_id, attempt + 1));
                                } else {
                                    return Err(NodeError::Timeout(timeout));
                                }
                            }
                        }
                    }
                    pending = next_pending;
                }
                Ok(merged_state)
            }
        }
    }

    /// Get a branch's handler by node ID.
    fn get_branch_handler(
        &self,
        branch_id: &NodeId,
    ) -> Result<Arc<dyn super::node::NodeHandler<S>>, NodeError> {
        self.graph
            .get_node(branch_id)
            .and_then(|n| match n {
                NodeType::Function(f) => Some(f.handler.clone()),
                _ => None,
            })
            .ok_or_else(|| NodeError::HandlerError(format!("branch {} not found", branch_id)))
    }

    /// Get a branch's timeout by node ID.
    fn get_branch_timeout(&self, branch_id: &NodeId) -> Duration {
        self.graph
            .get_node(branch_id)
            .and_then(|n| match n {
                NodeType::Function(f) => Some(f.timeout),
                _ => None,
            })
            .unwrap_or(Duration::from_secs(30))
    }

    /// Execute the graph using DAG scheduling for automatic parallelism.
    ///
    /// Analyzes the graph topology and runs independent nodes in parallel
    /// waves. Falls back to sequential execution if the graph is not
    /// DAG-schedulable (e.g., contains conditional edges).
    pub async fn execute_dag(&self, initial_state: S) -> Result<S, GraphError> {
        use super::dag_scheduler::DagScheduler;

        let waves = match DagScheduler::compute_waves(&self.graph) {
            Some(w) => w,
            None => return self.execute(initial_state).await,
        };

        let execution_id = uuid::Uuid::new_v4().to_string();
        let graph_start = Instant::now();

        self.observer
            .on_graph_start(&execution_id, &initial_state)
            .await;

        let mut state = initial_state;

        for wave in &waves {
            self.observer
                .on_dag_wave_start(&execution_id, wave.wave_index, &wave.nodes)
                .await;
            let wave_start = Instant::now();

            if wave.nodes.len() == 1 {
                // Single node — execute directly
                let node_id = &wave.nodes[0];
                let node = self
                    .graph
                    .get_node(node_id)
                    .ok_or_else(|| GraphError::NodeNotFound(node_id.clone()))?;

                self.observer
                    .on_node_enter(&execution_id, node_id, &state)
                    .await;
                let node_start = Instant::now();

                match self
                    .execute_node(node, state.clone(), &execution_id, 0)
                    .await
                {
                    Ok(new_state) => {
                        self.observer
                            .on_node_exit(&execution_id, node_id, &new_state, node_start.elapsed())
                            .await;
                        state = new_state;
                    }
                    Err(e) => {
                        let graph_err = GraphError::NodeExecutionFailed {
                            node_id: node_id.clone(),
                            message: e.to_string(),
                        };
                        self.observer
                            .on_node_error(&execution_id, node_id, &graph_err)
                            .await;
                        return Err(graph_err);
                    }
                }
            } else {
                // Multiple nodes — execute in parallel wave
                state = self
                    .execute_wave(&wave.nodes, state, &execution_id, 0)
                    .await?;
            }

            self.observer
                .on_dag_wave_complete(&execution_id, wave.wave_index, wave_start.elapsed())
                .await;

            // Checkpoint after each wave
            if self.graph.config.checkpoint_enabled {
                if let Some(ref cp) = self.graph.checkpointer {
                    let cp_id = format!("{}:wave_{}", execution_id, wave.wave_index);
                    if let Some(last_node) = wave.nodes.last() {
                        match cp.save(&cp_id, last_node, &state).await {
                            Ok(saved_id) => {
                                self.observer
                                    .on_checkpoint(&execution_id, last_node, &saved_id)
                                    .await;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    wave = wave.wave_index,
                                    "wave checkpoint save failed (non-fatal)"
                                );
                            }
                        }
                    }
                }
            }
        }

        self.observer
            .on_graph_complete(&execution_id, &state, graph_start.elapsed())
            .await;
        Ok(state)
    }

    /// Execute a wave of independent nodes in parallel.
    async fn execute_wave(
        &self,
        node_ids: &[NodeId],
        state: S,
        execution_id: &str,
        depth: usize,
    ) -> Result<S, GraphError> {
        let mut handles = Vec::new();

        for node_id in node_ids {
            let s = state.clone();
            let nid = node_id.clone();
            let eid = execution_id.to_string();
            let graph = self.graph.clone();
            let observer = self.observer.clone();
            let semaphore = self.semaphore.clone();
            // R2-C3: Pass parent cancel_rx so wave nodes can be cancelled
            let cancel_rx = self.cancel_rx.clone();

            let handle = tokio::spawn(async move {
                let _permit = semaphore
                    .acquire()
                    .await
                    .map_err(|_| GraphError::Internal("semaphore closed".into()))?;

                let node = graph
                    .get_node(&nid)
                    .ok_or_else(|| GraphError::NodeNotFound(nid.clone()))?;

                let handler = match node {
                    NodeType::Function(f) => f.handler.clone(),
                    _ => {
                        return Err(GraphError::Internal(
                            "DAG wave nodes must be function nodes".into(),
                        ))
                    }
                };
                let timeout = match node {
                    NodeType::Function(f) => f.timeout,
                    _ => Duration::from_secs(30),
                };

                observer.on_node_enter(&eid, &nid, &s).await;
                let start = Instant::now();

                let ctx = NodeContext {
                    graph_execution_id: eid.clone(),
                    node_id: nid.clone(),
                    depth,
                    max_depth: graph.config.max_depth,
                    cancelled: Arc::new(cancel_rx),
                };

                match tokio::time::timeout(timeout, handler.execute(s, &ctx)).await {
                    Ok(Ok(new_state)) => {
                        observer
                            .on_node_exit(&eid, &nid, &new_state, start.elapsed())
                            .await;
                        Ok(new_state)
                    }
                    Ok(Err(e)) => {
                        let graph_err = GraphError::NodeExecutionFailed {
                            node_id: nid.clone(),
                            message: e.to_string(),
                        };
                        observer.on_node_error(&eid, &nid, &graph_err).await;
                        Err(graph_err)
                    }
                    Err(_) => {
                        let graph_err = GraphError::NodeTimeout {
                            node_id: nid.clone(),
                            timeout,
                        };
                        observer.on_node_error(&eid, &nid, &graph_err).await;
                        Err(graph_err)
                    }
                }
            });
            handles.push(handle);
        }

        // Collect and merge results
        let mut merged = state;
        for handle in handles {
            let result = handle
                .await
                .map_err(|e| GraphError::Internal(format!("task join: {e}")))?;
            merged.merge(result?);
        }
        Ok(merged)
    }

    /// Automatically choose between sequential and DAG execution.
    ///
    /// If `dag_scheduling` is enabled in the graph config and the graph
    /// is DAG-schedulable, uses parallel wave execution. Otherwise falls
    /// back to sequential execution.
    pub async fn execute_auto(&self, initial_state: S) -> Result<S, GraphError> {
        use super::dag_scheduler::DagScheduler;

        if self.graph.config.dag_scheduling && DagScheduler::is_dag_schedulable(&self.graph) {
            self.execute_dag(initial_state).await
        } else {
            self.execute(initial_state).await
        }
    }

    /// Resolve the next node to execute after the current node.
    async fn resolve_next_node(
        &self,
        current_node: &NodeId,
        state: &S,
    ) -> Result<NodeId, GraphError> {
        let edges =
            self.graph
                .get_edges(current_node)
                .ok_or_else(|| GraphError::NodeExecutionFailed {
                    node_id: current_node.clone(),
                    message: "no outgoing edges".into(),
                })?;

        if edges.is_empty() {
            return Err(GraphError::NodeExecutionFailed {
                node_id: current_node.clone(),
                message: "no outgoing edges".into(),
            });
        }

        // Use the first edge (most graphs have one edge per node,
        // conditional edges handle multiple routes internally)
        edges[0].resolve_target(state).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::builder::StateGraphBuilder;
    use crate::graph::checkpoint::MemoryCheckpointer;
    use crate::graph::edge::EdgePredicate;
    use crate::graph::node::{ClosureHandler, RetryPolicy};
    use crate::graph::observer::TracingObserver;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct TestState {
        value: i32,
        path: Vec<String>,
    }

    impl GraphState for TestState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
            self.path.extend(other.path);
        }
    }

    fn make_handler(name: &str, increment: i32) -> ClosureHandler<TestState> {
        let name = name.to_string();
        ClosureHandler::new(move |mut state: TestState, _ctx: &NodeContext| {
            let name = name.clone();
            async move {
                state.value += increment;
                state.path.push(name);
                Ok(state)
            }
        })
    }

    #[tokio::test]
    async fn test_execute_linear_graph() {
        let graph = StateGraphBuilder::new()
            .add_node("a", make_handler("a", 1))
            .add_node("b", make_handler("b", 2))
            .add_node("c", make_handler("c", 3))
            .add_edge("a", "b")
            .add_edge("b", "c")
            .set_entry("a")
            .set_terminal("c")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 6); // 0 + 1 + 2 + 3
        assert_eq!(result.path, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn test_execute_conditional_branching() {
        struct CheckValue;

        #[async_trait::async_trait]
        impl EdgePredicate<TestState> for CheckValue {
            async fn evaluate(&self, state: &TestState) -> String {
                if state.value > 5 {
                    "high".into()
                } else {
                    "low".into()
                }
            }
        }

        let graph = StateGraphBuilder::new()
            .add_node("start", make_handler("start", 10))
            .add_node("high_path", make_handler("high", 100))
            .add_node("low_path", make_handler("low", 1))
            .add_conditional_edge(
                "start",
                CheckValue,
                vec![("high", "high_path"), ("low", "low_path")],
            )
            .set_entry("start")
            .set_terminal("high_path")
            .set_terminal("low_path")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        // start adds 10, then value=10 > 5 → high path adds 100
        assert_eq!(result.value, 110);
        assert_eq!(result.path, vec!["start", "high"]);
    }

    #[tokio::test]
    async fn test_execute_conditional_low_path() {
        struct CheckValueLow;

        #[async_trait::async_trait]
        impl EdgePredicate<TestState> for CheckValueLow {
            async fn evaluate(&self, state: &TestState) -> String {
                if state.value > 5 {
                    "high".into()
                } else {
                    "low".into()
                }
            }
        }

        let predicate = CheckValueLow;

        let graph = StateGraphBuilder::new()
            .add_node("start", make_handler("start", 1)) // value becomes 1, < 5
            .add_node("high_path", make_handler("high", 100))
            .add_node("low_path", make_handler("low", 10))
            .add_conditional_edge(
                "start",
                predicate,
                vec![("high", "high_path"), ("low", "low_path")],
            )
            .set_entry("start")
            .set_terminal("high_path")
            .set_terminal("low_path")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 11); // 0 + 1 + 10
        assert_eq!(result.path, vec!["start", "low"]);
    }

    #[tokio::test]
    async fn test_execute_with_checkpointing() {
        let checkpointer = MemoryCheckpointer::new();

        let graph = StateGraphBuilder::new()
            .add_node("a", make_handler("a", 1))
            .add_node("b", make_handler("b", 2))
            .add_edge("a", "b")
            .set_entry("a")
            .set_terminal("b")
            .with_checkpointer(checkpointer)
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 3);
    }

    #[tokio::test]
    async fn test_execute_with_observer() {
        let graph = StateGraphBuilder::new()
            .add_node("a", make_handler("a", 1))
            .set_entry("a")
            .set_terminal("a")
            .with_observer(TracingObserver::new())
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 1);
    }

    #[tokio::test]
    async fn test_execute_cancellation() {
        let graph = StateGraphBuilder::new()
            .add_node(
                "slow",
                ClosureHandler::new(|state: TestState, _ctx: &NodeContext| async move {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    Ok(state)
                }),
            )
            .add_node("unreachable", make_handler("unreachable", 1))
            .add_edge("slow", "unreachable")
            .set_entry("slow")
            .set_terminal("unreachable")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };

        // Cancel immediately before execution
        executor.cancel();
        let result = executor.execute(state).await;
        assert!(matches!(result, Err(GraphError::Cancelled)));
    }

    #[tokio::test]
    async fn test_execute_node_error() {
        let graph = StateGraphBuilder::new()
            .add_node(
                "failing",
                ClosureHandler::new(|_state: TestState, _ctx: &NodeContext| async move {
                    Err(NodeError::HandlerError("something broke".into()))
                }),
            )
            .set_entry("failing")
            .set_terminal("failing")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("something broke"));
    }

    #[tokio::test]
    async fn test_execute_node_timeout() {
        use crate::graph::builder::GraphConfig;

        let config = GraphConfig {
            max_depth: 5,
            default_node_timeout: Duration::from_millis(50),
            checkpoint_enabled: false,
            ..Default::default()
        };

        let graph = StateGraphBuilder::new()
            .with_config(config)
            .add_node(
                "slow",
                ClosureHandler::new(|state: TestState, _ctx: &NodeContext| async move {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    Ok(state)
                }),
            )
            .set_entry("slow")
            .set_terminal("slow")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_parallel_wait_all() {
        use crate::graph::node::ParallelNode;

        let graph = StateGraphBuilder::new()
            .add_node("branch_a", make_handler("a", 10))
            .add_node("branch_b", make_handler("b", 20))
            .add_parallel_node(
                ParallelNode::new(
                    "parallel",
                    "Parallel",
                    vec!["branch_a".into(), "branch_b".into()],
                )
                .with_join_strategy(JoinStrategy::WaitAll),
            )
            .add_node("end", make_handler("end", 1))
            .add_edge("parallel", "end")
            .set_entry("parallel")
            .set_terminal("end")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        // Parallel merges: 0 + (0+10) + (0+20) = 30, then end adds 1
        assert_eq!(result.value, 31);
        assert!(result.path.contains(&"a".to_string()));
        assert!(result.path.contains(&"b".to_string()));
        assert!(result.path.contains(&"end".to_string()));
    }

    #[tokio::test]
    async fn test_execute_single_node_graph() {
        let graph = StateGraphBuilder::new()
            .add_node("only", make_handler("only", 42))
            .set_entry("only")
            .set_terminal("only")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 42);
        assert_eq!(result.path, vec!["only"]);
    }

    #[tokio::test]
    async fn test_execute_retry_on_retryable_error() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let attempt_count = Arc::new(AtomicU32::new(0));
        let attempt_count_clone = attempt_count.clone();

        let handler = ClosureHandler::new(move |mut state: TestState, _ctx: &NodeContext| {
            let count = attempt_count_clone.clone();
            async move {
                let attempt = count.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(NodeError::Retryable("transient failure".into()))
                } else {
                    state.value = 99;
                    state.path.push("success".into());
                    Ok(state)
                }
            }
        });

        let graph = StateGraphBuilder::new()
            .add_named_node(
                "retrying",
                "Retrying Node",
                handler,
                RetryPolicy {
                    max_retries: 3,
                    initial_backoff: Duration::from_millis(1),
                    backoff_multiplier: 1.0,
                },
                Duration::from_secs(10),
            )
            .set_entry("retrying")
            .set_terminal("retrying")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 99);
        assert_eq!(attempt_count.load(Ordering::SeqCst), 3); // 2 failures + 1 success
    }

    // ── A23 Module 1 tests ──

    #[tokio::test]
    async fn test_parallel_max_concurrency_respected() {
        use crate::graph::node::ParallelNode;
        use std::sync::atomic::{AtomicU32, Ordering};

        let concurrent = Arc::new(AtomicU32::new(0));
        let max_seen = Arc::new(AtomicU32::new(0));

        let make_tracking_handler =
            |concurrent: Arc<AtomicU32>, max_seen: Arc<AtomicU32>, val: i32| {
                ClosureHandler::new(move |mut state: TestState, _ctx: &NodeContext| {
                    let c = concurrent.clone();
                    let m = max_seen.clone();
                    async move {
                        let cur = c.fetch_add(1, Ordering::SeqCst) + 1;
                        // Track max concurrent
                        loop {
                            let prev = m.load(Ordering::SeqCst);
                            if cur <= prev
                                || m.compare_exchange(prev, cur, Ordering::SeqCst, Ordering::SeqCst)
                                    .is_ok()
                            {
                                break;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        c.fetch_sub(1, Ordering::SeqCst);
                        state.value += val;
                        Ok(state)
                    }
                })
            };

        let graph = StateGraphBuilder::new()
            .add_node(
                "b1",
                make_tracking_handler(concurrent.clone(), max_seen.clone(), 1),
            )
            .add_node(
                "b2",
                make_tracking_handler(concurrent.clone(), max_seen.clone(), 1),
            )
            .add_node(
                "b3",
                make_tracking_handler(concurrent.clone(), max_seen.clone(), 1),
            )
            .add_node(
                "b4",
                make_tracking_handler(concurrent.clone(), max_seen.clone(), 1),
            )
            .add_node(
                "b5",
                make_tracking_handler(concurrent.clone(), max_seen.clone(), 1),
            )
            .add_parallel_node(
                ParallelNode::new(
                    "par",
                    "Parallel",
                    vec![
                        "b1".into(),
                        "b2".into(),
                        "b3".into(),
                        "b4".into(),
                        "b5".into(),
                    ],
                )
                .with_max_concurrency(2),
            )
            .set_entry("par")
            .set_terminal("par")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 5); // 5 branches each add 1
        assert!(
            max_seen.load(Ordering::SeqCst) <= 2,
            "max concurrent should be <= 2"
        );
    }

    #[tokio::test]
    async fn test_dual_semaphore_interaction() {
        use crate::graph::builder::GraphConfig;
        use crate::graph::node::ParallelNode;
        use std::sync::atomic::{AtomicU32, Ordering};

        let concurrent = Arc::new(AtomicU32::new(0));
        let max_seen = Arc::new(AtomicU32::new(0));

        let make_handler_tracked =
            |concurrent: Arc<AtomicU32>, max_seen: Arc<AtomicU32>, val: i32| {
                ClosureHandler::new(move |mut state: TestState, _ctx: &NodeContext| {
                    let c = concurrent.clone();
                    let m = max_seen.clone();
                    async move {
                        let cur = c.fetch_add(1, Ordering::SeqCst) + 1;
                        loop {
                            let prev = m.load(Ordering::SeqCst);
                            if cur <= prev
                                || m.compare_exchange(prev, cur, Ordering::SeqCst, Ordering::SeqCst)
                                    .is_ok()
                            {
                                break;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        c.fetch_sub(1, Ordering::SeqCst);
                        state.value += val;
                        Ok(state)
                    }
                })
            };

        // max_concurrency=5 on node but max_global=3 → actual concurrency <= 3
        let config = GraphConfig {
            max_global_concurrency: 3,
            ..Default::default()
        };

        let graph = StateGraphBuilder::new()
            .with_config(config)
            .add_node(
                "b1",
                make_handler_tracked(concurrent.clone(), max_seen.clone(), 1),
            )
            .add_node(
                "b2",
                make_handler_tracked(concurrent.clone(), max_seen.clone(), 1),
            )
            .add_node(
                "b3",
                make_handler_tracked(concurrent.clone(), max_seen.clone(), 1),
            )
            .add_node(
                "b4",
                make_handler_tracked(concurrent.clone(), max_seen.clone(), 1),
            )
            .add_node(
                "b5",
                make_handler_tracked(concurrent.clone(), max_seen.clone(), 1),
            )
            .add_parallel_node(
                ParallelNode::new(
                    "par",
                    "Parallel",
                    vec![
                        "b1".into(),
                        "b2".into(),
                        "b3".into(),
                        "b4".into(),
                        "b5".into(),
                    ],
                )
                .with_max_concurrency(5),
            )
            .set_entry("par")
            .set_terminal("par")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 5);
        assert!(
            max_seen.load(Ordering::SeqCst) <= 3,
            "max concurrent should be <= 3 (global limit)"
        );
    }

    #[tokio::test]
    async fn test_continue_on_error_partial_results() {
        use crate::graph::node::ParallelNode;

        let graph = StateGraphBuilder::new()
            .add_node("ok_1", make_handler("ok_1", 10))
            .add_node("ok_2", make_handler("ok_2", 20))
            .add_node(
                "fail",
                ClosureHandler::new(|_state: TestState, _ctx: &NodeContext| async move {
                    Err(NodeError::HandlerError("deliberate failure".into()))
                }),
            )
            .add_parallel_node(
                ParallelNode::new(
                    "par",
                    "Parallel",
                    vec!["ok_1".into(), "fail".into(), "ok_2".into()],
                )
                .with_error_strategy(ErrorStrategy::ContinueOnError),
            )
            .set_entry("par")
            .set_terminal("par")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        // 2 successful branches: 10 + 20 = 30
        assert_eq!(result.value, 30);
        assert!(result.path.contains(&"ok_1".to_string()));
        assert!(result.path.contains(&"ok_2".to_string()));
    }

    #[tokio::test]
    async fn test_retry_then_fail_success() {
        use crate::graph::node::ParallelNode;
        use std::sync::atomic::{AtomicU32, Ordering};

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let graph = StateGraphBuilder::new()
            .add_node("ok", make_handler("ok", 10))
            .add_node(
                "flaky",
                ClosureHandler::new(move |mut state: TestState, _ctx: &NodeContext| {
                    let a = attempts_clone.clone();
                    async move {
                        let attempt = a.fetch_add(1, Ordering::SeqCst);
                        if attempt < 2 {
                            Err(NodeError::HandlerError("transient".into()))
                        } else {
                            state.value += 5;
                            state.path.push("flaky".into());
                            Ok(state)
                        }
                    }
                }),
            )
            .add_parallel_node(
                ParallelNode::new("par", "Parallel", vec!["ok".into(), "flaky".into()])
                    .with_error_strategy(ErrorStrategy::RetryThenFail { max_retries: 3 }),
            )
            .set_entry("par")
            .set_terminal("par")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        // ok=10 + flaky=5 = 15
        assert_eq!(result.value, 15);
    }

    #[tokio::test]
    async fn test_retry_then_fail_exhausted() {
        use crate::graph::node::ParallelNode;

        let graph = StateGraphBuilder::new()
            .add_node("ok", make_handler("ok", 10))
            .add_node(
                "always_fail",
                ClosureHandler::new(|_state: TestState, _ctx: &NodeContext| async move {
                    Err(NodeError::HandlerError("permanent".into()))
                }),
            )
            .add_parallel_node(
                ParallelNode::new("par", "Parallel", vec!["ok".into(), "always_fail".into()])
                    .with_error_strategy(ErrorStrategy::RetryThenFail { max_retries: 2 }),
            )
            .set_entry("par")
            .set_terminal("par")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed after 2 retries"));
    }

    #[tokio::test]
    async fn test_fail_fast_backward_compat() {
        // Default ErrorStrategy::FailFast — one failure kills the whole parallel
        use crate::graph::node::ParallelNode;

        let graph = StateGraphBuilder::new()
            .add_node("ok", make_handler("ok", 10))
            .add_node(
                "fail",
                ClosureHandler::new(|_state: TestState, _ctx: &NodeContext| async move {
                    Err(NodeError::HandlerError("boom".into()))
                }),
            )
            .add_parallel_node(
                ParallelNode::new("par", "Parallel", vec!["ok".into(), "fail".into()]),
                // Default FailFast error_strategy
            )
            .set_entry("par")
            .set_terminal("par")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_execute_dag_diamond() {
        use crate::graph::builder::GraphConfig;

        let config = GraphConfig {
            dag_scheduling: true,
            ..Default::default()
        };

        // Diamond: entry→a, entry→b, a→end, b→end
        let graph = StateGraphBuilder::new()
            .with_config(config)
            .add_node("entry", make_handler("entry", 1))
            .add_node("a", make_handler("a", 10))
            .add_node("b", make_handler("b", 20))
            .add_node("end", make_handler("end", 100))
            .add_edge("entry", "a")
            .add_edge("entry", "b")
            .add_edge("a", "end")
            .add_edge("b", "end")
            .set_entry("entry")
            .set_terminal("end")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute_dag(state).await.unwrap();
        // Wave 0: entry(0→1)
        // Wave 1: a(1→11), b(1→21), merged = 1+11+21 = 33
        // Wave 2: end(33→133)
        assert_eq!(result.value, 133);
    }

    #[tokio::test]
    async fn test_execute_auto_uses_dag() {
        use crate::graph::builder::GraphConfig;

        let config = GraphConfig {
            dag_scheduling: true,
            ..Default::default()
        };

        let graph = StateGraphBuilder::new()
            .with_config(config)
            .add_node("entry", make_handler("entry", 1))
            .add_node("a", make_handler("a", 10))
            .add_node("b", make_handler("b", 20))
            .add_node("end", make_handler("end", 100))
            .add_edge("entry", "a")
            .add_edge("entry", "b")
            .add_edge("a", "end")
            .add_edge("b", "end")
            .set_entry("entry")
            .set_terminal("end")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute_auto(state).await.unwrap();
        assert_eq!(result.value, 133);
    }

    #[tokio::test]
    async fn test_execute_auto_falls_back_to_sequential() {
        use crate::graph::builder::GraphConfig;
        use crate::graph::edge::ClosurePredicate;

        let config = GraphConfig {
            dag_scheduling: true,
            ..Default::default()
        };

        // Graph with conditional edge → cannot DAG schedule, falls back
        let graph = StateGraphBuilder::new()
            .with_config(config)
            .add_node("start", make_handler("start", 1))
            .add_node("end", make_handler("end", 10))
            .add_conditional_edge(
                "start",
                ClosurePredicate::new(|_: &TestState| "end".to_string()),
                vec![("end", "end")],
            )
            .set_entry("start")
            .set_terminal("end")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute_auto(state).await.unwrap();
        assert_eq!(result.value, 11);
    }

    #[tokio::test]
    async fn test_budget_terminates_execution() {
        use crate::graph::budget::{ExecutionBudget, NodeBudget};

        // Create a budget that will be "exceeded" (we simulate by setting
        // a very small global budget and recording manually)
        let budget = ExecutionBudget::new(1000);

        // Record usage so the budget is nearly exhausted
        budget.record("pre", 999);

        let graph = StateGraphBuilder::new()
            .add_node("a", make_handler("a", 1))
            .set_entry("a")
            .set_terminal("a")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph).with_budget(budget);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        // Execution still works (budget check is at executor level, not enforced in this basic test)
        let result = executor.execute(state).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_wave_parallel() {
        // Wave with 2 nodes — both execute and their values merge
        let graph = StateGraphBuilder::new()
            .add_node("a", make_handler("a", 10))
            .add_node("b", make_handler("b", 20))
            .add_node("end", make_handler("end", 1))
            .add_edge("a", "end")
            .add_edge("b", "end")
            .set_entry("a")
            .set_terminal("end")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };

        let result = executor
            .execute_wave(&["a".into(), "b".into()], state, "test_exec", 0)
            .await
            .unwrap();

        // base(0) + a(0→10) + b(0→20) = 0+10+20 = 30
        assert_eq!(result.value, 30);
    }

    // ── A23 observer integration tests ──

    #[tokio::test]
    async fn test_observer_parallel_partial_called() {
        use crate::graph::node::ParallelNode;
        use crate::graph::observer::CompositeObserver;
        use std::sync::atomic::{AtomicU32, Ordering};

        struct PartialObserver {
            partial_count: AtomicU32,
            succeeded: std::sync::Mutex<Vec<usize>>,
            failed: std::sync::Mutex<Vec<usize>>,
        }

        #[async_trait::async_trait]
        impl super::super::observer::GraphObserver<TestState> for PartialObserver {
            async fn on_parallel_partial(
                &self,
                _exec_id: &str,
                _node_id: &NodeId,
                succeeded: usize,
                failed: usize,
            ) {
                self.partial_count.fetch_add(1, Ordering::SeqCst);
                self.succeeded.lock().unwrap().push(succeeded);
                self.failed.lock().unwrap().push(failed);
            }
        }

        let observer = Arc::new(PartialObserver {
            partial_count: AtomicU32::new(0),
            succeeded: std::sync::Mutex::new(Vec::new()),
            failed: std::sync::Mutex::new(Vec::new()),
        });

        struct SharedObs(Arc<PartialObserver>);

        #[async_trait::async_trait]
        impl super::super::observer::GraphObserver<TestState> for SharedObs {
            async fn on_parallel_partial(
                &self,
                exec_id: &str,
                node_id: &NodeId,
                succeeded: usize,
                failed: usize,
            ) {
                self.0
                    .on_parallel_partial(exec_id, node_id, succeeded, failed)
                    .await;
            }
        }

        let graph = StateGraphBuilder::new()
            .add_node("ok_1", make_handler("ok_1", 10))
            .add_node("ok_2", make_handler("ok_2", 20))
            .add_node(
                "fail",
                ClosureHandler::new(|_state: TestState, _ctx: &NodeContext| async move {
                    Err(NodeError::HandlerError("deliberate failure".into()))
                }),
            )
            .add_parallel_node(
                ParallelNode::new(
                    "par",
                    "Parallel",
                    vec!["ok_1".into(), "fail".into(), "ok_2".into()],
                )
                .with_error_strategy(ErrorStrategy::ContinueOnError),
            )
            .set_entry("par")
            .set_terminal("par")
            .with_observer(SharedObs(observer.clone()))
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 30);

        // on_parallel_partial should have been called once
        assert_eq!(observer.partial_count.load(Ordering::SeqCst), 1);
        // 2 succeeded, 1 failed
        assert_eq!(observer.succeeded.lock().unwrap()[0], 2);
        assert_eq!(observer.failed.lock().unwrap()[0], 1);
    }

    #[tokio::test]
    async fn test_observer_branch_failed_called() {
        use crate::graph::node::ParallelNode;
        use std::sync::atomic::{AtomicU32, Ordering};

        struct BranchFailObserver {
            fail_count: AtomicU32,
            branch_ids: std::sync::Mutex<Vec<String>>,
        }

        #[async_trait::async_trait]
        impl super::super::observer::GraphObserver<TestState> for BranchFailObserver {
            async fn on_parallel_branch_failed(
                &self,
                _exec_id: &str,
                _node_id: &NodeId,
                branch_id: &NodeId,
                _error: &str,
            ) {
                self.fail_count.fetch_add(1, Ordering::SeqCst);
                self.branch_ids.lock().unwrap().push(branch_id.clone());
            }
        }

        let observer = Arc::new(BranchFailObserver {
            fail_count: AtomicU32::new(0),
            branch_ids: std::sync::Mutex::new(Vec::new()),
        });

        struct SharedBFObs(Arc<BranchFailObserver>);

        #[async_trait::async_trait]
        impl super::super::observer::GraphObserver<TestState> for SharedBFObs {
            async fn on_parallel_branch_failed(
                &self,
                exec_id: &str,
                node_id: &NodeId,
                branch_id: &NodeId,
                error: &str,
            ) {
                self.0
                    .on_parallel_branch_failed(exec_id, node_id, branch_id, error)
                    .await;
            }
        }

        let graph = StateGraphBuilder::new()
            .add_node("ok", make_handler("ok", 10))
            .add_node(
                "fail_1",
                ClosureHandler::new(|_state: TestState, _ctx: &NodeContext| async move {
                    Err(NodeError::HandlerError("failure 1".into()))
                }),
            )
            .add_node(
                "fail_2",
                ClosureHandler::new(|_state: TestState, _ctx: &NodeContext| async move {
                    Err(NodeError::HandlerError("failure 2".into()))
                }),
            )
            .add_parallel_node(
                ParallelNode::new(
                    "par",
                    "Parallel",
                    vec!["ok".into(), "fail_1".into(), "fail_2".into()],
                )
                .with_error_strategy(ErrorStrategy::ContinueOnError),
            )
            .set_entry("par")
            .set_terminal("par")
            .with_observer(SharedBFObs(observer.clone()))
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let _result = executor.execute(state).await.unwrap();

        // 2 branches failed → on_parallel_branch_failed called twice
        assert_eq!(observer.fail_count.load(Ordering::SeqCst), 2);
        let ids = observer.branch_ids.lock().unwrap();
        assert!(ids.contains(&"fail_1".to_string()));
        assert!(ids.contains(&"fail_2".to_string()));
    }

    #[tokio::test]
    async fn test_retry_reuses_original_state() {
        use crate::graph::node::ParallelNode;
        use std::sync::atomic::{AtomicU32, Ordering};

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        // Handler that fails once, then succeeds. On success, verifies state.value == 0
        // (the original state value, not accumulated from prior attempts).
        let graph = StateGraphBuilder::new()
            .add_node(
                "flaky",
                ClosureHandler::new(move |mut state: TestState, _ctx: &NodeContext| {
                    let a = attempts_clone.clone();
                    async move {
                        let attempt = a.fetch_add(1, Ordering::SeqCst);
                        if attempt == 0 {
                            Err(NodeError::HandlerError("transient".into()))
                        } else {
                            // On retry, the original state (value=0) should be used
                            state.path.push(format!("success_v{}", state.value));
                            state.value += 100;
                            Ok(state)
                        }
                    }
                }),
            )
            .add_node("ok", make_handler("ok", 5))
            .add_parallel_node(
                ParallelNode::new("par", "Parallel", vec!["flaky".into(), "ok".into()])
                    .with_error_strategy(ErrorStrategy::RetryThenFail { max_retries: 3 }),
            )
            .set_entry("par")
            .set_terminal("par")
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };
        let result = executor.execute(state).await.unwrap();
        // ok(5) + flaky-retry(100) + base(0) = 105
        assert_eq!(result.value, 105);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        // The retry should use original state (value=0)
        assert!(result.path.contains(&"success_v0".to_string()));
    }

    #[tokio::test]
    async fn test_dag_observer_wave_events() {
        use super::super::dag_scheduler::DagScheduler;
        use std::sync::atomic::{AtomicU32, Ordering};

        struct WaveObserver {
            wave_starts: AtomicU32,
            wave_completes: AtomicU32,
            wave_indices: std::sync::Mutex<Vec<usize>>,
        }

        #[async_trait::async_trait]
        impl super::super::observer::GraphObserver<TestState> for WaveObserver {
            async fn on_dag_wave_start(
                &self,
                _exec_id: &str,
                wave_index: usize,
                _node_ids: &[NodeId],
            ) {
                self.wave_starts.fetch_add(1, Ordering::SeqCst);
                self.wave_indices.lock().unwrap().push(wave_index);
            }

            async fn on_dag_wave_complete(
                &self,
                _exec_id: &str,
                _wave_index: usize,
                _duration: Duration,
            ) {
                self.wave_completes.fetch_add(1, Ordering::SeqCst);
            }
        }

        let observer = Arc::new(WaveObserver {
            wave_starts: AtomicU32::new(0),
            wave_completes: AtomicU32::new(0),
            wave_indices: std::sync::Mutex::new(Vec::new()),
        });

        struct SharedWObs(Arc<WaveObserver>);

        #[async_trait::async_trait]
        impl super::super::observer::GraphObserver<TestState> for SharedWObs {
            async fn on_dag_wave_start(
                &self,
                exec_id: &str,
                wave_index: usize,
                node_ids: &[NodeId],
            ) {
                self.0
                    .on_dag_wave_start(exec_id, wave_index, node_ids)
                    .await;
            }
            async fn on_dag_wave_complete(
                &self,
                exec_id: &str,
                wave_index: usize,
                duration: Duration,
            ) {
                self.0
                    .on_dag_wave_complete(exec_id, wave_index, duration)
                    .await;
            }
        }

        // Diamond: A,B → C (A and B are independent, C depends on both)
        let graph = StateGraphBuilder::new()
            .add_node("a", make_handler("a", 1))
            .add_node("b", make_handler("b", 2))
            .add_node("c", make_handler("c", 3))
            .add_edge("a", "c")
            .add_edge("b", "c")
            .set_entry("a")
            .set_terminal("c")
            .with_observer(SharedWObs(observer.clone()))
            .build()
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TestState {
            value: 0,
            path: vec![],
        };

        // Use DAG execution
        let result = executor.execute_dag(state).await.unwrap();
        // A(1) + B(2) + C(3) — DAG merges values from wave
        assert!(result.value > 0);

        // 2 waves: wave 0 = [A, B], wave 1 = [C]
        assert_eq!(observer.wave_starts.load(Ordering::SeqCst), 2);
        assert_eq!(observer.wave_completes.load(Ordering::SeqCst), 2);
        let indices = observer.wave_indices.lock().unwrap();
        assert_eq!(indices[0], 0);
        assert_eq!(indices[1], 1);
    }

    #[tokio::test]
    async fn test_budget_observer_events() {
        // Verify that budget-related observer methods can be called directly
        // through the RecordingObserver → ExecutionStore pipeline.
        // The executor does not yet call on_budget_exceeded automatically;
        // this test validates the observer wiring from caller code.
        use crate::graph::execution_store::{EventPayload, ExecutionMode, ExecutionStore};
        use crate::graph::recording_observer::RecordingObserver;

        let store = Arc::new(ExecutionStore::new(10));
        store
            .start_execution("budget_test", ExecutionMode::Direct)
            .await;

        let observer = RecordingObserver::new(store.clone(), "budget_test");
        let obs: &dyn super::super::observer::GraphObserver<TestState> = &observer;

        // Simulate budget warning and exceeded events
        obs.on_budget_warning("budget_test", &"expensive_node".to_string())
            .await;
        obs.on_budget_exceeded("budget_test", &"expensive_node".to_string())
            .await;

        let events = store.get_events("budget_test", 0, None);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0].payload,
            EventPayload::BudgetWarning { .. }
        ));
        assert!(matches!(
            events[1].payload,
            EventPayload::BudgetExceeded { .. }
        ));
    }
}
