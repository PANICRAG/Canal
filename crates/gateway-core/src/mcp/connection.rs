//! MCP Connection management
//!
//! Handles connection lifecycle for MCP servers using STDIO and HTTP transports.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::time::{timeout, Duration};

use super::protocol::{JsonRpcRequest, JsonRpcResponse, McpToolDef, ToolCallResult};
use crate::error::{Error, Result};

/// MCP Server configuration for spawning (STDIO transport)
#[derive(Debug, Clone)]
pub struct McpSpawnConfig {
    /// Server name identifier
    pub name: String,
    /// Command to run
    pub command: String,
    /// Command arguments
    pub args: Vec<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Startup timeout in seconds
    pub startup_timeout_secs: u64,
    /// R3-M: Per-request timeout in seconds (default 30)
    pub request_timeout_secs: u64,
}

impl Default for McpSpawnConfig {
    fn default() -> Self {
        Self {
            name: "unnamed".to_string(),
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            startup_timeout_secs: 30,
            request_timeout_secs: 30,
        }
    }
}

/// MCP Server configuration for HTTP transport
#[derive(Debug, Clone)]
pub struct McpHttpConfig {
    /// Server name identifier
    pub name: String,
    /// HTTP endpoint URL
    pub url: String,
    /// Startup/connection timeout in seconds
    pub startup_timeout_secs: u64,
    /// Optional Bearer token for authentication
    pub auth_token: Option<String>,
    /// R3-M: Per-request timeout in seconds (default 30)
    pub request_timeout_secs: u64,
}

impl Default for McpHttpConfig {
    fn default() -> Self {
        Self {
            name: "unnamed".to_string(),
            url: String::new(),
            startup_timeout_secs: 30,
            auth_token: None,
            request_timeout_secs: 30,
        }
    }
}

/// Pending request tracker
struct PendingRequest {
    response_tx: oneshot::Sender<JsonRpcResponse>,
}

/// Internal transport for STDIO connections
struct StdioTransport {
    config: McpSpawnConfig,
    #[allow(dead_code)]
    process: Child,
    request_tx: mpsc::Sender<(JsonRpcRequest, oneshot::Sender<JsonRpcResponse>)>,
    request_id: AtomicU64,
    #[allow(dead_code)]
    io_task: tokio::task::JoinHandle<()>,
}

/// Internal transport for HTTP connections
struct HttpTransport {
    config: McpHttpConfig,
    client: reqwest::Client,
    request_id: AtomicU64,
}

/// Internal transport enum
enum ConnectionTransport {
    Stdio(StdioTransport),
    Http(HttpTransport),
}

/// MCP Connection - manages communication with an MCP server
///
/// Supports both STDIO (subprocess) and HTTP transports. Use `spawn()` for
/// STDIO transport and `connect_http()` for HTTP transport.
pub struct McpConnection {
    transport: ConnectionTransport,
    tools: Arc<RwLock<Vec<McpToolDef>>>,
}

impl McpConnection {
    /// Spawn a new MCP server subprocess and establish connection (STDIO transport)
    pub async fn spawn(config: McpSpawnConfig) -> Result<Self> {
        tracing::info!(
            name = %config.name,
            command = %config.command,
            "Spawning MCP server"
        );

        // Build command
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // Spawn process
        let mut process = cmd.spawn().map_err(|e| {
            Error::Internal(format!(
                "Failed to spawn MCP server '{}': {}",
                config.name, e
            ))
        })?;

        let stdin = process.stdin.take().ok_or_else(|| {
            Error::Internal(format!(
                "Failed to get stdin for MCP server '{}'",
                config.name
            ))
        })?;

        let stdout = process.stdout.take().ok_or_else(|| {
            Error::Internal(format!(
                "Failed to get stdout for MCP server '{}'",
                config.name
            ))
        })?;

        // Create request channel
        let (request_tx, request_rx) = mpsc::channel(100);

        // Start I/O task
        let io_task = tokio::spawn(Self::stdio_io_loop(stdin, stdout, request_rx));

        let stdio_transport = StdioTransport {
            config,
            process,
            request_tx,
            request_id: AtomicU64::new(1),
            io_task,
        };

        let mut conn = Self {
            transport: ConnectionTransport::Stdio(stdio_transport),
            tools: Arc::new(RwLock::new(Vec::new())),
        };

        // Initialize the connection
        conn.initialize().await?;

        // Discover tools
        conn.discover_tools().await?;

        Ok(conn)
    }

