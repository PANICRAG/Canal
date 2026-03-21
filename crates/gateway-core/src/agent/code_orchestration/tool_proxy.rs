//! Tool Proxy Bridge - HTTP server exposing tools to sandbox code
//!
//! Runs a lightweight HTTP server on a random port that sandbox code
//! can call via HTTP POST to invoke registered tools. Records all
//! tool calls for auditing and returns results as JSON.

use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use super::types::ToolCallRecord;
use crate::agent::tools::{ToolContext, ToolRegistry};
use crate::error::{Error, Result};

/// HTTP bridge that exposes tools to sandbox code
///
/// The bridge runs an axum HTTP server on a random port, providing
/// a `/call_tool` endpoint that sandbox code can POST to in order
/// to invoke any registered tool.
pub struct ToolProxyBridge {
    tool_registry: Arc<ToolRegistry>,
    recorded_calls: Arc<RwLock<Vec<ToolCallRecord>>>,
    port: u16,
    max_tool_calls: usize,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

/// Request body for the /call_tool endpoint
#[derive(Debug, serde::Deserialize)]
struct ToolCallRequest {
    tool_name: String,
    arguments: serde_json::Value,
}

/// Response body for the /call_tool endpoint
#[derive(Debug, serde::Serialize)]
struct ToolCallResponse {
    result: Option<serde_json::Value>,
    error: Option<String>,
}

/// Shared state for axum handlers
struct ProxyState {
    tool_registry: Arc<ToolRegistry>,
    recorded_calls: Arc<RwLock<Vec<ToolCallRecord>>>,
    tool_context: ToolContext,
    max_tool_calls: usize,
}

impl ToolProxyBridge {
    /// Create a new ToolProxyBridge (does not start the server yet)
    pub fn new(tool_registry: Arc<ToolRegistry>, max_tool_calls: usize) -> Self {
        Self {
            tool_registry,
            recorded_calls: Arc::new(RwLock::new(Vec::new())),
            port: 0,
            max_tool_calls,
            shutdown_tx: None,
        }
    }

    /// Start the HTTP proxy server on a random available port
    ///
    /// Returns the port number the server is listening on.
    pub async fn start(&mut self, tool_context: ToolContext) -> Result<u16> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| Error::ToolProxy(format!("Failed to bind proxy server: {}", e)))?;

        let addr = listener
            .local_addr()
            .map_err(|e| Error::ToolProxy(format!("Failed to get local addr: {}", e)))?;

        self.port = addr.port();

        let state = Arc::new(ProxyState {
            tool_registry: self.tool_registry.clone(),
            recorded_calls: self.recorded_calls.clone(),
            tool_context,
            max_tool_calls: self.max_tool_calls,
        });

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        self.shutdown_tx = Some(shutdown_tx);

        // Build the axum router
        let app = axum::Router::new()
            .route("/call_tool", axum::routing::post(handle_tool_call))
            .route("/health", axum::routing::get(handle_health))
            .with_state(state);

        // Spawn the server
        tokio::spawn(async move {
            let server = axum::serve(listener, app);
            tokio::select! {
                result = server => {
                    if let Err(e) = result {
                        tracing::error!("Tool proxy server error: {}", e);
                    }
                }
                _ = async {
                    let _ = shutdown_rx.await;
                } => {
                    tracing::debug!("Tool proxy server shutting down");
                }
            }
        });

