//! Settings API endpoints
//!
//! Provides API routes for managing application settings including
//! LLM providers, MCP servers, filesystem access, and more.
//!
//! Settings are persisted to `~/.canal/settings.json` (or the path
//! resolved via `dirs::config_dir()`).  On each update the incoming payload
//! is merged into the cached JSON object and atomically written to disk.

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::{error::ApiError, middleware::auth::AuthContext, state::AppState};

/// Create the settings routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(get_settings))
        .route("/", post(update_settings))
        .route("/llm", get(get_llm_settings))
        .route("/llm", post(update_llm_settings))
        .route("/mcp", get(get_mcp_settings))
        .route("/mcp", post(update_mcp_settings))
        .route("/mcp/browser/toggle", post(toggle_browser_mcp))
        .route("/mcp/{namespace}/toggle", post(toggle_mcp_namespace))
        .route("/mcp/enabled", get(get_enabled_mcp_namespaces))
        .route("/filesystem", get(get_filesystem_settings))
        .route("/filesystem", post(update_filesystem_settings))
        .route("/executor", get(get_executor_settings))
        .route("/tools", get(get_available_tools))
        .route("/tools/enabled", get(get_enabled_tools))
}

// ============ Types ============

/// Full application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub llm: LlmSettings,
    pub mcp: McpSettings,
    pub filesystem: FilesystemSettings,
    pub executor: ExecutorSettings,
    pub general: GeneralSettings,
}

/// LLM provider settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmSettings {
    pub providers: Vec<LlmProviderConfig>,
    pub default_provider: String,
    pub default_model: String,
}

/// Single LLM provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmProviderConfig {
    pub name: String,
    pub enabled: bool,
    pub api_key_set: bool,
    pub models: Vec<String>,
    pub base_url: Option<String>,
}

/// MCP server settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSettings {
    pub servers: Vec<McpServerConfig>,
    pub auto_connect: bool,
}

/// Single MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub namespace: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub enabled: bool,
}

/// Filesystem access settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemSettings {
    pub allowed_directories: Vec<String>,
    pub blocked_patterns: Vec<String>,
    pub max_file_size_mb: u64,
}

/// Code executor settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorSettings {
    pub docker_enabled: bool,
    pub languages: Vec<LanguageConfig>,
    pub default_timeout_ms: u64,
    pub memory_limit_mb: u64,
    pub cpu_limit: f64,
}

/// Language configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageConfig {
    pub name: String,
    pub enabled: bool,
    pub timeout_ms: u64,
}

/// General application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralSettings {
    pub theme: String,
    pub language: String,
    pub notifications_enabled: bool,
    pub auto_start: bool,
    pub api_url: String,
}

/// Settings update request
#[derive(Debug, Deserialize)]
pub struct UpdateSettingsRequest {
    #[serde(flatten)]
    pub settings: serde_json::Value,
}

// ============ Handlers ============

/// Get all settings
///
/// Returns a merged view: persisted settings override runtime defaults.
pub async fn get_settings(State(state): State<AppState>) -> Result<Json<AppSettings>, ApiError> {
    // Build runtime defaults
    let mut llm_settings = build_llm_settings(&state);
    let mcp_settings = build_mcp_settings(&state);
    let mut filesystem_settings = build_filesystem_settings(&state);
    let executor_settings = build_executor_settings(&state);
    let mut general_settings = GeneralSettings {
        theme: "system".to_string(),
        language: "en".to_string(),
        notifications_enabled: true,
        auto_start: false,
        api_url: std::env::var("API_URL").unwrap_or_else(|_| "http://localhost:4000".to_string()),
    };

    // Overlay persisted settings if available
    let cached = state.cached_settings.read().await;
    if let Some(obj) = cached.as_object() {
        if let Some(llm_val) = obj.get("llm") {
            if let Ok(persisted) = serde_json::from_value::<LlmSettings>(llm_val.clone()) {
                llm_settings = persisted;
            }
        }
        if let Some(fs_val) = obj.get("filesystem") {
            if let Ok(persisted) = serde_json::from_value::<FilesystemSettings>(fs_val.clone()) {
                filesystem_settings = persisted;
            }
        }
        if let Some(gen_val) = obj.get("general") {
            if let Ok(persisted) = serde_json::from_value::<GeneralSettings>(gen_val.clone()) {
                general_settings = persisted;
            }
        }
    }

    Ok(Json(AppSettings {
        llm: llm_settings,
        mcp: mcp_settings,
        filesystem: filesystem_settings,
        executor: executor_settings,
        general: general_settings,
    }))
}

