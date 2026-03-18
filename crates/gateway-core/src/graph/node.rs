//! Graph node types and handlers.
//!
//! Nodes are the execution units of a StateGraph. Each node transforms
//! the graph state and can be a function, sub-graph, or parallel fan-out.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::error::{NodeError, NodeId};
use super::GraphState;

/// Context passed to node handlers during execution.
#[derive(Debug, Clone)]
pub struct NodeContext {
    /// The graph execution ID.
    pub graph_execution_id: String,
    /// The current node ID.
    pub node_id: NodeId,
    /// The current depth (for sub-graph nesting).
    pub depth: usize,
    /// Maximum allowed depth.
    pub max_depth: usize,
    /// Cancellation token.
    pub cancelled: Arc<tokio::sync::watch::Receiver<bool>>,
}

impl NodeContext {
    /// Check if the execution has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }
}

/// Trait for node execution handlers.
///
/// Implement this trait to define custom node behavior. The handler
/// receives the current state and a context, and must return the
/// (potentially modified) state.
///
/// # Example
///
/// ```ignore
/// struct MyNode;
///
/// #[async_trait]
/// impl NodeHandler<MyState> for MyNode {
///     async fn execute(&self, mut state: MyState, _ctx: &NodeContext) -> Result<MyState, NodeError> {
///         state.count += 1;
///         Ok(state)
///     }
/// }
/// ```
#[async_trait]
pub trait NodeHandler<S: GraphState>: Send + Sync {
    /// Execute the node with the given state and context.
    async fn execute(&self, state: S, ctx: &NodeContext) -> Result<S, NodeError>;
}

/// A function node that wraps a NodeHandler.
pub struct FunctionNode<S: GraphState> {
    /// Unique node identifier.
    pub id: NodeId,
    /// Human-readable name.
    pub name: String,
    /// The handler that executes this node's logic.
    pub handler: Arc<dyn NodeHandler<S>>,
    /// Retry policy for transient failures.
    pub retry_policy: RetryPolicy,
    /// Maximum execution time.
    pub timeout: Duration,
}

/// Retry policy for node execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retries.
    pub max_retries: u32,
    /// Initial backoff duration.
    pub initial_backoff: Duration,
    /// Backoff multiplier (exponential backoff).
    pub backoff_multiplier: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 0,
            initial_backoff: Duration::from_millis(100),
            backoff_multiplier: 2.0,
        }
    }
}

/// A parallel node that fans out to multiple branches and joins results.
pub struct ParallelNode<S: GraphState> {
    /// Unique node identifier.
    pub id: NodeId,
    /// Human-readable name.
    pub name: String,
    /// Branch node IDs to execute in parallel.
    pub branches: Vec<NodeId>,
    /// How to join branch results.
    pub join_strategy: JoinStrategy,
    /// Maximum concurrent branches.
    pub max_concurrency: usize,
    /// How to handle branch failures (only applies to WaitAll strategy).
    pub error_strategy: ErrorStrategy,
    /// Phantom data for generic.
    _phantom: std::marker::PhantomData<S>,
}

impl<S: GraphState> ParallelNode<S> {
    /// Create a new parallel node.
    pub fn new(id: impl Into<NodeId>, name: impl Into<String>, branches: Vec<NodeId>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            branches,
            join_strategy: JoinStrategy::WaitAll,
            max_concurrency: 10,
            error_strategy: ErrorStrategy::default(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Set the join strategy.
    pub fn with_join_strategy(mut self, strategy: JoinStrategy) -> Self {
        self.join_strategy = strategy;
        self
    }

    /// Set maximum concurrency.
    pub fn with_max_concurrency(mut self, max: usize) -> Self {
        self.max_concurrency = max;
        self
    }

    /// Set error handling strategy for WaitAll joins.
    pub fn with_error_strategy(mut self, strategy: ErrorStrategy) -> Self {
        self.error_strategy = strategy;
        self
    }
}

/// Strategy for joining parallel branch results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JoinStrategy {
    /// Wait for all branches to complete, merge all results.
    WaitAll,
    /// Wait for the first branch to complete, cancel others.
    WaitFirst,
    /// Wait for N branches to complete.
    WaitQuorum(usize),
}

/// Strategy for handling errors in parallel execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ErrorStrategy {
    /// First branch failure terminates the parallel execution (default, backward-compatible).
    FailFast,
    /// Continue executing remaining branches, collect partial results.
    ContinueOnError,
    /// Retry failed branches up to max_retries, then fail if still unsuccessful.
    RetryThenFail {
        /// Maximum number of retries per failed branch.
        max_retries: u32,
    },
}

impl Default for ErrorStrategy {
    fn default() -> Self {
        Self::FailFast
    }
}

/// A human-in-the-loop review node.
pub struct HumanReviewNode<S: GraphState> {
    /// Unique node identifier.
    pub id: NodeId,
    /// Human-readable name.
    pub name: String,
    /// Message to display to the human reviewer.
    pub review_prompt: String,
    /// Phantom data for generic.
    _phantom: std::marker::PhantomData<S>,
}

