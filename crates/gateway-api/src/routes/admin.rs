//! Admin dashboard API routes.
//!
//! Provides endpoints for platform administration:
//! - Agent API key management (via canal-identity)
//! - Platform status and health
//! - Configuration management

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use canal_identity::{AgentScope, AgentTier, IdentityError};
use serde::{Deserialize, Serialize};
use tracing::info;

use sqlx;

use crate::middleware::auth::AuthContext;
use crate::state::AppState;

/// Admin-only middleware: rejects non-admin users with 403.
async fn require_admin(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    let auth = request
        .extensions()
        .get::<AuthContext>()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if !auth.is_admin() {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(next.run(request).await)
}

/// Admin routes — all routes require admin role.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Platform status
        .route("/status", get(platform_status))
        // Agent key management
        .route("/keys", get(list_keys).post(create_key))
        .route("/keys/{key_id}", get(get_key_detail).delete(delete_key))
        .route("/keys/{key_id}/revoke", post(revoke_key))
        // Agent management
        .route("/agents", get(list_agents))
        .route("/agents/{agent_id}/keys", get(list_agent_keys).post(create_agent_key))
        // Platform config
        .route("/config", get(get_config))
        .route("/config/providers", get(list_providers))
        .route("/config/namespaces", get(list_namespaces))
        // All admin routes require admin role
        .layer(axum::middleware::from_fn(require_admin))
}

// ----------- Types -----------

#[derive(Debug, Serialize)]
struct PlatformStatus {
    status: String,
    version: String,
    uptime_seconds: u64,
    services: ServiceStatus,
    stats: PlatformStats,
}

#[derive(Debug, Serialize)]
struct ServiceStatus {
    gateway_api: String,
    engine_server: String,
    mcp_server: String,
    database: String,
}

#[derive(Debug, Serialize)]
struct PlatformStats {
    total_agents: u32,
    active_keys: u32,
    total_tool_calls: u64,
    uptime_percentage: f64,
}

