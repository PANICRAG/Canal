//! AI Gateway Core Library
//!
//! This crate contains the core business logic for the AI Gateway,
//! including LLM routing, MCP gateway, chat engine, workflow engine,
//! secure code execution, and filesystem access.

pub mod agent;
pub mod artifacts;
pub mod billing;
pub mod chat;
pub mod computer_use;
pub mod context;
pub mod screen;
// pub mod creative;  // Disabled - pending full implementation
pub mod error;
pub mod executor;
pub mod filesystem;
pub mod git;
pub use gateway_llm as llm;
pub mod mcp;
pub use gateway_memory as memory;
pub use gateway_plugins::connectors;
pub use gateway_plugins::plugins;
pub mod rte;
pub mod session;
pub mod tool_system;
#[cfg(unix)]
pub use gateway_tools::vm;
pub mod workflow;

// Role Constraint System (A46)
pub mod roles;

// Service traits and local implementations (A45 dual-mode deployment)
pub mod services;
pub use gateway_service_traits as service_traits;

// Feature-gated modules (A18: Hybrid Orchestration Architecture v2)
#[cfg(feature = "graph")]
pub mod graph;

#[cfg(feature = "collaboration")]
pub mod collaboration;

#[cfg(feature = "multimodal")]
pub mod multimodal;

#[cfg(feature = "cache")]
pub use gateway_memory::cache;

#[cfg(feature = "learning")]
pub mod learning;

#[cfg(feature = "jobs")]
pub mod jobs;

#[cfg(feature = "prompt-constraints")]
pub mod prompt;

pub use error::{Error, Result};

// Note: canal-engine provides the clean public API (AiEngine trait).
// Consumers should depend on canal-engine directly (with bridge feature)
// rather than accessing it through gateway-core, to avoid cyclic dependencies.