impl<S: GraphState> HumanReviewNode<S> {
    /// Create a new human review node.
    pub fn new(id: impl Into<NodeId>, name: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            review_prompt: prompt.into(),
            _phantom: std::marker::PhantomData,
        }
    }
}

/// Enum representing all node types in the graph.
pub enum NodeType<S: GraphState> {
    /// A function node with a handler.
    Function(FunctionNode<S>),
    /// A parallel execution node.
    Parallel(ParallelNode<S>),
    /// A human-in-the-loop review node.
    HumanReview(HumanReviewNode<S>),
}

impl<S: GraphState> NodeType<S> {
    /// Get the node ID.
    pub fn id(&self) -> &NodeId {
        match self {
            NodeType::Function(n) => &n.id,
            NodeType::Parallel(n) => &n.id,
            NodeType::HumanReview(n) => &n.id,
        }
    }

    /// Get the node name.
    pub fn name(&self) -> &str {
        match self {
            NodeType::Function(n) => &n.name,
            NodeType::Parallel(n) => &n.name,
            NodeType::HumanReview(n) => &n.name,
        }
    }
}

/// Closure-based node handler for simple cases.
pub struct ClosureHandler<S: GraphState> {
    handler: Box<
        dyn Fn(
                S,
                &NodeContext,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<S, NodeError>> + Send + '_>,
            > + Send
            + Sync,
    >,
}

impl<S: GraphState> ClosureHandler<S> {
    /// Create a handler from an async closure.
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: Fn(S, &NodeContext) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<S, NodeError>> + Send + 'static,
    {
        Self {
            handler: Box::new(move |state, ctx| Box::pin(f(state, ctx))),
        }
    }
}

#[async_trait]
impl<S: GraphState> NodeHandler<S> for ClosureHandler<S> {
    async fn execute(&self, state: S, ctx: &NodeContext) -> Result<S, NodeError> {
        (self.handler)(state, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct TestState {
        value: i32,
    }

    impl GraphState for TestState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
        }
    }

    struct IncrementHandler;

    #[async_trait]
    impl NodeHandler<TestState> for IncrementHandler {
        async fn execute(
            &self,
            mut state: TestState,
            _ctx: &NodeContext,
        ) -> Result<TestState, NodeError> {
            state.value += 1;
            Ok(state)
        }
    }

    fn make_test_context() -> NodeContext {
        let (_tx, rx) = tokio::sync::watch::channel(false);
        NodeContext {
            graph_execution_id: "test".into(),
            node_id: "test_node".into(),
            depth: 0,
            max_depth: 5,
            cancelled: Arc::new(rx),
        }
    }

    #[tokio::test]
    async fn test_function_node_handler() {
        let handler = Arc::new(IncrementHandler);
        let state = TestState { value: 0 };
        let ctx = make_test_context();
        let result = handler.execute(state, &ctx).await.unwrap();
        assert_eq!(result.value, 1);
    }

    #[tokio::test]
    async fn test_closure_handler() {
        let handler = ClosureHandler::new(|mut state: TestState, _ctx: &NodeContext| async move {
            state.value *= 2;
            Ok(state)
        });
        let state = TestState { value: 5 };
        let ctx = make_test_context();
        let result = handler.execute(state, &ctx).await.unwrap();
        assert_eq!(result.value, 10);
    }

    #[test]
    fn test_node_type_accessors() {
        let node: NodeType<TestState> = NodeType::Function(FunctionNode {
            id: "n1".into(),
            name: "Node One".into(),
            handler: Arc::new(IncrementHandler),
            retry_policy: RetryPolicy::default(),
            timeout: Duration::from_secs(30),
        });
        assert_eq!(node.id(), "n1");
        assert_eq!(node.name(), "Node One");
    }

    #[test]
    fn test_parallel_node_builder() {
        let node = ParallelNode::<TestState>::new("p1", "Parallel", vec!["a".into(), "b".into()])
            .with_join_strategy(JoinStrategy::WaitFirst)
            .with_max_concurrency(5);
        assert_eq!(node.id, "p1");
        assert_eq!(node.branches.len(), 2);
        assert_eq!(node.max_concurrency, 5);
    }

    #[test]
    fn test_retry_policy_default() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 0);
        assert_eq!(policy.backoff_multiplier, 2.0);
    }

    #[test]
    fn test_node_context_cancelled() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let ctx = NodeContext {
            graph_execution_id: "test".into(),
            node_id: "n1".into(),
            depth: 0,
            max_depth: 5,
            cancelled: Arc::new(rx),
        };
        assert!(!ctx.is_cancelled());
        tx.send(true).unwrap();
        assert!(ctx.is_cancelled());
    }

    #[test]
    fn test_graph_state_merge() {
        let mut a = TestState { value: 3 };
        let b = TestState { value: 7 };
        a.merge(b);
        assert_eq!(a.value, 10);
    }
}
