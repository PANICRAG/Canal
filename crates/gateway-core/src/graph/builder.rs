//! Fluent builder for constructing state graphs.
//!
//! # Example
//!
//! ```ignore
//! let graph = StateGraphBuilder::new()
//!     .add_node("start", handler_a)
//!     .add_node("process", handler_b)
//!     .add_node("end", handler_c)
//!     .add_edge("start", "process")
//!     .add_conditional_edge("process", predicate, vec![
//!         ("success", "end"),
//!         ("retry", "process"),
//!     ])
//!     .set_entry("start")
//!     .set_terminal("end")
//!     .build()?;
//! ```

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use super::checkpoint::GraphCheckpointer;
use super::edge::{ConditionalEdge, DirectEdge, EdgePredicate, EdgeType};
use super::error::{GraphError, NodeId};
use super::node::{
    FunctionNode, HumanReviewNode, NodeHandler, NodeType, ParallelNode, RetryPolicy,
};
use super::observer::GraphObserver;
use super::GraphState;

/// Configuration for a compiled graph.
#[derive(Debug, Clone)]
pub struct GraphConfig {
    /// Maximum depth for sub-graph nesting.
    pub max_depth: usize,
    /// Default timeout for nodes without explicit timeout.
    pub default_node_timeout: Duration,
    /// Whether to enable checkpointing at each node.
    pub checkpoint_enabled: bool,
    /// Default error strategy for parallel nodes that don't specify one.
    pub default_error_strategy: super::node::ErrorStrategy,
    /// Maximum global concurrency for all parallel execution.
    pub max_global_concurrency: usize,
    /// Whether to enable DAG-level automatic parallel scheduling.
    pub dag_scheduling: bool,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            max_depth: 5,
            default_node_timeout: Duration::from_secs(300),
            checkpoint_enabled: false,
            default_error_strategy: super::node::ErrorStrategy::default(),
            max_global_concurrency: 10,
            dag_scheduling: false,
        }
    }
}

/// A compiled state graph ready for execution.
pub struct StateGraph<S: GraphState> {
    pub(crate) nodes: HashMap<NodeId, NodeType<S>>,
    pub(crate) edges: HashMap<NodeId, Vec<EdgeType<S>>>,
    pub(crate) entry_point: NodeId,
    pub(crate) terminal_nodes: HashSet<NodeId>,
    pub(crate) checkpointer: Option<Arc<dyn GraphCheckpointer<S>>>,
    pub(crate) observer: Option<Arc<dyn GraphObserver<S>>>,
    pub(crate) config: GraphConfig,
}

impl<S: GraphState> StateGraph<S> {
    /// Get the entry point node ID.
    pub fn entry_point(&self) -> &NodeId {
        &self.entry_point
    }

    /// Get the terminal node IDs.
    pub fn terminal_nodes(&self) -> &HashSet<NodeId> {
        &self.terminal_nodes
    }

    /// Check if a node ID is a terminal node.
    pub fn is_terminal(&self, node_id: &NodeId) -> bool {
        self.terminal_nodes.contains(node_id)
    }

    /// Get a node by ID.
    pub fn get_node(&self, node_id: &NodeId) -> Option<&NodeType<S>> {
        self.nodes.get(node_id)
    }

    /// Get edges from a node.
    pub fn get_edges(&self, node_id: &NodeId) -> Option<&Vec<EdgeType<S>>> {
        self.edges.get(node_id)
    }

    /// Get the graph configuration.
    pub fn config(&self) -> &GraphConfig {
        &self.config
    }

    /// Get all node IDs.
    pub fn node_ids(&self) -> Vec<&NodeId> {
        self.nodes.keys().collect()
    }

