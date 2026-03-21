//! Direct collaboration mode.
//!
//! The simplest collaboration mode: a single agent handles the entire task.
//! This wraps a single `NodeHandler` execution via a 1-node StateGraph,
//! preserving compatibility with the existing AgentRunner pattern.

use std::sync::Arc;

use crate::graph::{
    GraphError, GraphExecutor, GraphState, NodeContext, NodeError, NodeHandler, StateGraphBuilder,
};

// ClosureHandler used in tests only
#[cfg(test)]
use crate::graph::ClosureHandler;

/// Direct mode: single-agent execution via a 1-node graph.
///
/// This is the simplest collaboration mode. It wraps a single handler
/// in a trivial graph and executes it. Use this for simple tasks that
/// don't require multi-agent coordination.
pub struct DirectMode<S: GraphState> {
    handler: Arc<dyn NodeHandler<S>>,
    name: String,
}

impl<S: GraphState> DirectMode<S> {
    /// Create a new DirectMode with a handler.
    pub fn new(name: impl Into<String>, handler: impl NodeHandler<S> + 'static) -> Self {
        Self {
            handler: Arc::new(handler),
            name: name.into(),
        }
    }

    /// Execute the handler on the given state.
    pub async fn execute(&self, state: S) -> Result<S, GraphError> {
        let handler = self.handler.clone();
        let wrapper = HandlerWrapper { inner: handler };

        let graph = StateGraphBuilder::new()
            .add_node("agent", wrapper)
            .set_entry("agent")
            .set_terminal("agent")
            .build()?;

        let executor = GraphExecutor::new(graph);
        executor.execute(state).await
    }

    /// Get the mode name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Wrapper to clone an Arc<dyn NodeHandler> into a new NodeHandler.
struct HandlerWrapper<S: GraphState> {
    inner: Arc<dyn NodeHandler<S>>,
}

#[async_trait::async_trait]
impl<S: GraphState> NodeHandler<S> for HandlerWrapper<S> {
    async fn execute(&self, state: S, ctx: &NodeContext) -> Result<S, NodeError> {
        self.inner.execute(state, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct TestState {
        value: i32,
        messages: Vec<String>,
    }

    impl GraphState for TestState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
            self.messages.extend(other.messages);
        }
    }

    #[tokio::test]
    async fn test_direct_mode_simple() {
        let handler = ClosureHandler::new(|mut state: TestState, _ctx: &NodeContext| async move {
            state.value += 42;
            state.messages.push("processed".into());
            Ok(state)
        });

        let mode = DirectMode::new("test_agent", handler);
        assert_eq!(mode.name(), "test_agent");

        let state = TestState {
            value: 0,
            messages: vec![],
        };
        let result = mode.execute(state).await.unwrap();
        assert_eq!(result.value, 42);
        assert_eq!(result.messages, vec!["processed"]);
    }

    #[tokio::test]
    async fn test_direct_mode_passthrough() {
        let handler =
            ClosureHandler::new(|state: TestState, _ctx: &NodeContext| async move { Ok(state) });

        let mode = DirectMode::new("passthrough", handler);
        let state = TestState {
            value: 99,
            messages: vec!["existing".into()],
        };
        let result = mode.execute(state).await.unwrap();
        assert_eq!(result.value, 99);
        assert_eq!(result.messages, vec!["existing"]);
    }

    #[tokio::test]
    async fn test_direct_mode_error_propagation() {
        let handler = ClosureHandler::new(|_state: TestState, _ctx: &NodeContext| async move {
            Err(NodeError::HandlerError("test error".into()))
        });

        let mode = DirectMode::new("failing", handler);
        let state = TestState {
            value: 0,
            messages: vec![],
        };
        let result = mode.execute(state).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("test error"));
    }
}