/// Update settings
///
/// Merges the incoming JSON into the cached settings and persists to disk.
/// Requires admin role — system-wide settings should not be modifiable by any user.
pub async fn update_settings(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(request): Json<UpdateSettingsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !auth.is_admin() {
        return Err(ApiError::forbidden(
            "Admin role required to modify settings",
        ));
    }
    tracing::info!(settings = ?request.settings, "Settings update requested");

    state
        .merge_and_persist_settings_flat(request.settings)
        .await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Settings updated and persisted"
    })))
}

/// Get LLM settings
///
/// Returns persisted LLM settings if available, otherwise runtime defaults.
pub async fn get_llm_settings(
    State(state): State<AppState>,
) -> Result<Json<LlmSettings>, ApiError> {
    let cached = state.cached_settings.read().await;
    if let Some(llm_val) = cached.as_object().and_then(|o| o.get("llm")) {
        if let Ok(persisted) = serde_json::from_value::<LlmSettings>(llm_val.clone()) {
            return Ok(Json(persisted));
        }
    }
    Ok(Json(build_llm_settings(&state)))
}

/// Update LLM settings
///
/// Persists the LLM settings under the "llm" key.
/// Requires admin role — LLM provider config is system-wide.
pub async fn update_llm_settings(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(settings): Json<LlmSettings>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !auth.is_admin() {
        return Err(ApiError::forbidden(
            "Admin role required to modify LLM settings",
        ));
    }
    tracing::info!(
        default_provider = %settings.default_provider,
        default_model = %settings.default_model,
        "LLM settings update requested"
    );

    let value = serde_json::to_value(&settings)
        .map_err(|e| ApiError::internal(format!("Failed to serialize LLM settings: {}", e)))?;
    state.merge_and_persist_settings("llm", value).await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "LLM settings updated and persisted"
    })))
}

/// Get MCP settings
pub async fn get_mcp_settings(
    State(state): State<AppState>,
) -> Result<Json<McpSettings>, ApiError> {
    // MCP settings come from the live gateway; persisted overrides for
    // `auto_connect` etc. are layered on top.
    let gateway = &state.mcp_gateway;
    let servers = gateway.list_servers().await;

    let server_configs: Vec<McpServerConfig> = servers
        .into_iter()
        .map(|(namespace, info)| McpServerConfig {
            name: namespace.clone(),
            namespace,
            transport: "stdio".to_string(),
            command: None,
            args: vec![],
            url: None,
            enabled: info.connected,
        })
        .collect();

    // Add builtin namespaces (filesystem, executor)
    let mut all_servers = server_configs;

    // Check if builtin tools are available
    let tools = gateway.get_tools().await;
    let has_filesystem = tools.iter().any(|t| t.namespace == "filesystem");
    let has_executor = tools.iter().any(|t| t.namespace == "executor");

    if has_filesystem {
        all_servers.push(McpServerConfig {
            name: "Filesystem".to_string(),
            namespace: "filesystem".to_string(),
            transport: "builtin".to_string(),
            command: None,
            args: vec![],
            url: None,
            enabled: true,
        });
    }

    if has_executor {
        all_servers.push(McpServerConfig {
            name: "Code Executor".to_string(),
            namespace: "executor".to_string(),
            transport: "builtin".to_string(),
            command: None,
            args: vec![],
            url: None,
            enabled: true,
        });
    }

    // Layer persisted auto_connect preference
    let auto_connect = {
        let cached = state.cached_settings.read().await;
        cached
            .as_object()
            .and_then(|o| o.get("mcp"))
            .and_then(|v| v.get("auto_connect"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    };

    Ok(Json(McpSettings {
        servers: all_servers,
        auto_connect,
    }))
}

/// Update MCP settings
///
/// Persists the MCP settings under the "mcp" key.
/// Requires admin role — MCP server config is system-wide.
pub async fn update_mcp_settings(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(settings): Json<McpSettings>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !auth.is_admin() {
        return Err(ApiError::forbidden(
            "Admin role required to modify MCP settings",
        ));
    }
    tracing::info!(
        server_count = settings.servers.len(),
        "MCP settings update requested"
    );

    let value = serde_json::to_value(&settings)
        .map_err(|e| ApiError::internal(format!("Failed to serialize MCP settings: {}", e)))?;
    state.merge_and_persist_settings("mcp", value).await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "MCP settings updated and persisted"
    })))
}