    /// Connect to an MCP server over HTTP transport
    pub async fn connect_http(config: McpHttpConfig) -> Result<Self> {
        tracing::info!(
            name = %config.name,
            url = %config.url,
            "Connecting to MCP server via HTTP"
        );

        // Build reqwest client with timeout
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .connect_timeout(Duration::from_secs(config.startup_timeout_secs))
            .build()
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to create HTTP client for MCP server '{}': {}",
                    config.name, e
                ))
            })?;

        let http_transport = HttpTransport {
            config,
            client,
            request_id: AtomicU64::new(1),
        };

        let mut conn = Self {
            transport: ConnectionTransport::Http(http_transport),
            tools: Arc::new(RwLock::new(Vec::new())),
        };

        // Initialize the connection
        conn.initialize().await?;

        // Discover tools
        conn.discover_tools().await?;

        Ok(conn)
    }

    /// I/O loop for handling communication with the STDIO subprocess
    async fn stdio_io_loop(
        mut stdin: ChildStdin,
        stdout: ChildStdout,
        mut request_rx: mpsc::Receiver<(JsonRpcRequest, oneshot::Sender<JsonRpcResponse>)>,
    ) {
        let mut reader = BufReader::new(stdout);
        let mut pending: HashMap<u64, PendingRequest> = HashMap::new();
        let mut line_buffer = String::new();

        loop {
            tokio::select! {
                // Handle outgoing requests
                Some((request, response_tx)) = request_rx.recv() => {
                    let id = request.id;
                    match serde_json::to_string(&request) {
                        Ok(json) => {
                            let msg = format!("{}\n", json);
                            if let Err(e) = stdin.write_all(msg.as_bytes()).await {
                                tracing::error!(error = %e, "Failed to write to MCP server");
                                let _ = response_tx.send(JsonRpcResponse {
                                    jsonrpc: "2.0".to_string(),
                                    id,
                                    result: None,
                                    error: Some(super::protocol::JsonRpcError {
                                        code: -32000,
                                        message: format!("Write error: {}", e),
                                        data: None,
                                    }),
                                });
                                continue;
                            }
                            if let Err(e) = stdin.flush().await {
                                tracing::error!(error = %e, "Failed to flush to MCP server");
                            }
                            // R3-H13: Cap pending requests to prevent unbounded growth
                            const MAX_PENDING: usize = 1000;
                            if pending.len() >= MAX_PENDING {
                                tracing::warn!("MCP pending request limit reached ({}), rejecting", MAX_PENDING);
                                let _ = response_tx.send(JsonRpcResponse {
                                    jsonrpc: "2.0".to_string(),
                                    id,
                                    result: None,
                                    error: Some(super::protocol::JsonRpcError {
                                        code: -32000,
                                        message: "Too many pending requests".to_string(),
                                        data: None,
                                    }),
                                });
                                continue;
                            }
                            pending.insert(id, PendingRequest { response_tx });
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to serialize request");
                            let _ = response_tx.send(JsonRpcResponse {
                                jsonrpc: "2.0".to_string(),
                                id,
                                result: None,
                                error: Some(super::protocol::JsonRpcError {
                                    code: -32700,
                                    message: format!("Serialization error: {}", e),
                                    data: None,
                                }),
                            });
                        }
                    }
                }

                // Handle incoming responses
                result = reader.read_line(&mut line_buffer) => {
                    match result {
                        Ok(0) => {
                            tracing::warn!("MCP server closed stdout");
                            break;
                        }
                        Ok(_) => {
                            let line = line_buffer.trim();
                            if !line.is_empty() {
                                match serde_json::from_str::<JsonRpcResponse>(line) {
                                    Ok(response) => {
                                        if let Some(pending_req) = pending.remove(&response.id) {
                                            let _ = pending_req.response_tx.send(response);
                                        }
                                    }
                                    Err(e) => {
                                        tracing::debug!(
                                            error = %e,
                                            line = %line,
                                            "Failed to parse response (might be notification)"
                                        );
                                    }
                                }
                            }
                            line_buffer.clear();
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Failed to read from MCP server");
                            break;
                        }
                    }
                }
            }
        }

        tracing::info!("MCP STDIO I/O loop terminated");
    }

    /// Send a JSON-RPC request and wait for response (dispatches to correct transport)
    async fn send_request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        match &self.transport {
            ConnectionTransport::Stdio(stdio) => {
                Self::send_stdio_request(stdio, method, params).await
            }
            ConnectionTransport::Http(http) => Self::send_http_request(http, method, params).await,
        }
    }

    /// Send a request via STDIO transport
    async fn send_stdio_request(
        stdio: &StdioTransport,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        let id = stdio.request_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);

        let (response_tx, response_rx) = oneshot::channel();

        stdio
            .request_tx
            .send((request, response_tx))
            .await
            .map_err(|_| Error::Internal("MCP request channel closed".to_string()))?;

        // R3-M: Use config timeout instead of hardcoded 30s
        let response = timeout(
            Duration::from_secs(stdio.config.request_timeout_secs),
            response_rx,
        )
        .await
        .map_err(|_| Error::Internal("MCP request timeout".to_string()))?
        .map_err(|_| Error::Internal("MCP response channel closed".to_string()))?;

        Ok(response)
    }

    /// Send a request via HTTP transport
    ///
    /// Posts the JSON-RPC request to the server's HTTP endpoint and parses the
    /// JSON-RPC response from the HTTP response body.
    async fn send_http_request(
        http: &HttpTransport,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse> {
        let id = http.request_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest::new(id, method, params);

        let mut request_builder = http
            .client
            .post(&http.config.url)
            .header("Content-Type", "application/json");

        if let Some(token) = &http.config.auth_token {
            request_builder = request_builder.header("Authorization", format!("Bearer {}", token));
        }

        let http_response = request_builder.json(&request).send().await.map_err(|e| {
            Error::Internal(format!(
                "HTTP request to MCP server '{}' failed: {}",
                http.config.name, e
            ))
        })?;

        let status = http_response.status();
        if !status.is_success() {
            let body = http_response.text().await.unwrap_or_default();
            return Err(Error::Internal(format!(
                "MCP server '{}' returned HTTP {}: {}",
                http.config.name, status, body
            )));
        }

        let response: JsonRpcResponse = http_response.json().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to parse JSON-RPC response from MCP server '{}': {}",
                http.config.name, e
            ))
        })?;

        Ok(response)
    }

    /// Initialize the MCP connection (works for both transports)
    async fn initialize(&mut self) -> Result<()> {
        let startup_timeout_secs = match &self.transport {
            ConnectionTransport::Stdio(s) => s.config.startup_timeout_secs,
            ConnectionTransport::Http(h) => h.config.startup_timeout_secs,
        };

        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "roots": { "listChanged": true }
            },
            "clientInfo": {
                "name": "canal",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let response = timeout(
            Duration::from_secs(startup_timeout_secs),
            self.send_request("initialize", Some(params)),
        )
        .await
        .map_err(|_| Error::Internal("MCP initialization timeout".to_string()))??;

        if let Some(error) = response.error {
            return Err(Error::Internal(format!(
                "MCP initialization failed: {}",
                error.message
            )));
        }

        // Send initialized notification
        let _ = self.send_request("notifications/initialized", None).await;

        tracing::info!(name = %self.name(), "MCP server initialized");
        Ok(())
    }

    /// Discover available tools from the server
    async fn discover_tools(&mut self) -> Result<()> {
        let response = self.send_request("tools/list", None).await?;

        if let Some(error) = response.error {
            tracing::warn!(
                name = %self.name(),
                error = %error.message,
                "Failed to list tools"
            );
            return Ok(());
        }

        if let Some(result) = response.result {
            if let Some(tools_value) = result.get("tools") {
                let tools: Vec<McpToolDef> = serde_json::from_value(tools_value.clone())
                    .map_err(|e| Error::Internal(format!("Failed to parse tools: {}", e)))?;

                tracing::info!(
                    name = %self.name(),
                    count = tools.len(),
                    "Discovered MCP tools"
                );

                let mut tools_guard = self.tools.write().await;
                *tools_guard = tools;
            }
        }

        Ok(())
    }

    /// Call a tool on this server
    pub async fn call_tool(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallResult> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments
        });

        let response = self.send_request("tools/call", Some(params)).await?;

        if let Some(error) = response.error {
            return Ok(ToolCallResult::error(error.message));
        }

        if let Some(result) = response.result {
            let tool_result: ToolCallResult = serde_json::from_value(result)
                .map_err(|e| Error::Internal(format!("Failed to parse tool result: {}", e)))?;
            return Ok(tool_result);
        }

        Ok(ToolCallResult::error("No result from tool call"))
    }

    /// Get the list of available tools
    pub async fn tools(&self) -> Vec<McpToolDef> {
        let tools = self.tools.read().await;
        tools.clone()
    }

    /// Get server name
    pub fn name(&self) -> &str {
        match &self.transport {
            ConnectionTransport::Stdio(s) => &s.config.name,
            ConnectionTransport::Http(h) => &h.config.name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_config_default() {
        let config = McpSpawnConfig::default();
        assert_eq!(config.startup_timeout_secs, 30);
        assert!(config.args.is_empty());
    }

    #[test]
    fn test_http_config_default() {
        let config = McpHttpConfig::default();
        assert_eq!(config.startup_timeout_secs, 30);
        assert_eq!(config.name, "unnamed");
        assert!(config.url.is_empty());
        assert!(config.auth_token.is_none());
    }

    #[test]
    fn test_http_config_with_auth_token() {
        let config = McpHttpConfig {
            name: "test".to_string(),
            url: "https://mcp.example.com/mcp".to_string(),
            startup_timeout_secs: 30,
            auth_token: Some("my-bearer-token".to_string()),
            request_timeout_secs: 30,
        };
        assert_eq!(config.auth_token.as_deref(), Some("my-bearer-token"));
    }
}
