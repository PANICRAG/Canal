//! Graph execution error types.

use std::time::Duration;

/// Unique identifier for a node in the graph.
pub type NodeId = String;

/// Errors that can occur during graph operations.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    /// A referenced node was not found in the graph.
    #[error("node not found: {0}")]
    NodeNotFound(NodeId),

    /// The graph has no entry point defined.
    #[error("no entry point defined")]
    NoEntryPoint,

    /// The graph has no terminal nodes defined.
    #[error("no terminal nodes defined")]
    NoTerminalNodes,

    /// A cycle was detected in the graph.
    #[error("cycle detected involving node: {0}")]
    CycleDetected(NodeId),

    /// A node execution timed out.
    #[error("node '{node_id}' timed out after {timeout:?}")]
    NodeTimeout { node_id: NodeId, timeout: Duration },

    /// A node execution failed.
    #[error("node '{node_id}' failed: {message}")]
    NodeExecutionFailed { node_id: NodeId, message: String },

    /// An edge predicate returned an unknown route label.
    #[error(
        "edge from '{from}' returned unknown route '{label}', available routes: {available:?}"
    )]
    UnknownRoute {
        from: NodeId,
        label: String,
        available: Vec<String>,
    },

    /// Parallel node execution failed.
    #[error("parallel execution failed: {0}")]
    ParallelExecutionFailed(String),

    /// Maximum graph depth exceeded (sub-graph nesting).
    #[error("maximum graph depth exceeded: {depth} > {max_depth}")]
    MaxDepthExceeded { depth: usize, max_depth: usize },

    /// Checkpoint operation failed.
    #[error("checkpoint error: {0}")]
    CheckpointError(String),

    /// Serialization/deserialization error.
    #[error("serialization error: {0}")]
    SerializationError(String),

    /// The graph execution was cancelled.
    #[error("execution cancelled")]
    Cancelled,

    /// Execution budget exceeded for a node.
    #[error("budget exceeded at node '{0}'")]
    BudgetExceeded(NodeId),

    /// A generic internal error.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Errors that can occur during node execution.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// The node handler returned an error.
    #[error("{0}")]
    HandlerError(String),

    /// The node execution timed out.
    #[error("timeout after {0:?}")]
    Timeout(Duration),

    /// The node was cancelled.
    #[error("cancelled")]
    Cancelled,

    /// A retryable error (will be retried according to retry policy).
    #[error("retryable: {0}")]
    Retryable(String),
}

impl NodeError {
    /// R2-M78: Convert to GraphError with the actual node_id instead of "unknown".
    pub fn into_graph_error(self, node_id: NodeId) -> GraphError {
        match self {
            NodeError::HandlerError(msg) => GraphError::NodeExecutionFailed {
                node_id,
                message: msg,
            },
            NodeError::Timeout(d) => GraphError::NodeTimeout {
                node_id,
                timeout: d,
            },
            NodeError::Cancelled => GraphError::Cancelled,
            NodeError::Retryable(msg) => GraphError::NodeExecutionFailed {
                node_id,
                message: format!("retryable: {msg}"),
            },
        }
    }
}

impl From<NodeError> for GraphError {
    fn from(e: NodeError) -> Self {
        // Fallback when node_id is not available — prefer into_graph_error() when possible
        e.into_graph_error("unknown".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = GraphError::NodeNotFound("agent_1".into());
        assert_eq!(err.to_string(), "node not found: agent_1");

        let err = GraphError::NodeTimeout {
            node_id: "llm_call".into(),
            timeout: Duration::from_secs(30),
        };
        assert!(err.to_string().contains("llm_call"));
        assert!(err.to_string().contains("30s"));
    }

    #[test]
    fn test_node_error_to_graph_error() {
        let node_err = NodeError::HandlerError("bad input".into());
        let graph_err: GraphError = node_err.into();
        assert!(graph_err.to_string().contains("bad input"));
    }
}