/// Get filesystem settings
///
/// Returns persisted filesystem settings if available, otherwise runtime defaults.
pub async fn get_filesystem_settings(
    State(state): State<AppState>,
) -> Result<Json<FilesystemSettings>, ApiError> {
    let cached = state.cached_settings.read().await;
    if let Some(fs_val) = cached.as_object().and_then(|o| o.get("filesystem")) {
        if let Ok(persisted) = serde_json::from_value::<FilesystemSettings>(fs_val.clone()) {
            return Ok(Json(persisted));
        }
    }
    Ok(Json(build_filesystem_settings(&state)))
}

/// Update filesystem settings
///
/// Persists the filesystem settings under the "filesystem" key.
/// Requires admin role — filesystem access config is security-sensitive.
pub async fn update_filesystem_settings(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(settings): Json<FilesystemSettings>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !auth.is_admin() {
        return Err(ApiError::forbidden(
            "Admin role required to modify filesystem settings",
        ));
    }
    tracing::info!(
        allowed_dirs = ?settings.allowed_directories,
        "Filesystem settings update requested"
    );

    let value = serde_json::to_value(&settings).map_err(|e| {
        ApiError::internal(format!("Failed to serialize filesystem settings: {}", e))
    })?;
    state
        .merge_and_persist_settings("filesystem", value)
        .await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Filesystem settings updated and persisted"
    })))
}

/// Get executor settings
pub async fn get_executor_settings(
    State(state): State<AppState>,
) -> Result<Json<ExecutorSettings>, ApiError> {
    Ok(Json(build_executor_settings(&state)))
}

/// Toggle browser MCP request
#[derive(Debug, Deserialize)]
pub struct ToggleBrowserRequest {
    pub enabled: bool,
}

