//! StateGraph execution engine.
//!
//! This module implements a graph-based execution engine inspired by LangGraph.
//! It provides typed state flow between nodes, conditional routing, parallel
//! execution, checkpointing, and observability.
//!
//! # Architecture
//!
//! ```text
//! StateGraphBuilder → StateGraph → GraphExecutor → Result<S>
//!                                       ↓
//!                                 [Nodes + Edges]
//!                                       ↓
//!                              [Checkpointer + Observer]
//! ```
//!
//! # Example
//!
//! ```ignore
//! use gateway_core::graph::*;
//!
//! #[derive(Clone, Serialize, Deserialize)]
//! struct MyState { count: i32 }
//!
//! impl GraphState for MyState {
//!     fn merge(&mut self, other: Self) { self.count += other.count; }
//! }
//!
//! let graph = StateGraphBuilder::new()
//!     .add_node("increment", ClosureHandler::new(|mut s: MyState, _| async move {
//!         s.count += 1;
//!         Ok(s)
//!     }))
//!     .set_entry("increment")
//!     .set_terminal("increment")
//!     .build()?;
//!
//! let executor = GraphExecutor::new(graph);
//! let result = executor.execute(MyState { count: 0 }).await?;
//! assert_eq!(result.count, 1);
//! ```

pub mod adapters;
pub mod budget;
pub mod builder;
pub mod checkpoint;
pub mod dag_scheduler;
#[cfg(feature = "devtools")]
pub mod devtools_observer;
pub mod edge;
pub mod error;
pub mod execution_store;
pub mod execution_store_observer;
pub mod executor;
pub mod memory_bridge;
pub mod node;
pub mod observer;
pub mod recording_observer;
pub mod streaming_observer;

// Re-export core types for convenience.
pub use adapters::{AgentGraphState, AgentRunnerNode, LlmCallNode, StateMetadata, ToolCallNode};
pub use budget::{
    BudgetAction, BudgetCheckResult, ExecutionBudget, NodeBudget, ParallelBudgetStrategy,
};
pub use builder::{GraphConfig, StateGraph, StateGraphBuilder};
pub use checkpoint::{CheckpointInfo, FileCheckpointer, GraphCheckpointer, MemoryCheckpointer};
pub use dag_scheduler::{DagScheduler, DagSegment, ExecutionWave};
#[cfg(feature = "devtools")]
pub use devtools_observer::DevtoolsObserver;
pub use edge::{ClosurePredicate, ConditionalEdge, DirectEdge, EdgePredicate, EdgeType};
pub use error::{GraphError, NodeError, NodeId};
pub use execution_store::{
    EventPayload, ExecutionEvent, ExecutionMode, ExecutionRecord, ExecutionStatus, ExecutionStore,
    ExecutionSummary, GlobalEvent,
};
pub use execution_store_observer::ExecutionStoreObserver;
pub use executor::GraphExecutor;
pub use memory_bridge::{MemoryBridge, MemoryBridgeConfig};
pub use node::{
    ClosureHandler, ErrorStrategy, FunctionNode, HumanReviewNode, JoinStrategy, NodeContext,
    NodeHandler, NodeType, ParallelNode, RetryPolicy,
};
pub use observer::{CompositeObserver, GraphObserver, NoOpObserver, TracingObserver};
pub use recording_observer::RecordingObserver;
pub use streaming_observer::{GraphStreamEvent, StreamingObserver};

/// Trait that all graph states must implement.
///
/// Graph state flows through nodes and edges, getting transformed at each
/// step. The state must be cloneable (for parallel execution and checkpointing),
/// serializable (for persistence), and thread-safe.
///
/// The `merge` method is used by parallel nodes to combine results from
/// multiple branches back into a single state.
pub trait GraphState:
    Clone + Send + Sync + serde::Serialize + serde::de::DeserializeOwned + 'static
{
    /// Merge another state into this one.
    ///
    /// This is called by parallel nodes to combine branch results.
    /// The implementation should define how to merge fields (e.g., sum values,
    /// append collections, take latest timestamps).
    fn merge(&mut self, other: Self);
}
