//! MCP proxy namespace handler
//!
//! Handles `mcp.*` calls for proxying to external MCP servers.

use super::HandlerContext;
use crate::dispatcher::DispatchError;
use canal_identity::types::AgentIdentity;

/// Handle an mcp proxy namespace tool call
pub async fn handle(
    _ctx: &HandlerContext,
    _identity: &AgentIdentity,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, DispatchError> {
    match tool_name {
        "mcp.servers" => handle_servers().await,
        "mcp.call" => handle_call(arguments).await,
        _ => Err(DispatchError::ToolNotFound(tool_name.to_string())),
    }
}

async fn handle_servers() -> Result<serde_json::Value, DispatchError> {
    // R9-M6: Return error instead of fake empty result
    Err(DispatchError::HandlerError(
        "mcp.servers is not yet implemented — MCP connection manager pending integration"
            .to_string(),
    ))
}

async fn handle_call(_arguments: serde_json::Value) -> Result<serde_json::Value, DispatchError> {
    // R9-M6: Return error instead of fake success
    Err(DispatchError::HandlerError(
        "mcp.call is not yet implemented — MCP proxy pending integration".to_string(),
    ))
}