    /// Get the number of nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// Fluent builder for constructing state graphs.
pub struct StateGraphBuilder<S: GraphState> {
    nodes: HashMap<NodeId, NodeType<S>>,
    edges: HashMap<NodeId, Vec<EdgeType<S>>>,
    entry_point: Option<NodeId>,
    terminal_nodes: HashSet<NodeId>,
    checkpointer: Option<Arc<dyn GraphCheckpointer<S>>>,
    observer: Option<Arc<dyn GraphObserver<S>>>,
    config: GraphConfig,
}

impl<S: GraphState> StateGraphBuilder<S> {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
            entry_point: None,
            terminal_nodes: HashSet::new(),
            checkpointer: None,
            observer: None,
            config: GraphConfig::default(),
        }
    }

    /// Add a function node with a handler.
    pub fn add_node(
        mut self,
        id: impl Into<NodeId>,
        handler: impl NodeHandler<S> + 'static,
    ) -> Self {
        let id = id.into();
        let node = NodeType::Function(FunctionNode {
            id: id.clone(),
            name: id.clone(),
            handler: Arc::new(handler),
            retry_policy: RetryPolicy::default(),
            timeout: self.config.default_node_timeout,
        });
        self.nodes.insert(id, node);
        self
    }

    /// Add a function node with a custom name and configuration.
    pub fn add_named_node(
        mut self,
        id: impl Into<NodeId>,
        name: impl Into<String>,
        handler: impl NodeHandler<S> + 'static,
        retry_policy: RetryPolicy,
        timeout: Duration,
    ) -> Self {
        let id = id.into();
        let node = NodeType::Function(FunctionNode {
            id: id.clone(),
            name: name.into(),
            handler: Arc::new(handler),
            retry_policy,
            timeout,
        });
        self.nodes.insert(id, node);
        self
    }

    /// Add a parallel node that fans out to multiple branches.
    pub fn add_parallel_node(mut self, node: ParallelNode<S>) -> Self {
        let id = node.id.clone();
        self.nodes.insert(id, NodeType::Parallel(node));
        self
    }

    /// Add a human review node.
    pub fn add_human_review_node(mut self, node: HumanReviewNode<S>) -> Self {
        let id = node.id.clone();
        self.nodes.insert(id, NodeType::HumanReview(node));
        self
    }

    /// Add a direct edge from one node to another.
    pub fn add_edge(mut self, from: impl Into<NodeId>, to: impl Into<NodeId>) -> Self {
        let from = from.into();
        let to = to.into();
        let edge = EdgeType::Direct(DirectEdge {
            from: from.clone(),
            to,
        });
        self.edges.entry(from).or_default().push(edge);
        self
    }

    /// Add a conditional edge with a predicate and routes.
    pub fn add_conditional_edge(
        mut self,
        from: impl Into<NodeId>,
        predicate: impl EdgePredicate<S> + 'static,
        routes: Vec<(impl Into<String>, impl Into<NodeId>)>,
    ) -> Self {
        let from = from.into();
        let routes: Vec<(String, NodeId)> = routes
            .into_iter()
            .map(|(label, target)| (label.into(), target.into()))
            .collect();
        let edge = EdgeType::Conditional(ConditionalEdge {
            from: from.clone(),
            predicate: Arc::new(predicate),
            routes,
            default: None,
        });
        self.edges.entry(from).or_default().push(edge);
        self
    }

    /// Add a conditional edge with a default fallback target.
    pub fn add_conditional_edge_with_default(
        mut self,
        from: impl Into<NodeId>,
        predicate: impl EdgePredicate<S> + 'static,
        routes: Vec<(impl Into<String>, impl Into<NodeId>)>,
        default: impl Into<NodeId>,
    ) -> Self {
        let from = from.into();
        let routes: Vec<(String, NodeId)> = routes
            .into_iter()
            .map(|(label, target)| (label.into(), target.into()))
            .collect();
        let edge = EdgeType::Conditional(ConditionalEdge {
            from: from.clone(),
            predicate: Arc::new(predicate),
            routes,
            default: Some(default.into()),
        });
        self.edges.entry(from).or_default().push(edge);
        self
    }

    /// Set the entry point node.
    pub fn set_entry(mut self, node_id: impl Into<NodeId>) -> Self {
        self.entry_point = Some(node_id.into());
        self
    }

    /// Add a terminal node.
    pub fn set_terminal(mut self, node_id: impl Into<NodeId>) -> Self {
        self.terminal_nodes.insert(node_id.into());
        self
    }

    /// Set the checkpointer.
    pub fn with_checkpointer(mut self, checkpointer: impl GraphCheckpointer<S> + 'static) -> Self {
        self.checkpointer = Some(Arc::new(checkpointer));
        self.config.checkpoint_enabled = true;
        self
    }

    /// Set the observer.
    pub fn with_observer(mut self, observer: impl GraphObserver<S> + 'static) -> Self {
        self.observer = Some(Arc::new(observer));
        self
    }

    /// Set the graph configuration.
    pub fn with_config(mut self, config: GraphConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the maximum sub-graph depth.
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.config.max_depth = max_depth;
        self
    }

    /// Build and validate the graph.
    pub fn build(self) -> Result<StateGraph<S>, GraphError> {
        // Validate entry point
        let entry_point = self.entry_point.ok_or(GraphError::NoEntryPoint)?;

        if !self.nodes.contains_key(&entry_point) {
            return Err(GraphError::NodeNotFound(entry_point));
        }

        // Validate terminal nodes
        if self.terminal_nodes.is_empty() {
            return Err(GraphError::NoTerminalNodes);
        }

        for terminal in &self.terminal_nodes {
            if !self.nodes.contains_key(terminal) {
                return Err(GraphError::NodeNotFound(terminal.clone()));
            }
        }

        // Validate all edge targets exist
        for edges in self.edges.values() {
            for edge in edges {
                match edge {
                    EdgeType::Direct(e) => {
                        if !self.nodes.contains_key(&e.to) {
                            return Err(GraphError::NodeNotFound(e.to.clone()));
                        }
                    }
                    EdgeType::Conditional(e) => {
                        for (_, target) in &e.routes {
                            if !self.nodes.contains_key(target) {
                                return Err(GraphError::NodeNotFound(target.clone()));
                            }
                        }
                        if let Some(default) = &e.default {
                            if !self.nodes.contains_key(default) {
                                return Err(GraphError::NodeNotFound(default.clone()));
                            }
                        }
                    }
                }
            }
        }

        Ok(StateGraph {
            nodes: self.nodes,
            edges: self.edges,
            entry_point,
            terminal_nodes: self.terminal_nodes,
            checkpointer: self.checkpointer,
            observer: self.observer,
            config: self.config,
        })
    }
}

