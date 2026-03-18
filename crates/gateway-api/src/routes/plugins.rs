//! Plugin store endpoints.
//!
//! REST API for plugin lifecycle management:
//!
//! - **Catalog**: Browse available plugins
//! - **Install/Uninstall**: Manage per-user plugin subscriptions
//! - **References**: Load plugin reference documents
//! - **Reload**: Admin re-scan of plugin directories

use axum::{
    extract::{Json, Path, State},
    routing::{get, post},
    Router,
};
use gateway_core::plugins::PluginApiResponse;
use serde::Serialize;

use crate::{error::ApiError, middleware::auth::AuthContext, state::AppState};

/// Create the plugin routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/catalog", get(browse_catalog))
        .route("/catalog/reload", post(reload_catalog))
        .route("/catalog/{name}", get(catalog_detail))
        .route("/installed", get(list_installed))
        .route("/install/{name}", post(install_plugin))
        .route("/uninstall/{name}", post(uninstall_plugin))
        .route("/{name}/references/{ref_name}", get(get_reference))
}

// ============================================================================
// Response Types
// ============================================================================

/// Catalog entry with full detail for API responses.
#[derive(Debug, Serialize)]
pub struct CatalogDetailResponse {
    /// Plugin name
    pub name: String,
    /// Plugin description
    pub description: String,
    /// Plugin version
    pub version: String,
    /// Plugin format (ClaudeSkills or Cowork)
    pub format: String,
    /// Plugin author
    pub author: Option<String>,
    /// Number of skills in this plugin
    pub skills_count: usize,
    /// Reference document names
    pub references: Vec<String>,
    /// Whether plugin has scripts directory
    pub has_scripts: bool,
    /// Whether plugin has MCP server configuration
    pub has_mcp: bool,
    /// Whether the requesting user has installed this plugin
    pub installed: bool,
    /// Skill names in this plugin
    pub skill_names: Vec<String>,
}

// ============================================================================
// Handler Functions
// ============================================================================

/// Browse the plugin catalog with user's install status.
///
/// GET /api/plugins/catalog
pub async fn browse_catalog(
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

/// Get detail for a single plugin.
///
/// GET /api/plugins/catalog/:name
pub async fn catalog_detail(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<PluginApiResponse<CatalogDetailResponse>>, ApiError> {
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
                error: Some(format!("Plugin '{}' not found", name)),
            }));
        }
    };

    // Get full plugin details from catalog
    let catalog = state.plugin_manager.catalog.read().await;
    let detail = if let Some(plugin) = catalog.get(&name) {
        CatalogDetailResponse {
            name: entry.name,
            description: entry.description,
            version: entry.version,
            format: format!("{:?}", entry.format),
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
        }
    } else {
        CatalogDetailResponse {
            name: entry.name,
            description: entry.description,
            version: entry.version,
            format: format!("{:?}", entry.format),
            author: entry.author,
            skills_count: entry.skills_count,
            references: Vec::new(),
            has_scripts: false,
            has_mcp: false,
            installed: entry.installed,
            skill_names: Vec::new(),
        }
    };

    Ok(Json(PluginApiResponse {
        success: true,
        data: Some(detail),
        error: None,
    }))
}

/// Reload the plugin catalog (admin operation).
///
/// POST /api/plugins/catalog/reload
///
/// Requires admin role.
pub async fn reload_catalog(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<PluginApiResponse<usize>>, ApiError> {
    // R4-M: Return proper 403 instead of 200 with success:false
    if !auth.is_admin() {
        return Err(ApiError::forbidden("Admin access required"));
    }

    let count = state.plugin_manager.reload_catalog().await;
    tracing::info!(plugins_loaded = count, admin = %auth.email, "Plugin catalog reloaded");

    Ok(Json(PluginApiResponse {
        success: true,
        data: Some(count),
        error: None,
    }))
}

/// List user's installed plugins.
///
/// GET /api/plugins/installed
pub async fn list_installed(
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

/// Install a plugin for the current user.
///
/// POST /api/plugins/install/:name
pub async fn install_plugin(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<PluginApiResponse<()>>, ApiError> {
    let user_id = auth.user_id.to_string();
    tracing::info!(user_id = %user_id, plugin = %name, "Installing plugin");

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

/// Uninstall a plugin for the current user.
///
/// POST /api/plugins/uninstall/:name
pub async fn uninstall_plugin(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Json<PluginApiResponse<()>>, ApiError> {
    let user_id = auth.user_id.to_string();
    tracing::info!(user_id = %user_id, plugin = %name, "Uninstalling plugin");

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

/// Load a reference document from a plugin.
///
/// GET /api/plugins/:name/references/:ref_name
///
/// Requires authentication.
pub async fn get_reference(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_detail_response_serialization() {
        let detail = CatalogDetailResponse {
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
        };

        let json = serde_json::to_value(&detail).unwrap();
        assert_eq!(json["name"], "pdf");
        assert_eq!(json["skills_count"], 1);
        assert!(json["installed"].as_bool().unwrap());
        assert!(json["has_scripts"].as_bool().unwrap());
        assert!(!json["has_mcp"].as_bool().unwrap());
    }

    #[test]
    fn test_plugin_api_response_success() {
        let response: PluginApiResponse<Vec<String>> = PluginApiResponse {
            success: true,
            data: Some(vec!["pdf".to_string()]),
            error: None,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(json["success"].as_bool().unwrap());
        assert!(json["data"].is_array());
        assert!(json["error"].is_null());
    }

    #[test]
    fn test_plugin_api_response_error() {
        let response: PluginApiResponse<()> = PluginApiResponse {
            success: false,
            data: None,
            error: Some("Plugin not found".to_string()),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert!(!json["success"].as_bool().unwrap());
        assert!(json["data"].is_null());
        assert_eq!(json["error"], "Plugin not found");
    }
}
