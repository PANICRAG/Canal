//! Connector and Bundle API endpoints.
//!
//! Provides a unified API for:
//! - **Connectors**: Browse, install/uninstall, categories (replaces /api/plugins/)
//! - **Bundles**: Browse, activate/deactivate plugin bundles
//!
//! ## Routes
//!
//! ### Connector routes (at /api/connectors/)
//! - `GET /catalog` — Browse all connectors
//! - `GET /catalog/{name}` — Connector detail
//! - `GET /categories` — List all connector categories
//! - `POST /{name}/toggle` — Enable/disable built-in connector
//! - `POST /{name}/install` — Install connector
//! - `POST /{name}/uninstall` — Uninstall connector
//! - `GET /installed` — List installed connectors
//! - `POST /reload` — Re-scan directories
//!
//! ### Bundle routes (at /api/bundles/)
//! - `GET /` — List all bundles
//! - `GET /{name}` — Bundle detail
//! - `POST /{name}/activate` — Activate bundle for user
//! - `POST /{name}/deactivate` — Deactivate bundle
//! - `GET /active` — List active bundles
//! - `POST /reload` — Re-scan bundle directories

use axum::{
    extract::{Json, Path, State},
    routing::{get, post},
    Router,
};
use gateway_core::connectors::McpConnectionStatus;
use gateway_core::plugins::PluginApiResponse;
use serde::Serialize;

use crate::{error::ApiError, middleware::auth::AuthContext, state::AppState};

// ============================================================================
// Connector Routes (mirrors /api/plugins/ with new naming)
// ============================================================================

/// Create the connector routes (mounted at /api/connectors/)
pub fn connector_routes() -> Router<AppState> {
    Router::new()
        .route("/catalog", get(browse_catalog))
        .route("/catalog/{name}", get(catalog_detail))
        .route("/categories", get(list_categories))
        .route("/installed", get(list_installed))
        .route("/{name}/install", post(install_connector))
        .route("/{name}/uninstall", post(uninstall_connector))
        .route("/reload", post(reload_catalog))
        .route("/{name}/references/{ref_name}", get(get_reference))
}

/// Create the bundle routes (mounted at /api/bundles/)
pub fn bundle_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_bundles))
        .route("/active", get(list_active_bundles))
        .route("/reload", post(reload_bundles))
        .route("/{name}", get(bundle_detail))
        .route("/{name}/activate", post(activate_bundle))
        .route("/{name}/deactivate", post(deactivate_bundle))
}

// ============================================================================
// Response Types
// ============================================================================

/// Category entry for API responses.
#[derive(Debug, Serialize)]
pub struct CategoryResponse {
    /// Category ID (e.g., "~~file-system").
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Description.
    pub description: String,
    /// Default connectors.
    pub default_connectors: Vec<String>,
    /// Platform restriction.
    pub platform: Option<String>,
    /// Icon identifier.
    pub icon: Option<String>,
}

/// Bundle entry for API responses.
#[derive(Debug, Serialize)]
pub struct BundleResponse {
    /// Bundle name.
    pub name: String,
    /// Version.
    pub version: String,
    /// Description.
    pub description: String,
    /// Author.
    pub author: Option<String>,
    /// Required categories.
    pub required_categories: Vec<String>,
    /// Optional categories.
    pub optional_categories: Vec<String>,
    /// Whether this bundle is active for the user.
    pub active: bool,
    /// Has system prompt.
    pub has_prompt: bool,
    /// Number of MCP servers defined in this bundle.
    pub mcp_server_count: usize,
    /// Per-server MCP connection status (only populated when bundle is active).
    pub mcp_servers: Vec<McpServerStatusResponse>,
}

/// Per-server MCP connection status in API responses.
#[derive(Debug, Serialize)]
pub struct McpServerStatusResponse {
    /// Server name.
    pub name: String,
    /// Server URL.
    pub url: String,
    /// Status label: "pending", "connecting", "connected", "failed", "disconnected".
    pub status: String,
    /// Error message (only when status is "failed").
    pub error: Option<String>,
}