impl<S: GraphState> Default for StateGraphBuilder<S> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::node::ClosureHandler;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct TestState {
        value: i32,
    }

    impl GraphState for TestState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
        }
    }

    fn increment_handler() -> ClosureHandler<TestState> {
        ClosureHandler::new(
            |mut state: TestState, _ctx: &crate::graph::node::NodeContext| async move {
                state.value += 1;
                Ok(state)
            },
        )
    }

    #[test]
    fn test_build_simple_graph() {
        let graph = StateGraphBuilder::new()
            .add_node("start", increment_handler())
            .add_node("end", increment_handler())
            .add_edge("start", "end")
            .set_entry("start")
            .set_terminal("end")
            .build();
        assert!(graph.is_ok());
        let graph = graph.unwrap();
        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.entry_point(), "start");
        assert!(graph.is_terminal(&"end".into()));
    }

    #[test]
    fn test_build_no_entry_point() {
        let result = StateGraphBuilder::<TestState>::new()
            .add_node("a", increment_handler())
            .set_terminal("a")
            .build();
        assert!(matches!(result, Err(GraphError::NoEntryPoint)));
    }

    #[test]
    fn test_build_no_terminal_nodes() {
        let result = StateGraphBuilder::<TestState>::new()
            .add_node("a", increment_handler())
            .set_entry("a")
            .build();
        assert!(matches!(result, Err(GraphError::NoTerminalNodes)));
    }

    #[test]
    fn test_build_invalid_entry_node() {
        let result = StateGraphBuilder::<TestState>::new()
            .add_node("a", increment_handler())
            .set_entry("nonexistent")
            .set_terminal("a")
            .build();
        assert!(matches!(result, Err(GraphError::NodeNotFound(_))));
    }

    #[test]
    fn test_build_invalid_terminal_node() {
        let result = StateGraphBuilder::<TestState>::new()
            .add_node("a", increment_handler())
            .set_entry("a")
            .set_terminal("nonexistent")
            .build();
        assert!(matches!(result, Err(GraphError::NodeNotFound(_))));
    }

    #[test]
    fn test_build_invalid_edge_target() {
        let result = StateGraphBuilder::<TestState>::new()
            .add_node("a", increment_handler())
            .add_edge("a", "nonexistent")
            .set_entry("a")
            .set_terminal("a")
            .build();
        assert!(matches!(result, Err(GraphError::NodeNotFound(_))));
    }

    #[test]
    fn test_build_with_config() {
        let config = GraphConfig {
            max_depth: 3,
            default_node_timeout: Duration::from_secs(60),
            checkpoint_enabled: false,
            ..Default::default()
        };
        let graph = StateGraphBuilder::new()
            .with_config(config)
            .add_node("start", increment_handler())
            .set_entry("start")
            .set_terminal("start")
            .build()
            .unwrap();
        assert_eq!(graph.config().max_depth, 3);
    }

    #[test]
    fn test_build_three_node_chain() {
        let graph = StateGraphBuilder::new()
            .add_node("a", increment_handler())
            .add_node("b", increment_handler())
            .add_node("c", increment_handler())
            .add_edge("a", "b")
            .add_edge("b", "c")
            .set_entry("a")
            .set_terminal("c")
            .build();
        assert!(graph.is_ok());
        let graph = graph.unwrap();
        assert_eq!(graph.node_count(), 3);
    }

    #[test]
    fn test_graph_node_ids() {
        let graph = StateGraphBuilder::new()
            .add_node("x", increment_handler())
            .add_node("y", increment_handler())
            .set_entry("x")
            .set_terminal("y")
            .add_edge("x", "y")
            .build()
            .unwrap();
        let mut ids: Vec<&String> = graph.node_ids();
        ids.sort();
        assert_eq!(ids, vec!["x", "y"]);
    }
}
