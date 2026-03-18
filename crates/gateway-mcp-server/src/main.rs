//! AI Gateway MCP Server
//!
//! Exposes the AI Gateway as an MCP server, allowing LLM clients
//! to use the gateway's capabilities as tools.
//!
//! Supports two transports:
//! - STDIO (default): JSON-RPC over stdin/stdout
//! - HTTP: Streamable HTTP on port 4100 (MCP_TRANSPORT=http)

use anyhow::Result;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tracing::info;

mod catalog;
mod dispatcher;
mod handlers;
mod protocol;
mod transport;

use catalog::build_default_catalog;
use dispatcher::Dispatcher;
use handlers::HandlerContext;
use canal_identity::service::IdentityService;
use canal_identity::store::DashMapKeyStore;
use protocol::{JsonRpcRequest, JsonRpcResponse};
use transport::stdio::StdioTransport;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging (to stderr for MCP — stdout is the transport)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("gateway_mcp_server=info".parse()?),
        )
        .with_writer(io::stderr)
        .init();

    info!("Starting AI Gateway MCP Server");

    // Load environment variables
    dotenvy::dotenv().ok();

    // Initialize identity service
    let system_key = std::env::var("API_KEY").unwrap_or_default();
    let key_store = if system_key.is_empty() {
        tracing::warn!("API_KEY is not set — MCP server running without authentication. Set API_KEY (min 16 chars) for production use.");
        DashMapKeyStore::new()
    } else if system_key.len() < 16 {
        anyhow::bail!(
            "API_KEY is too short ({} chars). Minimum length is 16 characters for security. \
             Unset API_KEY entirely for unauthenticated dev mode, or provide a strong key.",
            system_key.len()
        );
    } else {
        let key_hash = canal_identity::key_gen::hash_key(&system_key);
        let key_prefix = if system_key.len() >= 8 {
            system_key[..8].to_string()
        } else {
            system_key.clone()
        };
        DashMapKeyStore::with_system_key(&key_hash, &key_prefix)
    };
    let identity_service = Arc::new(IdentityService::new(Arc::new(key_store)));

    // Build tool catalog
    let catalog = build_default_catalog();

    // Initialize LLM router and tool system (optional — may fail if no providers configured)
    let (llm_router, tool_system, capabilities) = init_llm().await;

    // Initialize billing-core services (metering + billing)
    let (metering, billing) = {
        use billing_core::store::memory::{InMemoryBalanceStore, InMemoryEventStore};
        use billing_core::{BillingService, MeteringService, PlanRegistry, PricingEngine};

        let pricing = Arc::new(PricingEngine::with_defaults());
        let plans = Arc::new(PlanRegistry::with_defaults());
        let balance = Arc::new(InMemoryBalanceStore::new());
        let events = Arc::new(InMemoryEventStore::new());
        let billing = Arc::new(BillingService::new(balance, events, pricing.clone(), plans));
        let metering = Arc::new(MeteringService::new(pricing, billing.clone()));
        (metering, billing)
    };
    info!("Billing-core services initialized (metering + billing)");

    // Initialize runtime managers
    let sandbox_manager = Arc::new(canal_runtime::sandbox::SandboxManager::default());
    let browser_manager = Arc::new(canal_runtime::browser::BrowserSessionManager::default());
    info!("Runtime managers initialized (sandbox + browser)");

    // Create handler context
    let handler_ctx = Arc::new(
        HandlerContext::new(llm_router, tool_system, capabilities, metering, billing)
            .with_sandbox_manager(sandbox_manager)
            .with_browser_manager(browser_manager),
    );

    // Create dispatcher
    let dispatcher = Arc::new(Dispatcher::new(
        catalog,
        identity_service.clone(),
        handler_ctx,
    ));

    // Select transport based on MCP_TRANSPORT env var
    let transport = std::env::var("MCP_TRANSPORT").unwrap_or_else(|_| "stdio".to_string());

    match transport.as_str() {
        "stdio" => {
            info!("Using STDIO transport");
            let server = McpRequestHandler::new(dispatcher, identity_service);
            let stdio = StdioTransport::new();
            stdio.run(server).await?;
        }
        "http" => {
            let port: u16 = std::env::var("MCP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(4100);
            info!("Using HTTP transport on port {}", port);
            let server = McpRequestHandler::new(dispatcher, identity_service);
            let http = transport::http::HttpTransport::new(port);
            http.run(server).await?;
        }
        other => {
            anyhow::bail!("Unknown transport: {}. Use 'stdio' or 'http'.", other);
        }
    }

    Ok(())
}