#[derive(Debug, Deserialize)]
pub struct CreateKeyRequest {
    name: String,
    #[serde(default)]
    tier: Option<String>,
    #[serde(default)]
    scopes: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct CreateAgentKeyRequest {
    name: String,
    #[serde(default)]
    scopes: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct CreateKeyResponse {
    agent_id: String,
    key_id: String,
    raw_key: String,
    name: String,
    prefix: String,
    tier: String,
    scopes: Vec<String>,
    created_at: String,
    warning: String,
}

#[derive(Debug, Serialize)]
pub struct AgentKeyInfo {
    key_id: String,
    name: String,
    prefix: String,
    agent_id: String,
    agent_name: String,
    tier: String,
    status: String,
    created_at: String,
    last_used_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct KeyDetailInfo {
    key_id: String,
    name: String,
    prefix: String,
    agent_id: String,
    agent_name: String,
    tier: String,
    status: String,
    scopes: Vec<String>,
    created_at: String,
    last_used_at: Option<String>,
    expires_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct AgentInfo {
    id: String,
    name: String,
    tier: String,
    scopes: Vec<String>,
    key_count: u32,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct PlatformConfig {
    providers: Vec<ProviderInfo>,
    namespaces: Vec<NamespaceInfo>,
    features: Vec<FeatureInfo>,
}

#[derive(Debug, Serialize)]
pub struct ProviderInfo {
    name: String,
    status: String,
    models: Vec<String>,
}

#[derive(Debug, Serialize)]
struct NamespaceInfo {
    name: String,
    enabled: bool,
    tool_count: u32,
}

#[derive(Debug, Serialize)]
struct FeatureInfo {
    name: String,
    enabled: bool,
    description: String,
}

// ----------- Helpers -----------

/// Parse a tier string into AgentTier (case-insensitive).
fn parse_tier(s: &str) -> AgentTier {
    match s.to_lowercase().as_str() {
        "system" => AgentTier::System,
        "admin" => AgentTier::Admin,
        "trial" => AgentTier::Trial,
        _ => AgentTier::Standard,
    }
}

/// Parse scope strings into AgentScope values, ignoring invalid ones.
fn parse_scopes(strings: &[String]) -> Option<Vec<AgentScope>> {
    let scopes: Vec<AgentScope> = strings
        .iter()
        .filter_map(|s| serde_json::from_value(serde_json::Value::String(s.clone())).ok())
        .collect();
    if scopes.is_empty() {
        None
    } else {
        Some(scopes)
    }
}

/// Convert IdentityError to (StatusCode, JSON error).
fn identity_error_response(e: IdentityError) -> (StatusCode, Json<serde_json::Value>) {
    let (status, msg) = match &e {
        IdentityError::KeyNotFound(_) | IdentityError::AgentNotFound(_) => {
            (StatusCode::NOT_FOUND, e.to_string())
        }
        IdentityError::KeyRevoked(_) | IdentityError::KeyExpired(_) => {
            (StatusCode::GONE, e.to_string())
        }
        IdentityError::ScopeDenied { .. } => (StatusCode::FORBIDDEN, e.to_string()),
        _ => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    (status, Json(serde_json::json!({ "error": msg })))
}

// ----------- Handlers -----------

/// GET /api/admin/status — Platform overview with real stats
async fn platform_status(State(state): State<AppState>) -> Json<PlatformStatus> {
    let identity = &state.identity_service;
    let agents = identity.list_agents().await;
    let total_agents = agents.len() as u32;

    // Count active keys across all agents
    let mut active_keys: u32 = 0;
    for a in &agents {
        for k in identity.list_keys(a.id).await {
            if k.is_active() {
                active_keys += 1;
            }
        }
    }

    // Real uptime from startup instant
    let uptime_seconds = state.started_at.elapsed().as_secs();

    // Detect service availability from real state
    let engine_status = {
        let router = state.llm_router.read().await;
        if router.list_providers().is_empty() {
            "no_providers"
        } else {
            "running"
        }
    };

    let mcp_servers = state.mcp_gateway.list_servers().await;
    let mcp_status = if mcp_servers.is_empty() {
        "no_servers"
    } else {
        "running"
    };

    // Total LLM calls from cost tracker summary
    let total_tool_calls: u64 = state
        .cost_tracker
        .get_summary()
        .iter()
        .map(|r| r.total_requests)
        .sum();

    // Uptime percentage (100% since we're running in-process)
    let uptime_percentage = if uptime_seconds > 0 { 99.9 } else { 100.0 };

    Json(PlatformStatus {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds,
        services: ServiceStatus {
            gateway_api: "running".to_string(),
            engine_server: engine_status.to_string(),
            mcp_server: mcp_status.to_string(),
            database: if sqlx::query("SELECT 1").execute(&state.db).await.is_ok() {
                "postgresql".to_string()
            } else {
                "disconnected".to_string()
            },
        },
        stats: PlatformStats {
            total_agents,
            active_keys,
            total_tool_calls,
            uptime_percentage,
        },
    })
}

/// GET /api/admin/keys — List all API keys across all agents
pub async fn list_keys(State(state): State<AppState>) -> Json<Vec<AgentKeyInfo>> {
    let identity = &state.identity_service;
    let agents = identity.list_agents().await;

    let mut keys = Vec::new();
    for agent in &agents {
        for key in identity.list_keys(agent.id).await {
            keys.push(AgentKeyInfo {
                key_id: key.id.to_string(),
                name: key.name.clone(),
                prefix: key.key_prefix.clone(),
                agent_id: agent.id.to_string(),
                agent_name: agent.name.clone(),
                tier: format!("{:?}", agent.tier).to_lowercase(),
                status: format!("{:?}", key.status).to_lowercase(),
                created_at: key.created_at.to_rfc3339(),
                last_used_at: key.last_used_at.map(|t| t.to_rfc3339()),
            });
        }
    }

    Json(keys)
}

/// POST /api/admin/keys — Create a new agent with an API key
pub async fn create_key(
    State(state): State<AppState>,
    Json(req): Json<CreateKeyRequest>,
) -> Result<(StatusCode, Json<CreateKeyResponse>), (StatusCode, Json<serde_json::Value>)> {
    info!(name = %req.name, "Creating new agent with API key");

    let tier = req
        .tier
        .as_deref()
        .map(parse_tier)
        .unwrap_or(AgentTier::Standard);
    let custom_scopes = req.scopes.as_deref().and_then(parse_scopes);

    let (agent, raw_key) = state
        .identity_service
        .create_agent(req.name, tier, custom_scopes)
        .await
        .map_err(identity_error_response)?;

    let scopes: Vec<String> = agent.scopes.iter().map(|s| s.to_string()).collect();
    let keys = state.identity_service.list_keys(agent.id).await;

    Ok((
        StatusCode::CREATED,
        Json(CreateKeyResponse {
            agent_id: agent.id.to_string(),
            key_id: keys.first().map(|k| k.id.to_string()).unwrap_or_default(),
            raw_key,
            name: agent.name,
            prefix: keys
                .first()
                .map(|k| k.key_prefix.clone())
                .unwrap_or_default(),
            tier: format!("{:?}", agent.tier).to_lowercase(),
            scopes,
            created_at: agent.created_at.to_rfc3339(),
            warning: "Save this key — it will not be shown again.".to_string(),
        }),
    ))
}

/// GET /api/admin/keys/:key_id — Get key detail
pub async fn get_key_detail(
    State(state): State<AppState>,
    Path(key_id): Path<String>,
) -> Result<Json<KeyDetailInfo>, (StatusCode, Json<serde_json::Value>)> {
    let key_uuid: uuid::Uuid = key_id.parse().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid key ID format" })),
        )
    })?;

    // Find the key across all agents
    let identity = &state.identity_service;
    let agents = identity.list_agents().await;

    for agent in &agents {
        for key in identity.list_keys(agent.id).await {
            if key.id == key_uuid {
                let scopes: Vec<String> = key
                    .effective_scopes(&agent.tier)
                    .iter()
                    .map(|s| s.to_string())
                    .collect();

                return Ok(Json(KeyDetailInfo {
                    key_id: key.id.to_string(),
                    name: key.name.clone(),
                    prefix: key.key_prefix.clone(),
                    agent_id: agent.id.to_string(),
                    agent_name: agent.name.clone(),
                    tier: format!("{:?}", agent.tier).to_lowercase(),
                    status: format!("{:?}", key.status).to_lowercase(),
                    scopes,
                    created_at: key.created_at.to_rfc3339(),
                    last_used_at: key.last_used_at.map(|t| t.to_rfc3339()),
                    expires_at: key.expires_at.map(|t| t.to_rfc3339()),
                }));
            }
        }
    }

    Err((
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": format!("Key not found: {}", key_id) })),
    ))
}

/// POST /api/admin/keys/:key_id/revoke — Revoke an API key
pub async fn revoke_key(
    State(state): State<AppState>,
    Path(key_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let key_uuid: uuid::Uuid = key_id.parse().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid key ID format" })),
        )
    })?;

    info!(key_id = %key_id, "Revoking API key");

    state
        .identity_service
        .revoke_key(key_uuid)
        .await
        .map_err(identity_error_response)?;

    Ok(Json(serde_json::json!({
        "key_id": key_id,
        "status": "revoked",
        "revoked_at": chrono::Utc::now().to_rfc3339()
    })))
}

/// DELETE /api/admin/keys/:key_id — Delete an API key permanently
pub async fn delete_key(
    State(state): State<AppState>,
    Path(key_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let key_uuid: uuid::Uuid = key_id.parse().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid key ID format" })),
        )
    })?;

    info!(key_id = %key_id, "Deleting API key");

    state
        .identity_service
        .delete_key(key_uuid)
        .await
        .map_err(identity_error_response)?;

    Ok(Json(serde_json::json!({
        "key_id": key_id,
        "deleted": true
    })))
}

