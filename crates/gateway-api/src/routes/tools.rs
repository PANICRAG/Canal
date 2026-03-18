//! Tool endpoints

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use gateway_core::rte::ToolExecuteResult;
use gateway_core::tool_system::ToolSource;

use crate::{error::ApiError, state::AppState};

/// Create the tools routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_tools))
        .route("/{namespace}/{name}/call", post(call_tool))
        .route("/result", post(submit_tool_result))
}

/// Tool representation for API
#[derive(Debug, Serialize)]
pub struct ApiTool {
    pub name: String,
    pub namespace: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    /// Tool source: "builtin" or "mcp_server"
    pub source: String,
    /// Transport type: "builtin", "stdio", "http"
    pub transport: String,
    /// Location: "local", "local:<command>", or remote URL
    pub location: String,
    /// Server name that provides this tool
    pub server_name: String,
}

/// Tools list response
#[derive(Debug, Serialize)]
pub struct ToolsResponse {
    pub tools: Vec<ApiTool>,
    pub count: usize,
}

/// List all available tools (agent built-in + MCP)
///
/// Returns tools from three sources via the Unified Tool System:
/// - **Agent built-in tools** (`source: "agent_builtin"`): Tools used by the agent loop
///   (Read, Write, Edit, Bash, Glob, Grep, Computer, browser tools, etc.)
/// - **MCP built-in tools** (`source: "mcp_builtin"`): MCP Gateway built-in tools
///   (filesystem, browser, executor, etc.)
/// - **MCP external tools** (`source: "mcp_external"`): Tools from external MCP servers
pub async fn list_tools(State(state): State<AppState>) -> Json<ToolsResponse> {
    let mut api_tools: Vec<ApiTool> = Vec::new();

    // === Agent ToolRegistry tools (dynamic tools from AgentFactory) ===
    let agent_tools = state.agent_tool_registry.list_agent_tools();
    for tool in agent_tools {
        api_tools.push(ApiTool {
            name: tool.name,
            namespace: format!("agent.{}", tool.namespace),
            description: tool.description,
            input_schema: tool.input_schema,
            source: "agent_builtin".to_string(),
            transport: "agent".to_string(),
            location: "local".to_string(),
            server_name: "agent".to_string(),
        });
    }

    // === Unified Tool System tools (MCP builtin + external) ===
    let ts_tools = state.tool_system.list_tools().await;
    for entry in ts_tools {
        // Skip agent tools since we already listed them from agent_tool_registry
        if matches!(entry.source, ToolSource::Agent) {
            continue;
        }

        let source = match &entry.source {
            ToolSource::Agent => "agent_builtin",
            ToolSource::McpBuiltin => "mcp_builtin",
            ToolSource::McpExternal { .. } => "mcp_external",
        };

        api_tools.push(ApiTool {
            name: entry.id.name.clone(),
            namespace: entry.id.namespace.clone(),
            description: entry.description.clone(),
            input_schema: entry.input_schema.clone(),
            source: source.to_string(),
            transport: entry.meta.transport_type.clone(),
            location: entry.meta.location.clone(),
            server_name: entry.meta.server_name.clone(),
        });
    }

    let count = api_tools.len();

    Json(ToolsResponse {
        tools: api_tools,
        count,
    })
}

/// Tool call request
#[derive(Debug, Deserialize)]
pub struct ToolCallRequest {
    pub input: serde_json::Value,
}

/// Tool call response
#[derive(Debug, Serialize)]
pub struct ToolCallResponse {
    pub namespace: String,
    pub tool: String,
    pub output: serde_json::Value,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Call a tool
pub async fn call_tool(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
    Json(request): Json<ToolCallRequest>,
) -> Result<Json<ToolCallResponse>, ApiError> {
    tracing::info!(
        namespace = %namespace,
        tool = %name,
        "Calling tool"
    );

    match state
        .tool_system
        .execute(&namespace, &name, request.input)
        .await
    {
        Ok(output) => {
            tracing::info!(
                namespace = %namespace,
                tool = %name,
                "Tool call succeeded"
            );
            Ok(Json(ToolCallResponse {
                namespace,
                tool: name,
                output,
                success: true,
                error: None,
            }))
        }
        Err(e) => {
            tracing::error!(
                namespace = %namespace,
                tool = %name,
                error = %e,
                "Tool call failed"
            );
            Ok(Json(ToolCallResponse {
                namespace,
                tool: name,
                output: serde_json::Value::Null,
                success: false,
                error: Some(e.to_string()),
            }))
        }
    }
}

// ============================================================
// RTE Tool Result Endpoint (A28)
// ============================================================

/// Request body for POST /api/tools/result
///
/// Native clients POST tool execution results here after receiving
/// a `tool_execute_request` SSE event during a streaming chat session.
#[derive(Debug, Deserialize)]
pub struct ToolResultRequest {
    /// The request_id from the tool_execute_request event
    pub request_id: uuid::Uuid,
    /// Tool execution output
    pub result: serde_json::Value,
    /// Whether execution succeeded
    pub success: bool,
    /// Error message if execution failed
    #[serde(default)]
    pub error: Option<String>,
    /// Actual execution time in milliseconds
    pub execution_time_ms: u64,
    /// HMAC-SHA256 signature for result integrity
    pub hmac_signature: String,
}

/// Response body for POST /api/tools/result
#[derive(Debug, Serialize)]
pub struct ToolResultResponse {
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Accept a tool execution result from a native client.
///
/// The client must sign the result with the session HMAC secret.
/// This endpoint resumes the waiting agent loop that is paused
/// on a oneshot channel.
pub async fn submit_tool_result(
    State(state): State<AppState>,
    Json(request): Json<ToolResultRequest>,
) -> Result<Json<ToolResultResponse>, ApiError> {
    let span = tracing::info_span!(
        "rte_tool_result",
        request_id = %request.request_id,
        success = request.success,
    );
    let _enter = span.enter();

    tracing::info!("Received RTE tool result");

    // Convert to internal type
    let result = ToolExecuteResult {
        request_id: request.request_id,
        result: request.result,
        success: request.success,
        error: request.error,
        execution_time_ms: request.execution_time_ms,
        hmac_signature: request.hmac_signature,
    };

    // Deliver result to the waiting agent loop
    match state.rte_pending.complete(&request.request_id, result) {
        Ok(()) => {
            tracing::info!("RTE tool result delivered to agent loop");
            Ok(Json(ToolResultResponse {
                accepted: true,
                error: None,
            }))
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to deliver RTE tool result");
            Ok(Json(ToolResultResponse {
                accepted: false,
                error: Some(e.to_string()),
            }))
        }
    }
}