/// Initialize LLM router and tool system directly (no canal-engine facade).
async fn init_llm() -> (
    Option<Arc<tokio::sync::RwLock<gateway_core::llm::router::LlmRouter>>>,
    Option<Arc<gateway_core::tool_system::ToolSystem>>,
    serde_json::Value,
) {
    use gateway_core::llm::router::LlmRouter;
    use gateway_core::tool_system::ToolSystem;
    use tokio::sync::RwLock;

    let llm_config = gateway_core::llm::LlmConfig::default();
    let mut llm_router = LlmRouter::new(llm_config);

    // Register providers from env vars (shared helper)
    let registered = gateway_core::llm::register_providers_from_env(&mut llm_router);
    if registered.is_empty() {
        info!(
            "No LLM providers configured — LLM router will not be available. \
             Set at least one of: QWEN_API_KEY, ANTHROPIC_API_KEY, OPENAI_API_KEY, GOOGLE_AI_API_KEY"
        );
        let capabilities = serde_json::json!({
            "execution_modes": ["direct"],
            "models": [],
            "tool_namespaces": [],
            "browser_automation": false,
            "code_execution": false,
            "mcp_proxy": false,
            "version": env!("CARGO_PKG_VERSION")
        });
        return (None, None, capabilities);
    }
    info!(
        "Registered {} LLM providers: {:?}",
        registered.len(),
        registered
    );

    let tool_system = ToolSystem::new();

    // Build model list from registered providers
    let mut models = Vec::new();
    for name in &registered {
        match name.as_str() {
            "qwen" => models.push("qwen3-max-2026-01-23"),
            "anthropic" => models.push("claude-sonnet-4-5-20250929"),
            "openai" => models.push("gpt-4o"),
            "google" => models.push("gemini-2.0-flash"),
            _ => {}
        }
    }

    let capabilities = serde_json::json!({
        "execution_modes": ["direct", "plan_execute", "swarm", "expert", "auto"],
        "models": models,
        "tool_namespaces": [],
        "browser_automation": false,
        "code_execution": false,
        "mcp_proxy": false,
        "version": env!("CARGO_PKG_VERSION")
    });

    info!("LLM Router initialized directly (no engine facade)");
    (
        Some(Arc::new(RwLock::new(llm_router))),
        Some(Arc::new(tool_system)),
        capabilities,
    )
}

/// MCP request handler — implements the transport::RequestHandler trait
///
/// ## R9-H3: Shared Identity Limitation
///
/// **Security note**: A single `AgentIdentity` is stored in `current_identity` and set
/// once during the `initialize` handshake. All concurrent HTTP requests share this one
/// identity, meaning concurrent clients are indistinguishable from each other.
///
/// **Intended fix** (requires auth rearchitecture):
/// - Each HTTP request should carry its own auth token (e.g. `Authorization: Bearer ...`).
/// - Identity should be resolved per-request from the token, not shared globally.
/// - The `handle` method should extract the token from the request context and call
///   `IdentityService::resolve()` independently for each request.
///
/// Until that rearchitecture, this struct logs a warning when concurrent request usage
/// is detected so the limitation is visible in production logs.
pub struct McpRequestHandler {
    dispatcher: Arc<Dispatcher>,
    identity_service: Arc<IdentityService>,
    /// Current session identity (set during initialize).
    /// WARNING (R9-H3): This identity is shared across ALL concurrent requests.
    /// It is NOT per-request. See struct-level doc comment for the intended fix.
    current_identity: tokio::sync::RwLock<Option<canal_identity::types::AgentIdentity>>,
    /// Tracks in-flight requests to detect concurrent usage of the shared identity.
    inflight_requests: AtomicU64,
    /// Whether we have already emitted the shared-identity warning (log once).
    shared_identity_warned: AtomicBool,
}