/// Connector catalog detail response.
#[derive(Debug, Serialize)]
pub struct ConnectorDetailResponse {
    /// Connector name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Version.
    pub version: String,
    /// Format type.
    pub format: String,
    /// Author.
    pub author: Option<String>,
    /// Number of skills.
    pub skills_count: usize,
    /// Reference names.
    pub references: Vec<String>,
    /// Has scripts.
    pub has_scripts: bool,
    /// Has MCP.
    pub has_mcp: bool,
    /// Installed by user.
    pub installed: bool,
    /// Skill names.
    pub skill_names: Vec<String>,
    /// Categories this connector satisfies.
    pub categories: Vec<String>,
}

// ============================================================================
// Helpers
// ============================================================================

/// Build MCP server status list for a bundle from the connection tracker.
fn build_mcp_server_statuses(
    bundle: &gateway_core::connectors::BundleDefinition,
    conn_tracker: &gateway_core::connectors::McpConnectionTracker,
    is_active: bool,
) -> Vec<McpServerStatusResponse> {
    if !is_active || bundle.mcp_servers.is_empty() {
        return Vec::new();
    }
    bundle
        .mcp_servers
        .iter()
        .map(|s| {
            let status = conn_tracker.get_status(&s.name);
            McpServerStatusResponse {
                name: s.name.clone(),
                url: s.url.clone(),
                status: status.label().to_string(),
                error: status.error_message().map(String::from),
            }
        })
        .collect()
}

// ============================================================================
// Connector Handlers
// ============================================================================