/// Toggle browser MCP response
#[derive(Debug, Serialize)]
pub struct ToggleBrowserResponse {
    pub enabled: bool,
    pub connected: bool,
    pub tool_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Toggle browser-mcp on/off
pub async fn toggle_browser_mcp(
    State(state): State<AppState>,
    Json(request): Json<ToggleBrowserRequest>,
) -> Result<Json<ToggleBrowserResponse>, ApiError> {
    let gateway = &state.mcp_gateway;

    if request.enabled {
        // Enable browser-mcp
        tracing::info!("Enabling browser-mcp...");

        // Check if already registered
        if let Some(info) = gateway.get_server_info("browser").await {
            if info.connected {
                return Ok(Json(ToggleBrowserResponse {
                    enabled: true,
                    connected: true,
                    tool_count: info.tool_count,
                    error: None,
                }));
            }
        }

        // Try to find and register browser-mcp
        let browser_path = find_browser_mcp_path();
        if browser_path.is_none() {
            return Ok(Json(ToggleBrowserResponse {
                enabled: false,
                connected: false,
                tool_count: 0,
                error: Some(
                    "browser-mcp not found. Set BROWSER_MCP_PATH environment variable.".to_string(),
                ),
            }));
        }

        let path = browser_path.unwrap();
        let mut env = std::collections::HashMap::new();
        let headless = std::env::var("BROWSER_HEADLESS").unwrap_or_else(|_| "false".to_string());
        env.insert("BROWSER_HEADLESS".to_string(), headless);

        // Register
        if let Err(e) = gateway
            .register_server("browser", "node", vec![path], env)
            .await
        {
            return Ok(Json(ToggleBrowserResponse {
                enabled: false,
                connected: false,
                tool_count: 0,
                error: Some(format!("Failed to register: {}", e)),
            }));
        }

        // Connect
        match gateway.connect_server("browser").await {
            Ok(()) => {
                let info = gateway.get_server_info("browser").await.unwrap_or(
                    gateway_core::mcp::gateway::McpServerInfo {
                        tool_count: 0,
                        connected: false,
                        transport_type: "stdio".to_string(),
                        location: "local".to_string(),
                        server_name: "browser".to_string(),
                        description: String::new(),
                    },
                );
                Ok(Json(ToggleBrowserResponse {
                    enabled: true,
                    connected: info.connected,
                    tool_count: info.tool_count,
                    error: None,
                }))
            }
            Err(e) => Ok(Json(ToggleBrowserResponse {
                enabled: true,
                connected: false,
                tool_count: 0,
                error: Some(format!("Failed to connect: {}", e)),
            })),
        }
    } else {
        // Disable browser-mcp
        tracing::info!("Disabling browser-mcp...");
        gateway.unregister_server("browser").await;

        Ok(Json(ToggleBrowserResponse {
            enabled: false,
            connected: false,
            tool_count: 0,
            error: None,
        }))
    }
}

/// Toggle MCP namespace request
#[derive(Debug, Deserialize)]
pub struct ToggleMcpNamespaceRequest {
    pub enabled: bool,
}

/// Toggle MCP namespace response
#[derive(Debug, Serialize)]
pub struct ToggleMcpNamespaceResponse {
    pub namespace: String,
    pub enabled: bool,
    pub tool_count: usize,
}

/// Toggle any MCP namespace on/off
pub async fn toggle_mcp_namespace(
    State(state): State<AppState>,
    Path(namespace): Path<String>,
    Json(request): Json<ToggleMcpNamespaceRequest>,
) -> Result<Json<ToggleMcpNamespaceResponse>, ApiError> {
    tracing::info!(
        namespace = %namespace,
        enabled = request.enabled,
        "Toggling MCP namespace"
    );

    // Get current enabled namespaces from settings
    let mut enabled_namespaces = get_enabled_namespaces_from_settings(&state).await;

    if request.enabled {
        // Add to enabled list
        if !enabled_namespaces.contains(&namespace) {
            enabled_namespaces.push(namespace.clone());
        }
    } else {
        // Remove from enabled list
        enabled_namespaces.retain(|n| n != &namespace);
    }

    // Persist the updated list
    let value = serde_json::json!({
        "enabled_namespaces": enabled_namespaces
    });
    state.merge_and_persist_settings("mcp", value).await?;

    // Get tool count for this namespace
    let tools = state.tool_system.list_tools().await;
    let tool_count = tools.iter().filter(|t| t.id.namespace == namespace).count();

    Ok(Json(ToggleMcpNamespaceResponse {
        namespace,
        enabled: request.enabled,
        tool_count,
    }))
}

/// Enabled MCP namespaces response
#[derive(Debug, Serialize)]
pub struct EnabledMcpNamespacesResponse {
    pub enabled_namespaces: Vec<String>,
    pub all_namespaces: Vec<NamespaceStatus>,
}

/// Namespace status with enabled flag
#[derive(Debug, Serialize)]
pub struct NamespaceStatus {
    pub namespace: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub tool_count: usize,
    pub platform: Option<String>,
}

/// Get enabled MCP namespaces
pub async fn get_enabled_mcp_namespaces(
    State(state): State<AppState>,
) -> Result<Json<EnabledMcpNamespacesResponse>, ApiError> {
    let enabled_namespaces = get_enabled_namespaces_from_settings(&state).await;

    // Get all available namespaces with their tools
    let tools = state.tool_system.list_tools().await;

    // Build namespace status list
    let mut namespace_map: std::collections::HashMap<String, NamespaceStatus> =
        std::collections::HashMap::new();

    for tool in &tools {
        let entry = namespace_map
            .entry(tool.id.namespace.clone())
            .or_insert_with(|| {
                let (name, description, platform) = get_namespace_info(&tool.id.namespace);
                NamespaceStatus {
                    namespace: tool.id.namespace.clone(),
                    name,
                    description,
                    enabled: enabled_namespaces.contains(&tool.id.namespace),
                    tool_count: 0,
                    platform,
                }
            });
        entry.tool_count += 1;
    }

    let all_namespaces: Vec<NamespaceStatus> = namespace_map.into_values().collect();

    Ok(Json(EnabledMcpNamespacesResponse {
        enabled_namespaces,
        all_namespaces,
    }))
}

/// Get namespace display info
fn get_namespace_info(namespace: &str) -> (String, String, Option<String>) {
    match namespace {
        "mac" => (
            "macOS Automation".to_string(),
            "Execute AppleScript to control Mac applications".to_string(),
            Some("macos".to_string()),
        ),
        "win" => (
            "Windows Automation".to_string(),
            "Control Windows through UI automation and PowerShell".to_string(),
            Some("windows".to_string()),
        ),
        "filesystem" => (
            "Filesystem".to_string(),
            "Read, write, and search files".to_string(),
            None,
        ),
        "executor" => (
            "Code Executor".to_string(),
            "Execute code in various languages (Python, Bash, etc.)".to_string(),
            None,
        ),
        "browser" => (
            "Browser Control".to_string(),
            "Automate web browsers via Chrome extension".to_string(),
            None,
        ),
        _ => (
            namespace.to_string(),
            format!("{} MCP tools", namespace),
            None,
        ),
    }
}

/// Helper to get enabled namespaces from persisted settings.
///
/// This function reads the cached settings from AppState and extracts
/// the list of enabled MCP namespaces. If no namespaces are configured,
/// it returns a default set of core namespaces.
///
/// # Returns
/// A vector of enabled namespace names (e.g., ["filesystem", "executor", "browser", "mac"])
pub async fn get_enabled_namespaces_from_settings(state: &AppState) -> Vec<String> {
    let cached = state.cached_settings.read().await;
    cached
        .as_object()
        .and_then(|o| o.get("mcp"))
        .and_then(|v| v.get("enabled_namespaces"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| {
            // Default: all core namespaces enabled
            vec![
                "filesystem".to_string(),
                "executor".to_string(),
                "browser".to_string(),
                "mac".to_string(),
                "hosting".to_string(),
                "platform".to_string(),
                "devtools".to_string(),
            ]
        })
}

/// Get only enabled tools (filtered by enabled namespaces)
pub async fn get_enabled_tools(
    State(state): State<AppState>,
) -> Result<Json<AvailableToolsResponse>, ApiError> {
    let enabled_namespaces = get_enabled_namespaces_from_settings(&state).await;
    let gateway = &state.mcp_gateway;

    // Get all tools and filter by enabled namespaces
    let all_tools = gateway.get_tools().await;
    let tools: Vec<ToolInfo> = all_tools
        .iter()
        .filter(|t| enabled_namespaces.contains(&t.namespace))
        .map(|t| ToolInfo {
            namespace: t.namespace.clone(),
            name: t.name.clone(),
            full_name: format!("{}_{}", t.namespace, t.name),
            description: t.description.clone(),
        })
        .collect();

    // Build namespace info (only for enabled)
    let servers = gateway.list_servers().await;
    let namespaces: Vec<NamespaceInfo> = servers
        .into_iter()
        .filter(|(ns, _)| enabled_namespaces.contains(ns))
        .map(|(ns, info)| NamespaceInfo {
            name: ns,
            tool_count: info.tool_count,
            connected: info.connected,
        })
        .collect();

    let total = tools.len();

    Ok(Json(AvailableToolsResponse {
        tools,
        total,
        namespaces,
    }))
}

/// Available tools response
#[derive(Debug, Serialize)]
pub struct AvailableToolsResponse {
    pub tools: Vec<ToolInfo>,
    pub total: usize,
    pub namespaces: Vec<NamespaceInfo>,
}

#[derive(Debug, Serialize)]
pub struct ToolInfo {
    pub namespace: String,
    pub name: String,
    pub full_name: String,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct NamespaceInfo {
    pub name: String,
    pub tool_count: usize,
    pub connected: bool,
}

/// Get all available tools
pub async fn get_available_tools(
    State(state): State<AppState>,
) -> Result<Json<AvailableToolsResponse>, ApiError> {
    let gateway = &state.mcp_gateway;

    // Get all tools
    let tools = gateway.get_tools().await;
    let tool_infos: Vec<ToolInfo> = tools
        .iter()
        .map(|t| ToolInfo {
            namespace: t.namespace.clone(),
            name: t.name.clone(),
            full_name: format!("{}_{}", t.namespace, t.name),
            description: t.description.clone(),
        })
        .collect();

    // Get namespace info
    let servers = gateway.list_servers().await;
    let namespaces: Vec<NamespaceInfo> = servers
        .into_iter()
        .map(|(ns, info)| NamespaceInfo {
            name: ns,
            tool_count: info.tool_count,
            connected: info.connected,
        })
        .collect();

    let total = tool_infos.len();

    Ok(Json(AvailableToolsResponse {
        tools: tool_infos,
        total,
        namespaces,
    }))
}

// ============ Helper Functions ============

/// Find browser-mcp path (duplicated from state.rs for use here)
fn find_browser_mcp_path() -> Option<String> {
    if let Ok(path) = std::env::var("BROWSER_MCP_PATH") {
        if std::path::Path::new(&path).exists() {
            return Some(path);
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let possible_paths = vec![
            "servers/browser-mcp/dist/index.js",
            "../servers/browser-mcp/dist/index.js",
            "../../servers/browser-mcp/dist/index.js",
        ];

        for rel_path in &possible_paths {
            let full_path = cwd.join(rel_path);
            if full_path.exists() {
                return Some(full_path.to_string_lossy().to_string());
            }
        }

        // Try going up from cwd
        let mut dir = cwd.as_path();
        for _ in 0..5 {
            let path = dir.join("servers/browser-mcp/dist/index.js");
            if path.exists() {
                return Some(path.to_string_lossy().to_string());
            }
            if let Some(parent) = dir.parent() {
                dir = parent;
            } else {
                break;
            }
        }
    }

    None
}

fn build_llm_settings(_state: &AppState) -> LlmSettings {
    // Build provider configs from known providers
    let provider_names = vec!["qwen", "anthropic", "google", "openai"];

    let provider_configs: Vec<LlmProviderConfig> = provider_names
        .iter()
        .map(|name| {
            let api_key_set = match *name {
                "qwen" => std::env::var("QWEN_API_KEY").is_ok(),
                "anthropic" => std::env::var("ANTHROPIC_API_KEY").is_ok(),
                "google" => std::env::var("GOOGLE_AI_API_KEY").is_ok(),
                "openai" => std::env::var("OPENAI_API_KEY").is_ok(),
                _ => false,
            };

            let models = match *name {
                "qwen" => vec![
                    "qwen3-vl-plus".to_string(),
                    "qwen3-max-2026-01-23".to_string(),
                    "qwq-plus".to_string(),
                ],
                "anthropic" => vec![
                    "claude-sonnet-4-6".to_string(),
                    "claude-haiku-4-5-20251001".to_string(),
                    "claude-opus-4-6".to_string(),
                ],
                "google" => vec!["gemini-3-pro".to_string(), "gemini-3-flash".to_string()],
                "openai" => vec![
                    "gpt-4o".to_string(),
                    "gpt-4-turbo".to_string(),
                    "gpt-3.5-turbo".to_string(),
                ],
                _ => vec![],
            };

            let base_url = match *name {
                "qwen" => Some(std::env::var("QWEN_BASE_URL").unwrap_or_else(|_| {
                    "https://dashscope-intl.aliyuncs.com/compatible-mode".to_string()
                })),
                _ => None,
            };

            LlmProviderConfig {
                name: name.to_string(),
                enabled: api_key_set,
                api_key_set,
                models,
                base_url,
            }
        })
        .collect();

    LlmSettings {
        providers: provider_configs,
        default_provider: "qwen".to_string(),
        default_model: "qwen3-max-2026-01-23".to_string(),
    }
}

fn build_mcp_settings(_state: &AppState) -> McpSettings {
    // MCP gateway uses async methods, so we return empty for sync call
    // The actual data should be fetched via the async endpoint
    McpSettings {
        servers: vec![],
        auto_connect: true,
    }
}

fn build_filesystem_settings(state: &AppState) -> FilesystemSettings {
    let allowed_dirs = state
        .filesystem_service
        .as_ref()
        .map(|fs| fs.get_allowed_directories())
        .unwrap_or_default();

    FilesystemSettings {
        allowed_directories: allowed_dirs,
        blocked_patterns: vec![
            ".env".to_string(),
            "*.key".to_string(),
            "*.pem".to_string(),
            "*credentials*".to_string(),
            ".ssh/*".to_string(),
        ],
        max_file_size_mb: 10,
    }
}

fn build_executor_settings(state: &AppState) -> ExecutorSettings {
    let docker_enabled = state.code_executor.is_some();

    let languages = vec![
        LanguageConfig {
            name: "python".to_string(),
            enabled: true,
            timeout_ms: 30000,
        },
        LanguageConfig {
            name: "bash".to_string(),
            enabled: true,
            timeout_ms: 10000,
        },
        LanguageConfig {
            name: "javascript".to_string(),
            enabled: true,
            timeout_ms: 30000,
        },
        LanguageConfig {
            name: "typescript".to_string(),
            enabled: true,
            timeout_ms: 30000,
        },
        LanguageConfig {
            name: "go".to_string(),
            enabled: true,
            timeout_ms: 60000,
        },
        LanguageConfig {
            name: "rust".to_string(),
            enabled: true,
            timeout_ms: 120000,
        },
    ];

    ExecutorSettings {
        docker_enabled,
        languages,
        default_timeout_ms: 30000,
        memory_limit_mb: 512,
        cpu_limit: 1.0,
    }
}