impl McpRequestHandler {
    pub fn new(dispatcher: Arc<Dispatcher>, identity_service: Arc<IdentityService>) -> Self {
        Self {
            dispatcher,
            identity_service,
            current_identity: tokio::sync::RwLock::new(None),
            inflight_requests: AtomicU64::new(0),
            shared_identity_warned: AtomicBool::new(false),
        }
    }

    async fn handle_initialize(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        // Extract API key from initialize params (if provided)
        if let Some(params) = &request.params {
            if let Some(api_key) = params
                .get("clientInfo")
                .and_then(|ci| ci.get("apiKey"))
                .and_then(|k| k.as_str())
            {
                match self.dispatcher.resolve_identity(api_key).await {
                    Ok(identity) => {
                        info!(agent = %identity.name, "Agent authenticated");
                        *self.current_identity.write().await = Some(identity);
                    }
                    Err(e) => {
                        return JsonRpcResponse::error(
                            request.id,
                            -32000,
                            format!("Authentication failed: {}", e),
                        );
                    }
                }
            }
        }

        // If no API key provided, use system identity from env
        if self.current_identity.read().await.is_none() {
            let system_key = std::env::var("API_KEY").unwrap_or_default();
            if !system_key.is_empty() {
                if let Ok(identity) = self.dispatcher.resolve_identity(&system_key).await {
                    info!("Using system API key for authentication");
                    *self.current_identity.write().await = Some(identity);
                }
            }
        }

        // R9-C3: Reject unauthenticated requests instead of silently granting Trial access.
        // Only allow default trial identity in explicit dev mode.
        if self.current_identity.read().await.is_none() {
            let is_dev = matches!(
                std::env::var("CANAL_ENV").as_deref(),
                Ok("development") | Ok("dev")
            );
            if is_dev {
                info!("Dev mode: using default trial identity (no API key)");
                let default_identity = canal_identity::types::AgentIdentity {
                    id: uuid::Uuid::new_v4(),
                    name: "anonymous-dev".to_string(),
                    tier: canal_identity::types::AgentTier::Trial,
                    scopes: canal_identity::types::AgentTier::Trial.default_scopes(),
                    created_at: chrono::Utc::now(),
                };
                *self.current_identity.write().await = Some(default_identity);
            } else {
                tracing::warn!("No valid API key — rejecting initialization");
                return JsonRpcResponse::error(
                    request.id,
                    -32001,
                    "Authentication required: set API_KEY environment variable".to_string(),
                );
            }
        }

        JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {},
                    "resources": {},
                    "prompts": {}
                },
                "serverInfo": {
                    "name": "canal",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
    }

