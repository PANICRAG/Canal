//! Gateway Tool Types — Shared traits and types for the Canal tool system.
//!
//! This crate provides the shared interface between `gateway-core` (AI orchestration)
//! and `gateway-tools` (tool execution), breaking the circular dependency between them.
//!
//! # Key Types
//!
//! - [`AgentTool`] — Typed tool trait with associated Input/Output types
//! - [`DynamicTool`] — Type-erased tool trait for registry storage
//! - [`ToolWrapper`] — Bridge from `AgentTool` to `DynamicTool`
//! - [`ToolContext`] — Execution context passed to tools
//! - [`ToolError`] / [`ToolResult`] — Error handling
//! - [`ToolFilterContext`] — Context for filtering tool schemas

pub mod context;
pub mod error;
pub mod filter;
pub mod traits;

pub use context::{PermissionMode, ToolContext};
pub use error::{ToolError, ToolResult};
pub use filter::ToolFilterContext;
pub use traits::{AgentTool, DynamicTool, ToolMetadata, ToolWrapper};
