//! Tools namespace handler
//!
//! Handles `tools.*` calls — listing and calling tools from the catalog.

use super::HandlerContext;
use crate::dispatcher::DispatchError;
use canal_identity::types::AgentIdentity;

/// Handle a tools namespace tool call
pub async fn handle(
    _ctx: &HandlerContext,
    _identity: &AgentIdentity,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, DispatchError> {
    match tool_name {
        "tools.list" => handle_list(arguments).await,
        "tools.call" => handle_call(arguments).await,
        _ => Err(DispatchError::ToolNotFound(tool_name.to_string())),
    }
}

async fn handle_list(_arguments: serde_json::Value) -> Result<serde_json::Value, DispatchError> {
    // R9-M5: Return error instead of fake empty result
    Err(DispatchError::HandlerError(
        "tools.list is not yet implemented — external MCP tool listing pending integration"
            .to_string(),
    ))
}

async fn handle_call(arguments: serde_json::Value) -> Result<serde_json::Value, DispatchError> {
    let _server = arguments.get("server").and_then(|v| v.as_str());
    let _tool = arguments.get("tool").and_then(|v| v.as_str());

    // R9-M5: Return error instead of fake success
    Err(DispatchError::HandlerError(
        "tools.call is not yet implemented — external MCP tool proxying pending integration"
            .to_string(),
    ))
}
