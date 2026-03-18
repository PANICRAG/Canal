//! MCP Server management endpoints
//!
//! This module provides REST API endpoints for dynamic MCP server management:
//! - `POST /mcp/servers` - Add a new MCP server dynamically
//! - `DELETE /mcp/servers/{name}` - Remove an MCP server
//! - `GET /mcp/servers/{name}/tools` - Get tools from a specific server

use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::{error::ApiError, state::AppState};

/// Create the MCP routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/servers", get(list_servers))
        .route("/servers", post(add_server))
        .route("/servers/{name}", get(get_server))
        .route("/servers/{name}", delete(remove_server))
        .route("/servers/{name}/tools", get(get_server_tools))
        .route("/servers/{name}/health", get(server_health))
        .route("/servers/{name}/connect", post(connect_server))
}

/// MCP Server info for API
#[derive(Debug, Serialize)]
pub struct ApiMcpServer {
    pub namespace: String,
    pub tool_count: usize,
    pub connected: bool,
    /// Transport type: "stdio" or "http"
    pub transport: String,
    /// Location: "local:<command>" or remote URL
    pub location: String,
    /// Server name
    pub name: String,
}

/// MCP Servers list response
#[derive(Debug, Serialize)]
pub struct McpServersResponse {
    pub servers: Vec<ApiMcpServer>,
    pub count: usize,
}

/// List all MCP servers
pub async fn list_servers(State(state): State<AppState>) -> Json<McpServersResponse> {
    let gateway = &state.mcp_gateway;
    let servers = gateway.list_servers().await;

    let api_servers: Vec<ApiMcpServer> = servers
        .into_iter()
        .map(|(namespace, info)| ApiMcpServer {
            namespace,
            tool_count: info.tool_count,
            connected: info.connected,
            transport: info.transport_type,
            location: info.location,
            name: info.server_name,
        })
        .collect();

    let count = api_servers.len();
    Json(McpServersResponse {
        servers: api_servers,
        count,
    })
}