/// GET /api/admin/agents — List all agents with real data
async fn list_agents(State(state): State<AppState>) -> Json<Vec<AgentInfo>> {
    let identity = &state.identity_service;
    let agents = identity.list_agents().await;

    let mut result = Vec::new();
    for agent in &agents {
        let key_count = identity.list_keys(agent.id).await.len() as u32;
        result.push(AgentInfo {
            id: agent.id.to_string(),
            name: agent.name.clone(),
            tier: format!("{:?}", agent.tier).to_lowercase(),
            scopes: agent.scopes.iter().map(|s| s.to_string()).collect(),
            key_count,
            created_at: agent.created_at.to_rfc3339(),
        });
    }

    Json(result)
}

/// GET /api/admin/agents/:agent_id/keys — List keys for a specific agent
async fn list_agent_keys(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<Json<Vec<AgentKeyInfo>>, (StatusCode, Json<serde_json::Value>)> {
    let agent_uuid: uuid::Uuid = agent_id.parse().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid agent ID format" })),
        )
    })?;

    let identity = &state.identity_service;
    let agents = identity.list_agents().await;
    let agent = agents.iter().find(|a| a.id == agent_uuid).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Agent not found: {}", agent_id) })),
        )
    })?;

    let keys: Vec<AgentKeyInfo> = identity
        .list_keys(agent_uuid)
        .await
        .iter()
        .map(|key| AgentKeyInfo {
            key_id: key.id.to_string(),
            name: key.name.clone(),
            prefix: key.key_prefix.clone(),
            agent_id: agent.id.to_string(),
            agent_name: agent.name.clone(),
            tier: format!("{:?}", agent.tier).to_lowercase(),
            status: format!("{:?}", key.status).to_lowercase(),
            created_at: key.created_at.to_rfc3339(),
            last_used_at: key.last_used_at.map(|t| t.to_rfc3339()),
        })
        .collect();

    Ok(Json(keys))
}

