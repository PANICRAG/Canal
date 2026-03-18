//! Platform namespace handler
//!
//! Handles `platform.*` tool calls for health, info, and status.

use super::HandlerContext;
use crate::dispatcher::DispatchError;
use canal_identity::types::AgentIdentity;

/// Handle a platform namespace tool call
pub async fn handle(
    ctx: &HandlerContext,
    _identity: &AgentIdentity,
    tool_name: &str,
    _arguments: serde_json::Value,
) -> Result<serde_json::Value, DispatchError> {
    match tool_name {
        "platform.health" => handle_health(ctx).await,
        "platform.info" => handle_info(ctx).await,
        _ => Err(DispatchError::ToolNotFound(tool_name.to_string())),
    }
}

async fn handle_health(ctx: &HandlerContext) -> Result<serde_json::Value, DispatchError> {
    let engine_status = if ctx.llm_router.is_some() {
        "connected"
    } else {
        "not_initialized"
    };

    Ok(serde_json::json!({
        "status": "healthy",
        "engine": engine_status,
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

async fn handle_info(_ctx: &HandlerContext) -> Result<serde_json::Value, DispatchError> {
    Ok(serde_json::json!({
        "name": "canal-mcp-server",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol_version": "2024-11-05",
        "namespaces": ["engine", "platform", "tools", "billing", "mcp"]
    }))
}