/// Browse the connector catalog.
///
/// GET /api/connectors/catalog
async fn browse_catalog(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<PluginApiResponse<Vec<gateway_core::plugins::CatalogEntry>>>, ApiError> {
    let user_id = auth.user_id.to_string();
    let entries = state.plugin_manager.browse_catalog(&user_id).await;

    Ok(Json(PluginApiResponse {
        success: true,
        data: Some(entries),
        error: None,
    }))
}

/// Get connector detail.
///
/// GET /api/connectors/catalog/{name}
async fn catalog_detail(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<PluginApiResponse<ConnectorDetailResponse>>, ApiError> {
    let user_id = auth.user_id.to_string();

    let entry = match state
        .plugin_manager
        .get_catalog_entry(&user_id, &name)
        .await
    {
        Ok(entry) => entry,
        Err(_) => {
            return Ok(Json(PluginApiResponse {
                success: false,
                data: None,
                error: Some(format!("Connector '{}' not found", name)),
            }));
        }
    };

    let catalog = state.plugin_manager.catalog.read().await;
    let detail = if let Some(plugin) = catalog.get(&name) {
        ConnectorDetailResponse {
            name: entry.name,
            description: entry.description,
            version: entry.version,
            format: entry.format.clone(),
            author: entry.author,
            skills_count: entry.skills_count,
            references: plugin
                .reference_paths
                .iter()
                .map(|(name, _)| name.clone())
                .collect(),
            has_scripts: plugin.scripts_dir.is_some(),
            has_mcp: plugin.mcp_config.is_some(),
            installed: entry.installed,
            skill_names: plugin.skills.iter().map(|s| s.name.clone()).collect(),
            categories: Vec::new(), // TODO: resolve from CategoryResolver
        }
    } else {
        ConnectorDetailResponse {
            name: entry.name,
            description: entry.description,
            version: entry.version,
            format: entry.format.clone(),
            author: entry.author,
            skills_count: entry.skills_count,
            references: Vec::new(),
            has_scripts: false,
            has_mcp: false,
            installed: entry.installed,
            skill_names: Vec::new(),
            categories: Vec::new(),
        }
    };

    Ok(Json(PluginApiResponse {
        success: true,
        data: Some(detail),
        error: None,
    }))
}

/// List all connector categories.
///
/// GET /api/connectors/categories
///
/// Requires authentication.
async fn list_categories(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
) -> Result<Json<PluginApiResponse<Vec<CategoryResponse>>>, ApiError> {
    let resolver = state.category_resolver.read().await;
    let categories = resolver
        .list_categories_with_definitions()
        .into_iter()
        .map(|cat| CategoryResponse {
            id: cat.id,
            display_name: cat.definition.display_name,
            description: cat.definition.description,
            default_connectors: cat.definition.default_connectors,
            platform: cat.definition.platform,
            icon: cat.definition.icon,
        })
        .collect();

    Ok(Json(PluginApiResponse {
        success: true,
        data: Some(categories),
        error: None,
    }))
}

/// List installed connectors.
///
/// GET /api/connectors/installed
async fn list_installed(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<PluginApiResponse<Vec<String>>>, ApiError> {
    let user_id = auth.user_id.to_string();
    let names = state
        .plugin_manager
        .get_installed_skill_names(&user_id)
        .await;

    Ok(Json(PluginApiResponse {
        success: true,
        data: Some(names),
        error: None,
    }))
}

/// Install a connector.
///
/// POST /api/connectors/{name}/install
async fn install_connector(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<PluginApiResponse<()>>, ApiError> {
    let user_id = auth.user_id.to_string();
    tracing::info!(user_id = %user_id, connector = %name, "Installing connector");

    match state.plugin_manager.install_plugin(&user_id, &name).await {
        Ok(()) => Ok(Json(PluginApiResponse {
            success: true,
            data: Some(()),
            error: None,
        })),
        Err(e) => Ok(Json(PluginApiResponse {
            success: false,
            data: None,
            error: Some(e.to_string()),
        })),
    }
}

/// Uninstall a connector.
///
/// POST /api/connectors/{name}/uninstall
async fn uninstall_connector(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<PluginApiResponse<()>>, ApiError> {
    let user_id = auth.user_id.to_string();
    tracing::info!(user_id = %user_id, connector = %name, "Uninstalling connector");

    match state.plugin_manager.uninstall_plugin(&user_id, &name).await {
        Ok(()) => Ok(Json(PluginApiResponse {
            success: true,
            data: Some(()),
            error: None,
        })),
        Err(e) => Ok(Json(PluginApiResponse {
            success: false,
            data: None,
            error: Some(e.to_string()),
        })),
    }
}

/// Reload connector catalog.
///
/// POST /api/connectors/reload
///
/// Requires admin role.
async fn reload_catalog(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<PluginApiResponse<usize>>, ApiError> {
    if !auth.is_admin() {
        return Ok(Json(PluginApiResponse {
            success: false,
            data: None,
            error: Some("Admin access required".to_string()),
        }));
    }

    let count = state.plugin_manager.reload_catalog().await;
    tracing::info!(connectors_loaded = count, admin = %auth.email, "Connector catalog reloaded");

    Ok(Json(PluginApiResponse {
        success: true,
        data: Some(count),
        error: None,
    }))
}

/// Get reference content from a connector.
///
/// GET /api/connectors/{name}/references/{ref_name}
///
/// Requires authentication.
async fn get_reference(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
    Path((name, ref_name)): Path<(String, String)>,
) -> Result<Json<PluginApiResponse<String>>, ApiError> {
    match state.plugin_manager.get_reference(&name, &ref_name).await {
        Ok(content) => Ok(Json(PluginApiResponse {
            success: true,
            data: Some(content),
            error: None,
        })),
        Err(e) => Ok(Json(PluginApiResponse {
            success: false,
            data: None,
            error: Some(e.to_string()),
        })),
    }
}

// ============================================================================
// Bundle Handlers
// ============================================================================

/// List all available bundles.
///
/// GET /api/bundles/
async fn list_bundles(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<PluginApiResponse<Vec<BundleResponse>>>, ApiError> {
    let user_id = auth.user_id.to_string();
    let bundle_mgr = state.bundle_manager.read().await;
    let runtime = state.runtime_registry.read().await;
    let active = runtime.get_active_bundles(&user_id);

    let bundles: Vec<BundleResponse> = bundle_mgr
        .list_all()
        .into_iter()
        .map(|b| {
            let is_active = active.contains(&b.name);
            BundleResponse {
                name: b.name.clone(),
                version: b.version.clone(),
                description: b.description.clone(),
                author: b.author.clone(),
                required_categories: b.required_categories.clone(),
                optional_categories: b.optional_categories.clone(),
                active: is_active,
                has_prompt: b.system_prompt.is_some(),
                mcp_server_count: b.mcp_servers.len(),
                mcp_servers: build_mcp_server_statuses(b, &state.mcp_connection_tracker, is_active),
            }
        })
        .collect();

    Ok(Json(PluginApiResponse {
        success: true,
        data: Some(bundles),
        error: None,
    }))
}

/// Get bundle detail.
///
/// GET /api/bundles/{name}
async fn bundle_detail(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<PluginApiResponse<BundleResponse>>, ApiError> {
    let user_id = auth.user_id.to_string();
    let bundle_mgr = state.bundle_manager.read().await;
    let runtime = state.runtime_registry.read().await;

    match bundle_mgr.get(&name) {
        Some(b) => {
            let active = runtime.is_active(&user_id, &name);
            Ok(Json(PluginApiResponse {
                success: true,
                data: Some(BundleResponse {
                    name: b.name.clone(),
                    version: b.version.clone(),
                    description: b.description.clone(),
                    author: b.author.clone(),
                    required_categories: b.required_categories.clone(),
                    optional_categories: b.optional_categories.clone(),
                    active,
                    has_prompt: b.system_prompt.is_some(),
                    mcp_server_count: b.mcp_servers.len(),
                    mcp_servers: build_mcp_server_statuses(
                        b,
                        &state.mcp_connection_tracker,
                        active,
                    ),
                }),
                error: None,
            }))
        }
        None => Ok(Json(PluginApiResponse {
            success: false,
            data: None,
            error: Some(format!("Bundle '{}' not found", name)),
        })),
    }
}

/// Activate a bundle for the current user.
///
/// POST /api/bundles/{name}/activate
async fn activate_bundle(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<PluginApiResponse<()>>, ApiError> {
    let user_id = auth.user_id.to_string();
    tracing::info!(user_id = %user_id, bundle = %name, "Activating bundle");

    // Check bundle exists and get MCP servers
    let (version, mcp_servers) = {
        let bundle_mgr = state.bundle_manager.read().await;
        match bundle_mgr.get(&name) {
            Some(b) => (b.version.clone(), b.mcp_servers.clone()),
            None => {
                return Ok(Json(PluginApiResponse {
                    success: false,
                    data: None,
                    error: Some(format!("Bundle '{}' not found", name)),
                }));
            }
        }
    };

    let mut runtime = state.runtime_registry.write().await;
    match runtime.activate_bundle(&user_id, &name, &version).await {
        Ok(()) => {
            // Wire MCP servers from this bundle
            for server_def in &mcp_servers {
                let is_first = state.mcp_ref_tracker.add_reference(&server_def.name, &name);
                if is_first {
                    // First bundle to reference this server — register & connect
                    let gw = state.mcp_gateway.clone();
                    let tracker = state.mcp_connection_tracker.clone();
                    let server_name = server_def.name.clone();
                    let server_url = server_def.url.clone();
                    let auth_token = server_def.auth_token.clone();

                    tracker.set_status(&server_name, McpConnectionStatus::Connecting);

                    tokio::spawn(async move {
                        use gateway_core::mcp::gateway::{
                            McpServerConfig as GwMcpServerConfig, McpTransport,
                        };

                        let config = GwMcpServerConfig {
                            name: server_name.clone(),
                            transport: McpTransport::Http { url: server_url },
                            enabled: true,
                            namespace: server_name.clone(),
                            startup_timeout_secs: 30,
                            auto_restart: false,
                            auth_token,
                        };

                        if let Err(e) = gw.register_server_config(config).await {
                            tracing::warn!(
                                server = %server_name,
                                error = %e,
                                "Failed to register MCP config"
                            );
                            tracker.set_status(
                                &server_name,
                                McpConnectionStatus::Failed(e.to_string()),
                            );
                            return;
                        }

                        match gw.connect_server(&server_name).await {
                            Ok(()) => {
                                tracing::info!(
                                    server = %server_name,
                                    "MCP server connected via bundle activation"
                                );
                                tracker.set_status(&server_name, McpConnectionStatus::Connected);
                            }
                            Err(e) => {
                                tracing::warn!(
                                    server = %server_name,
                                    error = %e,
                                    "MCP server connection failed"
                                );
                                tracker.set_status(
                                    &server_name,
                                    McpConnectionStatus::Failed(e.to_string()),
                                );
                            }
                        }
                    });
                }
            }

            Ok(Json(PluginApiResponse {
                success: true,
                data: Some(()),
                error: None,
            }))
        }
        Err(e) => Ok(Json(PluginApiResponse {
            success: false,
            data: None,
            error: Some(e.to_string()),
        })),
    }
}

/// Deactivate a bundle for the current user.
///
/// POST /api/bundles/{name}/deactivate
async fn deactivate_bundle(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<PluginApiResponse<()>>, ApiError> {
    let user_id = auth.user_id.to_string();
    tracing::info!(user_id = %user_id, bundle = %name, "Deactivating bundle");

    let mut runtime = state.runtime_registry.write().await;
    match runtime.deactivate_bundle(&user_id, &name).await {
        Ok(()) => {
            // Clean up orphaned MCP servers
            let orphaned = state.mcp_ref_tracker.remove_bundle(&name);
            for server_name in orphaned {
                tracing::info!(
                    server = %server_name,
                    bundle = %name,
                    "Disconnecting orphaned MCP server"
                );
                let _ = state
                    .mcp_gateway
                    .unregister_server_by_name(&server_name)
                    .await;
                state.mcp_connection_tracker.remove(&server_name);
            }

            Ok(Json(PluginApiResponse {
                success: true,
                data: Some(()),
                error: None,
            }))
        }
        Err(e) => Ok(Json(PluginApiResponse {
            success: false,
            data: None,
            error: Some(e.to_string()),
        })),
    }
}

/// List active bundles for the current user.
///
/// GET /api/bundles/active
async fn list_active_bundles(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<PluginApiResponse<Vec<String>>>, ApiError> {
    let user_id = auth.user_id.to_string();
    let runtime = state.runtime_registry.read().await;
    let active = runtime.get_active_bundles(&user_id);

    Ok(Json(PluginApiResponse {
        success: true,
        data: Some(active),
        error: None,
    }))
}

/// Reload bundle definitions.
///
/// POST /api/bundles/reload
///
/// Requires admin role.
async fn reload_bundles(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<PluginApiResponse<usize>>, ApiError> {
    if !auth.is_admin() {
        return Ok(Json(PluginApiResponse {
            success: false,
            data: None,
            error: Some("Admin access required".to_string()),
        }));
    }

    let mut bundle_mgr = state.bundle_manager.write().await;
    let count = bundle_mgr.reload();
    tracing::info!(bundles_loaded = count, admin = %auth.email, "Bundle definitions reloaded");

    Ok(Json(PluginApiResponse {
        success: true,
        data: Some(count),
        error: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_category_response_serialize() {
        let resp = CategoryResponse {
            id: "~~file-system".to_string(),
            display_name: "File System".to_string(),
            description: "Read and write files".to_string(),
            default_connectors: vec!["filesystem".to_string()],
            platform: None,
            icon: Some("folder".to_string()),
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["id"], "~~file-system");
        assert_eq!(json["display_name"], "File System");
    }

    #[test]
    fn test_bundle_response_serialize() {
        let resp = BundleResponse {
            name: "code-assistance".to_string(),
            version: "1.0.0".to_string(),
            description: "Code assistance bundle".to_string(),
            author: None,
            required_categories: vec!["~~file-system".to_string()],
            optional_categories: vec!["~~web-browser".to_string()],
            active: true,
            has_prompt: true,
            mcp_server_count: 2,
            mcp_servers: vec![McpServerStatusResponse {
                name: "slack".to_string(),
                url: "https://mcp.slack.com/mcp".to_string(),
                status: "connected".to_string(),
                error: None,
            }],
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["name"], "code-assistance");
        assert!(json["active"].as_bool().unwrap());
        assert_eq!(json["mcp_server_count"], 2);
        assert_eq!(json["mcp_servers"][0]["name"], "slack");
        assert_eq!(json["mcp_servers"][0]["status"], "connected");
    }

    #[test]
    fn test_connector_detail_response_serialize() {
        let resp = ConnectorDetailResponse {
            name: "pdf".to_string(),
            description: "PDF processing".to_string(),
            version: "1.0.0".to_string(),
            format: "ClaudeSkills".to_string(),
            author: Some("Canal".to_string()),
            skills_count: 1,
            references: vec!["REFERENCE.md".to_string()],
            has_scripts: true,
            has_mcp: false,
            installed: true,
            skill_names: vec!["pdf".to_string()],
            categories: vec!["~~pdf".to_string()],
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["name"], "pdf");
        assert!(json["installed"].as_bool().unwrap());
    }
}
