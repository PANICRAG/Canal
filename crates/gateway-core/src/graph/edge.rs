//! Graph edge types and predicates.
//!
//! Edges connect nodes in the graph. Direct edges always transition to a
//! single target. Conditional edges evaluate a predicate on the current
//! state to determine which route to take.

use std::sync::Arc;

use async_trait::async_trait;

use super::error::NodeId;
use super::GraphState;

/// An edge that always transitions to a fixed target node.
#[derive(Debug, Clone)]
pub struct DirectEdge {
    /// Source node.
    pub from: NodeId,
    /// Target node.
    pub to: NodeId,
}

/// A conditional edge that routes based on state evaluation.
pub struct ConditionalEdge<S: GraphState> {
    /// Source node.
    pub from: NodeId,
    /// Predicate that evaluates state and returns a route label.
    pub predicate: Arc<dyn EdgePredicate<S>>,
    /// Map of route labels to target node IDs.
    pub routes: Vec<(String, NodeId)>,
    /// Default target if predicate returns an unknown label.
    pub default: Option<NodeId>,
}

/// Trait for edge predicates that evaluate state to determine routing.
///
/// The predicate returns a route label (String) that is matched against
/// the edge's routes map.
///
/// # Example
///
/// ```ignore
/// struct IsPositive;
///
/// #[async_trait]
/// impl EdgePredicate<MyState> for IsPositive {
///     async fn evaluate(&self, state: &MyState) -> String {
///         if state.value > 0 { "positive".into() } else { "negative".into() }
///     }
/// }
/// ```
#[async_trait]
pub trait EdgePredicate<S: GraphState>: Send + Sync {
    /// Evaluate the state and return a route label.
    async fn evaluate(&self, state: &S) -> String;
}

/// Sync closure-based edge predicate for simple cases.
///
/// Takes a synchronous closure that evaluates state and returns a route label.
/// For async predicates, implement `EdgePredicate` directly.
pub struct ClosurePredicate<S: GraphState> {
    predicate: Box<dyn Fn(&S) -> String + Send + Sync>,
}

impl<S: GraphState> ClosurePredicate<S> {
    /// Create a predicate from a sync closure.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&S) -> String + Send + Sync + 'static,
    {
        Self {
            predicate: Box::new(f),
        }
    }
}

#[async_trait]
impl<S: GraphState> EdgePredicate<S> for ClosurePredicate<S> {
    async fn evaluate(&self, state: &S) -> String {
        (self.predicate)(state)
    }
}

/// Enum representing all edge types in the graph.
pub enum EdgeType<S: GraphState> {
    /// A direct edge that always transitions to a fixed target.
    Direct(DirectEdge),
    /// A conditional edge that routes based on state.
    Conditional(ConditionalEdge<S>),
}

impl<S: GraphState> EdgeType<S> {
    /// Get the source node ID.
    pub fn from_node(&self) -> &NodeId {
        match self {
            EdgeType::Direct(e) => &e.from,
            EdgeType::Conditional(e) => &e.from,
        }
    }

    /// Resolve the target node ID given the current state.
    pub async fn resolve_target(&self, state: &S) -> Result<NodeId, super::error::GraphError> {
        match self {
            EdgeType::Direct(e) => Ok(e.to.clone()),
            EdgeType::Conditional(e) => {
                let label = e.predicate.evaluate(state).await;
                // Find matching route
                for (route_label, target) in &e.routes {
                    if route_label == &label {
                        return Ok(target.clone());
                    }
                }
                // Try default
                if let Some(default) = &e.default {
                    return Ok(default.clone());
                }
                Err(super::error::GraphError::UnknownRoute {
                    from: e.from.clone(),
                    label,
                    available: e.routes.iter().map(|(l, _)| l.clone()).collect(),
                })
            }
        }
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

    impl GraphState for TestState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
        }
    }

    #[tokio::test]
    async fn test_direct_edge_resolve() {
        let edge = EdgeType::<TestState>::Direct(DirectEdge {
            from: "a".into(),
            to: "b".into(),
        });
        let state = TestState { value: 0 };
        let target = edge.resolve_target(&state).await.unwrap();
        assert_eq!(target, "b");
    }

    #[tokio::test]
    async fn test_conditional_edge_resolve() {
        struct CheckPositive;

        #[async_trait]
        impl EdgePredicate<TestState> for CheckPositive {
            async fn evaluate(&self, state: &TestState) -> String {
                if state.value > 0 {
                    "positive".into()
                } else {
                    "negative".into()
                }
            }
        }

        let edge = EdgeType::Conditional(ConditionalEdge {
            from: "start".into(),
            predicate: Arc::new(CheckPositive),
            routes: vec![
                ("positive".into(), "happy_path".into()),
                ("negative".into(), "sad_path".into()),
            ],
            default: None,
        });

        let positive_state = TestState { value: 5 };
        let target = edge.resolve_target(&positive_state).await.unwrap();
        assert_eq!(target, "happy_path");

        let negative_state = TestState { value: -1 };
        let target = edge.resolve_target(&negative_state).await.unwrap();
        assert_eq!(target, "sad_path");
    }

    #[tokio::test]
    async fn test_conditional_edge_default() {
        struct AlwaysUnknown;

        #[async_trait]
        impl EdgePredicate<TestState> for AlwaysUnknown {
            async fn evaluate(&self, _state: &TestState) -> String {
                "unknown_label".into()
            }
        }

        let edge = EdgeType::Conditional(ConditionalEdge {
            from: "start".into(),
            predicate: Arc::new(AlwaysUnknown),
            routes: vec![("known".into(), "target".into())],
            default: Some("fallback".into()),
        });

        let state = TestState { value: 0 };
        let target = edge.resolve_target(&state).await.unwrap();
        assert_eq!(target, "fallback");
    }

    #[tokio::test]
    async fn test_conditional_edge_unknown_no_default() {
        struct AlwaysUnknown;

        #[async_trait]
        impl EdgePredicate<TestState> for AlwaysUnknown {
            async fn evaluate(&self, _state: &TestState) -> String {
                "mystery".into()
            }
        }

        let edge = EdgeType::Conditional(ConditionalEdge {
            from: "start".into(),
            predicate: Arc::new(AlwaysUnknown),
            routes: vec![("known".into(), "target".into())],
            default: None,
        });

        let state = TestState { value: 0 };
        let result = edge.resolve_target(&state).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_closure_predicate() {
        let predicate = ClosurePredicate::new(|state: &TestState| {
            if state.value > 10 {
                "high".into()
            } else {
                "low".into()
            }
        });

        let state = TestState { value: 20 };
        let label = predicate.evaluate(&state).await;
        assert_eq!(label, "high");

        let state = TestState { value: 3 };
        let label = predicate.evaluate(&state).await;
        assert_eq!(label, "low");
    }

    #[test]
    fn test_edge_from_node() {
        let edge = EdgeType::<TestState>::Direct(DirectEdge {
            from: "src".into(),
            to: "dst".into(),
        });
        assert_eq!(edge.from_node(), "src");
    }
}