    async fn handle_tools_list(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let identity = self.current_identity.read().await;
        let tools = match identity.as_ref() {
            Some(id) => self.dispatcher.list_tools(id),
            None => vec![], // No identity = no tools
        };

        // Convert to MCP format
        let mcp_tools: Vec<serde_json::Value> = tools
            .into_iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema
                })
            })
            .collect();

        JsonRpcResponse::success(request.id, serde_json::json!({ "tools": mcp_tools }))
    }

    async fn handle_tools_call(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let params = request.params.unwrap_or(serde_json::Value::Null);
        let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let identity = self.current_identity.read().await;
        let identity = match identity.as_ref() {
            Some(id) => id,
            None => {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": "Error: Not authenticated. Call initialize first."
                        }],
                        "isError": true
                    }),
                );
            }
        };

        match self
            .dispatcher
            .dispatch(identity, tool_name, arguments)
            .await
        {
            Ok(result) => {
                let text = serde_json::to_string_pretty(&result).unwrap_or_default();
                JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": text
                        }]
                    }),
                )
            }
            Err(e) => JsonRpcResponse::success(
                request.id,
                serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": format!("Error: {}", e)
                    }],
                    "isError": true
                }),
            ),
        }
    }

    async fn handle_resources_list(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "resources": [
                    {
                        "uri": "canal://status",
                        "name": "Platform Status",
                        "description": "Current platform status and health information",
                        "mimeType": "application/json"
                    },
                    {
                        "uri": "canal://models",
                        "name": "Available Models",
                        "description": "List of available LLM models and their capabilities",
                        "mimeType": "application/json"
                    }
                ]
            }),
        )
    }

    async fn handle_resources_read(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let params = request.params.unwrap_or(serde_json::Value::Null);
        let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");

        let content = match uri {
            "canal://status" => serde_json::json!({
                "status": "healthy",
                "engine": if self.dispatcher.list_tools(
                    &canal_identity::types::AgentIdentity {
                        id: uuid::Uuid::nil(),
                        name: "system".to_string(),
                        tier: canal_identity::types::AgentTier::System,
                        scopes: canal_identity::types::AgentTier::System.default_scopes(),
                        created_at: chrono::Utc::now(),
                    }
                ).is_empty() { "no_tools" } else { "available" },
                "timestamp": chrono::Utc::now().to_rfc3339()
            }),
            "canal://models" => serde_json::json!({
                "models": ["claude-sonnet-4-5-20250929", "gpt-4o", "qwen-turbo"],
                "note": "Dynamic model list from engine capabilities"
            }),
            _ => {
                return JsonRpcResponse::error(
                    request.id,
                    -32002,
                    format!("Resource not found: {}", uri),
                );
            }
        };

        JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "contents": [{
                    "uri": uri,
                    "mimeType": "application/json",
                    "text": serde_json::to_string_pretty(&content).unwrap_or_default()
                }]
            }),
        )
    }

    async fn handle_prompts_list(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "prompts": [
                    {
                        "name": "chat",
                        "description": "Send a message to the AI gateway",
                        "arguments": [
                            {
                                "name": "message",
                                "description": "The message to send",
                                "required": true
                            },
                            {
                                "name": "model",
                                "description": "Model to use",
                                "required": false
                            }
                        ]
                    }
                ]
            }),
        )
    }

    async fn handle_prompts_get(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let params = request.params.unwrap_or(serde_json::Value::Null);
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");

        match name {
            "chat" => {
                let message = params
                    .get("arguments")
                    .and_then(|a| a.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Hello");

                JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({
                        "messages": [{
                            "role": "user",
                            "content": {
                                "type": "text",
                                "text": message
                            }
                        }]
                    }),
                )
            }
            _ => JsonRpcResponse::error(request.id, -32002, format!("Prompt not found: {}", name)),
        }
    }
}

#[async_trait::async_trait]
impl transport::RequestHandler for McpRequestHandler {
    async fn handle(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        // R9-H3: Track in-flight requests. When >1 request is being served
        // concurrently, all share the same identity — log a warning so this
        // limitation is visible in production.
        let prev = self.inflight_requests.fetch_add(1, Ordering::Relaxed);
        if prev > 0 && !self.shared_identity_warned.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                concurrent_requests = prev + 1,
                "R9-H3: Multiple concurrent requests detected, but all share a single \
                 identity set during initialize. Concurrent clients are indistinguishable. \
                 Each request should carry its own auth token and resolve identity per-request."
            );
        }

        let response = match request.method.as_str() {
            "initialize" => self.handle_initialize(request).await,
            "tools/list" => self.handle_tools_list(request).await,
            "tools/call" => self.handle_tools_call(request).await,
            "resources/list" => self.handle_resources_list(request).await,
            "resources/read" => self.handle_resources_read(request).await,
            "prompts/list" => self.handle_prompts_list(request).await,
            "prompts/get" => self.handle_prompts_get(request).await,
            "ping" => JsonRpcResponse::success(request.id, serde_json::json!({})),
            "notifications/initialized" => {
                // Client notification — no response needed but we return success
                JsonRpcResponse::success(request.id, serde_json::json!({}))
            }
            _ => JsonRpcResponse::error(
                request.id,
                -32601,
                format!("Method not found: {}", request.method),
            ),
        };

        self.inflight_requests.fetch_sub(1, Ordering::Relaxed);
        response
    }
}
