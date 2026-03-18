//! Namespace handlers for MCP tool calls
//!
//! Each namespace has its own handler module that processes tool calls.

pub mod engine;
pub mod mcp_proxy;
pub mod platform;
pub mod tools;
pub mod workflow;

use gateway_core::llm::router::LlmRouter;
use gateway_core::tool_system::ToolSystem;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Shared context available to all handlers
pub struct HandlerContext {
    /// LLM Router for chat/streaming (optional — may not be initialized)
    pub llm_router: Option<Arc<RwLock<LlmRouter>>>,
    /// Tool system for tool discovery and execution
    pub tool_system: Option<Arc<ToolSystem>>,
    /// Engine capabilities as JSON (models, execution modes, etc.)
    pub capabilities: serde_json::Value,
}

impl HandlerContext {
    pub fn new(
        llm_router: Option<Arc<RwLock<LlmRouter>>>,
        tool_system: Option<Arc<ToolSystem>>,
        capabilities: serde_json::Value,
    ) -> Self {
        Self {
            llm_router,
            tool_system,
            capabilities,
        }
    }
}
