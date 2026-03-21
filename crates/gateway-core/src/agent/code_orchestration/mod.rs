//! Code Orchestration module for Programmatic Tool Calling
//!
//! Provides the infrastructure for LLM-generated code to programmatically
//! orchestrate tool calls with loops, conditions, and error handling,
//! executing in a Docker sandbox with an HTTP tool proxy bridge.
//!
//! # Architecture
//!
//! ```text
//! LLM generates Python/JS code
//!   │
//!   ▼
//! CodeOrchestrationRuntime
//!   ├─ Starts ToolProxyBridge (HTTP server)
//!   ├─ ToolCodeGenerator creates SDK preamble
//!   ├─ Preamble + LLM code → Docker sandbox
//!   │   └─ tools.read() ──HTTP POST──> ToolProxyBridge
//!   │                                    └─> ToolRegistry / McpGateway
//!   ├─ Collects tool_call records
//!   └─ Returns CodeOrchestrationResult
//! ```

pub mod codegen;
pub mod runtime;
pub mod tool_proxy;
pub mod types;

pub use codegen::ToolCodeGenerator;
pub use runtime::CodeOrchestrationRuntime;
pub use tool_proxy::ToolProxyBridge;
pub use types::{
    CodeOrchestrationConfig, CodeOrchestrationRequest, CodeOrchestrationResult, ToolCallRecord,
};
