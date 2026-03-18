//! MCP (Model Context Protocol) Gateway
//!
//! This module provides MCP gateway capabilities for connecting to and
//! orchestrating MCP-compatible tool servers.

pub mod browser_client;
pub mod builtin;
pub mod connection;
pub mod discovery;
pub mod gateway;
pub mod memory_tool;
pub mod platform_automation;
pub mod protocol;
pub mod registry;

pub use browser_client::{BrowserMcpClient, BrowserMcpConfig, ScreenshotResult, TabInfo};
pub use builtin::BuiltinToolExecutor;
pub use connection::{McpConnection, McpHttpConfig, McpSpawnConfig};
pub use discovery::{
    builtin_servers, DiscoveredServer, DiscoveredTool, McpDiscovery, ServerConfig, ServerSource,
    TransportConfig,
};
pub use gateway::{
    McpGateway, McpGatewayBuilder, McpServerConfig, McpServerInfo, McpTransport, PermissionRequest,
};
pub use memory_tool::{
    get_memory_system_prompt, get_memory_tool_definition, MemoryToolHandler, MemoryToolInput,
    MemoryToolResult, ModelFamily,
};
pub use platform_automation::{
    get_macos_tool_definitions, get_windows_tool_definitions, PlatformAutomation, ToolDefinition,
};
pub use protocol::{McpToolDef, ToolCallResult, ToolContent};
pub use registry::{Tool, ToolRegistry};

// Re-export permission types from agent module for convenience
pub use crate::agent::types::{
    PermissionBehavior, PermissionContext, PermissionDestination, PermissionMode, PermissionResult,
    PermissionRule, PermissionSuggestion, PermissionUpdate,
};