/// Add server request - dynamically add a new MCP server
#[derive(Debug, Deserialize)]
pub struct AddServerRequest {
    /// Server name (unique identifier)
    pub name: String,
    /// Command to execute (e.g., "npx", "python", "/path/to/binary")
    pub command: String,
    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

/// Add server response
#[derive(Debug, Serialize)]
pub struct AddServerResponse {
    pub name: String,
    pub added: bool,
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Add a new MCP server dynamically
///
/// POST /mcp/servers
///
/// Request body:
/// ```json
/// {
///   "name": "my-server",
///   "command": "npx",
///   "args": ["-y", "@modelcontextprotocol/server-filesystem"],
///   "env": { "HOME": "/Users/me" }
/// }
/// ```
pub async fn add_server(
    State(state): State<AppState>,
    Json(request): Json<AddServerRequest>,
) -> Result<Json<AddServerResponse>, ApiError> {
    tracing::info!(
        name = %request.name,
        command = %request.command,
        "Adding MCP server dynamically"
    );

    let gateway = &state.mcp_gateway;

    // Register the server configuration
    gateway
        .register_server(&request.name, &request.command, request.args, request.env)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    tracing::info!(
        name = %request.name,
        "MCP server registered, connecting..."
    );

    // Auto-connect after registration
    let (connected, error) = match gateway.connect_server(&request.name).await {
        Ok(()) => {
            tracing::info!(name = %request.name, "MCP server connected successfully");
            (true, None)
        }
        Err(e) => {
            tracing::warn!(
                name = %request.name,
                error = %e,
                "Failed to auto-connect MCP server"
            );
            (false, Some(e.to_string()))
        }
    };

    Ok(Json(AddServerResponse {
        name: request.name,
        added: true,
        connected,
        error,
    }))
}

/// Connect server response
#[derive(Debug, Serialize)]
pub struct ConnectServerResponse {
    pub name: String,
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Connect to a registered MCP server
///
/// POST /mcp/servers/{name}/connect
pub async fn connect_server(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ConnectServerResponse>, ApiError> {
    tracing::info!(name = %name, "Connecting to MCP server");

    let gateway = &state.mcp_gateway;

    match gateway.connect_server(&name).await {
        Ok(()) => {
            tracing::info!(name = %name, "MCP server connected");
            Ok(Json(ConnectServerResponse {
                name,
                connected: true,
                error: None,
            }))
        }
        Err(e) => {
            tracing::error!(name = %name, error = %e, "Failed to connect MCP server");
            Ok(Json(ConnectServerResponse {
                name,
                connected: false,
                error: Some(e.to_string()),
            }))
        }
    }
}

/// Get a specific MCP server
///
/// GET /mcp/servers/{name}
pub async fn get_server(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ApiMcpServer>, ApiError> {
    let gateway = &state.mcp_gateway;

    let info = gateway
        .get_server_info(&name)
        .await
        .ok_or_else(|| ApiError::not_found(format!("MCP server not found: {}", name)))?;

    Ok(Json(ApiMcpServer {
        namespace: name,
        tool_count: info.tool_count,
        connected: info.connected,
        transport: info.transport_type,
        location: info.location,
        name: info.server_name,
    }))
}

/// Remove server response
#[derive(Debug, Serialize)]
pub struct RemoveServerResponse {
    pub name: String,
    pub removed: bool,
}

/// Remove an MCP server dynamically
///
/// DELETE /mcp/servers/{name}
///
/// Removes the server configuration and disconnects if connected.
pub async fn remove_server(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<RemoveServerResponse>, ApiError> {
    tracing::info!(name = %name, "Removing MCP server");

    let gateway = &state.mcp_gateway;

    // Check if server exists before removing
    if gateway.get_server_info(&name).await.is_none() {
        return Err(ApiError::not_found(format!(
            "MCP server not found: {}",
            name
        )));
    }

    gateway.unregister_server(&name).await;

    tracing::info!(name = %name, "MCP server removed successfully");

    Ok(Json(RemoveServerResponse {
        name,
        removed: true,
    }))
}

/// MCP Tool for API
#[derive(Debug, Serialize)]
pub struct ApiMcpTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Server tools response
#[derive(Debug, Serialize)]
pub struct ServerToolsResponse {
    pub name: String,
    pub tools: Vec<ApiMcpTool>,
    pub count: usize,
}

/// Get tools from a specific MCP server
///
/// GET /mcp/servers/{name}/tools
///
/// Returns the list of tools available from the specified server.
pub async fn get_server_tools(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ServerToolsResponse>, ApiError> {
    let gateway = &state.mcp_gateway;

    // First check if server exists
    if gateway.get_server_info(&name).await.is_none() {
        return Err(ApiError::not_found(format!(
            "MCP server not found: {}",
            name
        )));
    }

    let tools = state.tool_system.list_by_namespace(&name).await;

    let api_tools: Vec<ApiMcpTool> = tools
        .into_iter()
        .map(|t| ApiMcpTool {
            name: t.id.name,
            description: t.description,
            input_schema: t.input_schema,
        })
        .collect();

    let count = api_tools.len();
    Ok(Json(ServerToolsResponse {
        name,
        tools: api_tools,
        count,
    }))
}

/// Server health response
#[derive(Debug, Serialize)]
pub struct ServerHealthResponse {
    pub name: String,
    pub healthy: bool,
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Check health of an MCP server
///
/// GET /mcp/servers/{name}/health
pub async fn server_health(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ServerHealthResponse>, ApiError> {
    let gateway = &state.mcp_gateway;

    match gateway.health_check(&name).await {
        Ok(healthy) => Ok(Json(ServerHealthResponse {
            name,
            healthy,
            connected: healthy,
            error: None,
        })),
        Err(e) => Ok(Json(ServerHealthResponse {
            name,
            healthy: false,
            connected: false,
            error: Some(e.to_string()),
        })),
    }
}