        tracing::info!("Tool proxy bridge started on port {}", self.port);
        Ok(self.port)
    }

    /// Get the port the server is listening on
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Get all recorded tool calls
    pub async fn get_recorded_calls(&self) -> Vec<ToolCallRecord> {
        self.recorded_calls.read().await.clone()
    }

    /// Get the number of recorded tool calls
    pub async fn call_count(&self) -> usize {
        self.recorded_calls.read().await.len()
    }

    /// Shutdown the proxy server
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for ToolProxyBridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Handler for POST /call_tool
async fn handle_tool_call(
    axum::extract::State(state): axum::extract::State<Arc<ProxyState>>,
    axum::Json(request): axum::Json<ToolCallRequest>,
) -> axum::Json<ToolCallResponse> {
    let start = Instant::now();

    // Check tool call limit
    let current_count = state.recorded_calls.read().await.len();
    if current_count >= state.max_tool_calls {
        let record = ToolCallRecord::failure(
            &request.tool_name,
            request.arguments.clone(),
            format!(
                "Tool call limit exceeded: {} >= {}",
                current_count, state.max_tool_calls
            ),
            0,
        );
        state.recorded_calls.write().await.push(record);

        return axum::Json(ToolCallResponse {
            result: None,
            error: Some(format!(
                "Tool call limit exceeded ({}/{})",
                current_count, state.max_tool_calls
            )),
        });
    }

    // Execute the tool
    let result = state
        .tool_registry
        .execute(
            &request.tool_name,
            request.arguments.clone(),
            &state.tool_context,
        )
        .await;

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(value) => {
            let record = ToolCallRecord::success(
                &request.tool_name,
                request.arguments,
                value.clone(),
                duration_ms,
            );
            state.recorded_calls.write().await.push(record);

            axum::Json(ToolCallResponse {
                result: Some(value),
                error: None,
            })
        }
        Err(e) => {
            let error_msg = e.to_string();
            let record = ToolCallRecord::failure(
                &request.tool_name,
                request.arguments,
                &error_msg,
                duration_ms,
            );
            state.recorded_calls.write().await.push(record);

            axum::Json(ToolCallResponse {
                result: None,
                error: Some(error_msg),
            })
        }
    }
}

/// Handler for GET /health
async fn handle_health() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_proxy_bridge_creation() {
        let registry = Arc::new(ToolRegistry::new());
        let bridge = ToolProxyBridge::new(registry, 100);
        assert_eq!(bridge.port(), 0);
        assert_eq!(bridge.max_tool_calls, 100);
    }

    #[tokio::test]
    async fn test_tool_proxy_bridge_start() {
        let registry = Arc::new(ToolRegistry::new());
        let mut bridge = ToolProxyBridge::new(registry, 100);

        let context = ToolContext::new("test-session", std::path::Path::new("/tmp"));
        let port = bridge.start(context).await.unwrap();

        assert!(port > 0);
        assert_eq!(bridge.port(), port);

        // Verify health endpoint
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{}/health", port))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.text().await.unwrap(), "ok");

        bridge.shutdown();
    }

    #[tokio::test]
    async fn test_tool_proxy_bridge_call_tool() {
        let registry = Arc::new(ToolRegistry::new());
        let mut bridge = ToolProxyBridge::new(registry, 100);

        let temp_dir = tempfile::TempDir::new().unwrap();
        let test_file = temp_dir.path().join("test.txt");
        std::fs::write(&test_file, "Hello, World!").unwrap();

        let context = ToolContext::new("test-session", temp_dir.path())
            .with_allowed_directory(temp_dir.path());
        let port = bridge.start(context).await.unwrap();

        // Call the Read tool via HTTP
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/call_tool", port))
            .json(&serde_json::json!({
                "tool_name": "Read",
                "arguments": {
                    "file_path": test_file.to_string_lossy()
                }
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["error"].is_null());
        assert!(body["result"].is_object());

        // Verify call was recorded
        let calls = bridge.get_recorded_calls().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_name, "Read");

        bridge.shutdown();
    }

    #[tokio::test]
    async fn test_tool_proxy_bridge_call_limit() {
        let registry = Arc::new(ToolRegistry::new());
        let mut bridge = ToolProxyBridge::new(registry, 1); // Only 1 call allowed

        let context = ToolContext::new("test-session", std::path::Path::new("/tmp"));
        let port = bridge.start(context).await.unwrap();

        let client = reqwest::Client::new();

        // First call should succeed (though the tool might fail, the limit check passes)
        let _ = client
            .post(format!("http://127.0.0.1:{}/call_tool", port))
            .json(&serde_json::json!({
                "tool_name": "Glob",
                "arguments": {"pattern": "*.txt"}
            }))
            .send()
            .await
            .unwrap();

        // Second call should be rate-limited
        let resp = client
            .post(format!("http://127.0.0.1:{}/call_tool", port))
            .json(&serde_json::json!({
                "tool_name": "Glob",
                "arguments": {"pattern": "*.rs"}
            }))
            .send()
            .await
            .unwrap();

        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["error"].as_str().unwrap().contains("limit exceeded"));

        bridge.shutdown();
    }
}