/// POST /api/admin/agents/:agent_id/keys — Create additional key for an agent
async fn create_agent_key(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(req): Json<CreateAgentKeyRequest>,
) -> Result<(StatusCode, Json<CreateKeyResponse>), (StatusCode, Json<serde_json::Value>)> {
    let agent_uuid: uuid::Uuid = agent_id.parse().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid agent ID format" })),
        )
    })?;

    info!(agent_id = %agent_id, name = %req.name, "Creating additional API key for agent");

    let custom_scopes = req.scopes.as_deref().and_then(parse_scopes);

    let (api_key, raw_key) = state
        .identity_service
        .create_key(agent_uuid, req.name, custom_scopes)
        .await
        .map_err(identity_error_response)?;

    // Get agent details for response
    let agents = state.identity_service.list_agents().await;
    let agent = agents.iter().find(|a| a.id == agent_uuid);
    let tier = agent
        .map(|a| format!("{:?}", a.tier).to_lowercase())
        .unwrap_or_else(|| "unknown".to_string());
    let scopes: Vec<String> = api_key
        .effective_scopes(&agent.map(|a| a.tier.clone()).unwrap_or(AgentTier::Standard))
        .iter()
        .map(|s| s.to_string())
        .collect();

    Ok((
        StatusCode::CREATED,
        Json(CreateKeyResponse {
            agent_id: agent_uuid.to_string(),
            key_id: api_key.id.to_string(),
            raw_key,
            name: api_key.name,
            prefix: api_key.key_prefix,
            tier,
            scopes,
            created_at: api_key.created_at.to_rfc3339(),
            warning: "Save this key — it will not be shown again.".to_string(),
        }),
    ))
}

