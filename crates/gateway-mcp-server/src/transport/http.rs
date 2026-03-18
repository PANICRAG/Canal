//! HTTP transport for MCP server
//!
//! Implements Streamable HTTP transport as defined in MCP spec.
//! Single endpoint: `POST /mcp`
//! Authentication via `Authorization: Bearer piga_sk_...` header.

use axum::{
    extract::State,
    http::HeaderMap,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;

use super::RequestHandler;
use crate::protocol::JsonRpcRequest;

/// Shared state for the HTTP transport
struct HttpState<H: RequestHandler> {
    handler: H,
    /// R9-H2: Expected API key for per-request auth enforcement.
    /// Empty string = no auth (dev mode).
    expected_api_key: String,
}

/// HTTP transport — axum server on configurable port
pub struct HttpTransport {
    port: u16,
}

impl HttpTransport {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// Run the HTTP server with the given request handler
    pub async fn run<H>(self, handler: H) -> anyhow::Result<()>
    where
        H: RequestHandler + Send + Sync + 'static,
    {
        // R9-H2: Load API key for per-request auth enforcement
        let expected_api_key = std::env::var("API_KEY").unwrap_or_default();
        let state = Arc::new(HttpState {
            handler,
            expected_api_key,
        });

        let app = Router::new()
            .route("/mcp", post(handle_mcp::<H>))
            .route("/health", get(handle_health))
            .with_state(state);

        let addr = format!("0.0.0.0:{}", self.port);
        info!("MCP HTTP transport listening on {}", addr);

        let listener = TcpListener::bind(&addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}

/// Handle POST /mcp — the main MCP endpoint
///
/// Extracts Bearer token from Authorization header and injects it
/// into the request's `clientInfo.apiKey` so the handler can authenticate.
async fn handle_mcp<H: RequestHandler>(
    State(state): State<Arc<HttpState<H>>>,
    headers: HeaderMap,
    Json(mut request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    // Extract API key from Authorization header
    let api_key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    // R9-H2: Per-request auth enforcement — verify Bearer token on EVERY request,
    // not just initialize. Reject unauthenticated requests with JSON-RPC error.
    if !state.expected_api_key.is_empty() {
        match api_key {
            None => {
                return Json(crate::protocol::JsonRpcResponse::error(
                    request.id,
                    -32001,
                    "Authentication required: missing Authorization header".to_string(),
                ));
            }
            Some(key) if key != state.expected_api_key => {
                return Json(crate::protocol::JsonRpcResponse::error(
                    request.id,
                    -32001,
                    "Authentication failed: invalid API key".to_string(),
                ));
            }
            _ => {} // valid key
        }
    }

    // For initialize requests, also inject the Bearer token into clientInfo.apiKey
    // so the handler's identity resolution flow picks it up
    if let Some(key) = api_key {
        if request.method == "initialize" {
            let params = request.params.get_or_insert_with(|| serde_json::json!({}));
            if let Some(obj) = params.as_object_mut() {
                let client_info = obj
                    .entry("clientInfo")
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(ci) = client_info.as_object_mut() {
                    ci.entry("apiKey")
                        .or_insert_with(|| serde_json::Value::String(key.to_string()));
                }
            }
        }
    }

    let response = state.handler.handle(request).await;
    Json(response)
}

/// Handle GET /health
async fn handle_health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "transport": "http",
        "protocol": "mcp",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::JsonRpcResponse;

    struct EchoHandler;

    #[async_trait::async_trait]
    impl RequestHandler for EchoHandler {
        async fn handle(&self, request: JsonRpcRequest) -> JsonRpcResponse {
            JsonRpcResponse::success(
                request.id,
                serde_json::json!({
                    "method": request.method,
                    "echo": true
                }),
            )
        }
    }

    #[tokio::test]
    async fn test_http_transport_creates() {
        let transport = HttpTransport::new(4100);
        assert_eq!(transport.port, 4100);
    }
}
