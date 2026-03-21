//! Worker module for Orchestrator-Worker pattern
//!
//! Provides the infrastructure for a lead agent (Orchestrator) to decompose
//! complex tasks into subtasks and dispatch them to multiple worker agents
//! for parallel execution.
//!
//! # Architecture
//!
//! ```text
//! Lead Agent (Opus)
//!   ├─ Decomposes task into WorkerSpecs
//!   ├─ WorkerManager handles:
//!   │   ├─ Topological sort of DAG dependencies
//!   │   ├─ Semaphore-controlled parallel dispatch
//!   │   ├─ Timeout management per worker
//!   │   └─ Result collection
//!   └─ Optional: Synthesizes all WorkerResults into final output
//! ```

pub mod manager;
pub mod orchestrator_agent;
pub mod types;

pub use manager::WorkerManager;
pub use orchestrator_agent::OrchestratorAgent;
pub use types::{
    OrchestratedResult, OrchestratorConfig, WorkerResult, WorkerSpec, WorkerSpecJson, WorkerStatus,
    WorkerUsage,
};