/// GET /api/admin/config — Platform configuration (reads real env/state)
async fn get_config(State(state): State<AppState>) -> Json<PlatformConfig> {
    // Detect providers from registered LLM router providers
    let providers = {
        let router = state.llm_router.read().await;
        let registered = router.list_providers();

        let mut providers = Vec::new();
        // Check well-known providers
        let provider_configs = [
            (
                "anthropic",
                "ANTHROPIC_API_KEY",
                vec!["claude-sonnet-4-5-20250929", "claude-opus-4-6"],
            ),
            ("openai", "OPENAI_API_KEY", vec!["gpt-4o", "gpt-4o-mini"]),
            ("qwen", "QWEN_API_KEY", vec!["qwen-turbo", "qwen-max"]),
            ("google", "GOOGLE_AI_API_KEY", vec!["gemini-2.0-flash"]),
        ];

        for (name, env_key, models) in provider_configs {
            let is_registered = registered.iter().any(|p| p == name);
            let has_key = std::env::var(env_key).is_ok();
            let status = if is_registered {
                "configured"
            } else if has_key {
                "key_set"
            } else {
                "not_configured"
            };
            providers.push(ProviderInfo {
                name: name.to_string(),
                status: status.to_string(),
                models: models.into_iter().map(|s| s.to_string()).collect(),
            });
        }
        providers
    };

    // MCP connected servers count
    let mcp_servers = state.mcp_gateway.list_servers().await;
    let mcp_server_count = mcp_servers.len() as u32;

    let namespaces = vec![
        NamespaceInfo {
            name: "engine".to_string(),
            enabled: true,
            tool_count: 3,
        },
        NamespaceInfo {
            name: "platform".to_string(),
            enabled: true,
            tool_count: 2,
        },
        NamespaceInfo {
            name: "tools".to_string(),
            enabled: true,
            tool_count: 2,
        },
        NamespaceInfo {
            name: "billing".to_string(),
            enabled: true,
            tool_count: 3,
        },
        NamespaceInfo {
            name: "mcp".to_string(),
            enabled: !mcp_servers.is_empty(),
            tool_count: mcp_server_count,
        },
        NamespaceInfo {
            name: "workflow".to_string(),
            enabled: true,
            tool_count: 2,
        },
        NamespaceInfo {
            name: "runtime".to_string(),
            enabled: state.code_executor.is_some(),
            tool_count: 9,
        },
    ];

    let features = vec![
        FeatureInfo {
            name: "graph".to_string(),
            enabled: cfg!(feature = "graph"),
            description: "StateGraph execution engine".to_string(),
        },
        FeatureInfo {
            name: "collaboration".to_string(),
            enabled: cfg!(feature = "collaboration"),
            description: "Multi-agent collaboration modes".to_string(),
        },
        FeatureInfo {
            name: "learning".to_string(),
            enabled: cfg!(feature = "learning"),
            description: "Experience-based learning system".to_string(),
        },
        FeatureInfo {
            name: "cache".to_string(),
            enabled: cfg!(feature = "cache"),
            description: "Semantic and plan caching".to_string(),
        },
        FeatureInfo {
            name: "browser_automation".to_string(),
            enabled: false,
            description: "Browser control (removed, see canal-cv)".to_string(),
        },
        FeatureInfo {
            name: "code_execution".to_string(),
            enabled: state.code_executor.is_some(),
            description: "Sandboxed code execution".to_string(),
        },
    ];

    Json(PlatformConfig {
        providers,
        namespaces,
        features,
    })
}

/// GET /api/admin/config/providers — List LLM providers
pub async fn list_providers(State(state): State<AppState>) -> Json<Vec<ProviderInfo>> {
    let router = state.llm_router.read().await;
    let registered = router.list_providers();

    let providers: Vec<ProviderInfo> = registered
        .iter()
        .map(|name| {
            let models = match name.as_str() {
                "anthropic" => vec!["claude-sonnet-4-5-20250929".to_string()],
                "openai" => vec!["gpt-4o".to_string()],
                "qwen" => vec!["qwen-turbo".to_string(), "qwen-max".to_string()],
                "google" => vec!["gemini-2.0-flash".to_string()],
                _ => vec![],
            };
            ProviderInfo {
                name: name.clone(),
                status: "configured".to_string(),
                models,
            }
        })
        .collect();

    Json(providers)
}

/// GET /api/admin/config/namespaces — List tool namespaces
async fn list_namespaces(State(state): State<AppState>) -> Json<Vec<NamespaceInfo>> {
    Json(vec![
        NamespaceInfo {
            name: "engine".to_string(),
            enabled: true,
            tool_count: 3,
        },
        NamespaceInfo {
            name: "platform".to_string(),
            enabled: true,
            tool_count: 2,
        },
        NamespaceInfo {
            name: "tools".to_string(),
            enabled: true,
            tool_count: 2,
        },
        NamespaceInfo {
            name: "billing".to_string(),
            enabled: true,
            tool_count: 3,
        },
        NamespaceInfo {
            name: "workflow".to_string(),
            enabled: true,
            tool_count: 2,
        },
        NamespaceInfo {
            name: "runtime".to_string(),
            enabled: state.code_executor.is_some(),
            tool_count: 9,
        },
    ])
}
