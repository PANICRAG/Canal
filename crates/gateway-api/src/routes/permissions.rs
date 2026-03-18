//! Permission Management API endpoints
//!
//! Provides REST API for managing permission rules, modes, and user approval dialogs.

use axum::{
    extract::State,
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use gateway_core::agent::{
    PermissionBehavior, PermissionDestination, PermissionMode, PermissionRule, PermissionUpdate,
};
use serde::{Deserialize, Serialize};

use crate::{error::ApiError, middleware::auth::AuthContext, state::AppState};

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

/// Create the permission routes — admin-only access.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Mode management
        .route("/mode", get(get_permission_mode))
        .route("/mode", put(set_permission_mode))
        // Rule management
        .route("/rules", get(list_permission_rules))
        .route("/rules", post(add_permission_rules))
        .route("/rules", delete(remove_permission_rules))
        .route("/rules/clear", post(clear_session_rules))
        // Directory management
        .route("/directories", get(list_allowed_directories))
        .route("/directories", post(add_allowed_directory))
        .route("/directories", delete(remove_allowed_directory))
        // Permission updates (batch)
        .route("/update", post(apply_permission_update))
        // Tool permission check (for UI preview)
        .route("/check", post(check_tool_permission))
        // All permission routes require admin role
        .layer(axum::middleware::from_fn(require_admin))
}

// ============================================================================
// Request/Response Types
// ============================================================================

/// Permission mode response
#[derive(Debug, Serialize)]
pub struct PermissionModeResponse {
    pub mode: String,
    pub allows_edits: bool,
    pub auto_approve: bool,
    pub auto_approve_edits: bool,
}

impl From<PermissionMode> for PermissionModeResponse {
    fn from(mode: PermissionMode) -> Self {
        Self {
            mode: match mode {
                PermissionMode::Default => "default",
                PermissionMode::AcceptEdits => "accept_edits",
                PermissionMode::Plan => "plan",
                PermissionMode::BypassPermissions => "bypass_permissions",
            }
            .to_string(),
            allows_edits: mode.allows_edits(),
            auto_approve: mode.auto_approve(),
            auto_approve_edits: mode.auto_approve_edits(),
        }
    }
}

/// Set permission mode request
#[derive(Debug, Deserialize)]
pub struct SetPermissionModeRequest {
    /// Permission mode: "default", "accept_edits", "plan", "bypass_permissions"
    pub mode: String,
    /// Session ID (optional - applies to session if provided)
    #[serde(default)]
    pub session_id: Option<String>,
}

impl SetPermissionModeRequest {
    fn parse_mode(&self) -> Result<PermissionMode, ApiError> {
        match self.mode.as_str() {
            "default" => Ok(PermissionMode::Default),
            "accept_edits" => Ok(PermissionMode::AcceptEdits),
            "plan" => Ok(PermissionMode::Plan),
            "bypass_permissions" => Ok(PermissionMode::BypassPermissions),
            _ => Err(ApiError::bad_request(format!(
                "Invalid permission mode: {}. Valid values: default, accept_edits, plan, bypass_permissions",
                self.mode
            ))),
        }
    }
}

/// Permission rule response
#[derive(Debug, Serialize)]
pub struct PermissionRuleResponse {
    pub tool: Option<String>,
    pub path: Option<String>,
    pub command: Option<String>,
    pub behavior: String,
}

/// Permission rules list response
#[derive(Debug, Serialize)]
pub struct PermissionRulesListResponse {
    pub rules: Vec<PermissionRuleResponse>,
    pub count: usize,
}

/// Add permission rules request
#[derive(Debug, Deserialize)]
pub struct AddPermissionRulesRequest {
    /// Rules to add
    pub rules: Vec<PermissionRuleInput>,
    /// Behavior for all rules
    pub behavior: String,
    /// Destination: "session", "user_settings", "project_settings", "local_settings"
    #[serde(default = "default_session")]
    pub destination: String,
}

fn default_session() -> String {
    "session".to_string()
}

/// Permission rule input
#[derive(Debug, Deserialize)]
pub struct PermissionRuleInput {
    /// Tool name pattern
    #[serde(default)]
    pub tool: Option<String>,
    /// Path pattern
    #[serde(default)]
    pub path: Option<String>,
    /// Command pattern
    #[serde(default)]
    pub command: Option<String>,
}

impl From<PermissionRuleInput> for PermissionRule {
    fn from(input: PermissionRuleInput) -> Self {
        PermissionRule {
            tool: input.tool,
            path: input.path,
            command: input.command,
        }
    }
}

/// Remove permission rules request
#[derive(Debug, Deserialize)]
pub struct RemovePermissionRulesRequest {
    /// Rules to remove
    pub rules: Vec<PermissionRuleInput>,
    /// Behavior of rules to remove
    pub behavior: String,
    /// Destination
    #[serde(default = "default_session")]
    pub destination: String,
}

/// Allowed directory request
#[derive(Debug, Deserialize)]
pub struct AllowedDirectoryRequest {
    /// Directory path
    pub directory: String,
}

/// Allowed directories response
#[derive(Debug, Serialize)]
pub struct AllowedDirectoriesResponse {
    pub directories: Vec<String>,
    pub count: usize,
}

/// Permission update request (batch)
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum PermissionUpdateRequest {
    /// Add rules
    #[serde(rename = "add_rules")]
    AddRules {
        rules: Vec<PermissionRuleInput>,
        behavior: String,
        destination: String,
    },
    /// Replace rules
    #[serde(rename = "replace_rules")]
    ReplaceRules {
        rules: Vec<PermissionRuleInput>,
        behavior: String,
        destination: String,
    },
    /// Remove rules
    #[serde(rename = "remove_rules")]
    RemoveRules {
        rules: Vec<PermissionRuleInput>,
        behavior: String,
        destination: String,
    },
    /// Set mode
    #[serde(rename = "set_mode")]
    SetMode { mode: String, destination: String },
    /// Add directories
    #[serde(rename = "add_directories")]
    AddDirectories {
        directories: Vec<String>,
        destination: String,
    },
    /// Remove directories
    #[serde(rename = "remove_directories")]
    RemoveDirectories {
        directories: Vec<String>,
        destination: String,
    },
}

/// Check tool permission request
#[derive(Debug, Deserialize)]
pub struct CheckToolPermissionRequest {
    /// Tool name
    pub tool_name: String,
    /// Tool input
    pub tool_input: serde_json::Value,
    /// Session ID (optional)
    #[serde(default)]
    pub session_id: Option<String>,
    /// Working directory (optional)
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Check tool permission response
#[derive(Debug, Serialize)]
pub struct CheckToolPermissionResponse {
    /// Result type: "allow", "deny", "ask"
    pub result: String,
    /// Updated input (if modified)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<serde_json::Value>,
    /// Message (for deny/ask)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Whether execution should be interrupted (for deny)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<bool>,
}

// ============================================================================
// Helper Functions
// ============================================================================

fn parse_behavior(s: &str) -> Result<PermissionBehavior, ApiError> {
    match s {
        "allow" => Ok(PermissionBehavior::Allow),
        "deny" => Ok(PermissionBehavior::Deny),
        "ask" => Ok(PermissionBehavior::Ask),
        _ => Err(ApiError::bad_request(format!(
            "Invalid behavior: {}. Valid values: allow, deny, ask",
            s
        ))),
    }
}

fn parse_destination(s: &str) -> Result<PermissionDestination, ApiError> {
    match s {
        "session" => Ok(PermissionDestination::Session),
        "user_settings" => Ok(PermissionDestination::UserSettings),
        "project_settings" => Ok(PermissionDestination::ProjectSettings),
        "local_settings" => Ok(PermissionDestination::LocalSettings),
        _ => Err(ApiError::bad_request(format!(
            "Invalid destination: {}. Valid values: session, user_settings, project_settings, local_settings",
            s
        ))),
    }
}

fn parse_mode(s: &str) -> Result<PermissionMode, ApiError> {
    match s {
        "default" => Ok(PermissionMode::Default),
        "accept_edits" => Ok(PermissionMode::AcceptEdits),
        "plan" => Ok(PermissionMode::Plan),
        "bypass_permissions" => Ok(PermissionMode::BypassPermissions),
        _ => Err(ApiError::bad_request(format!(
            "Invalid mode: {}. Valid values: default, accept_edits, plan, bypass_permissions",
            s
        ))),
    }
}

// ============================================================================
// Handlers
// ============================================================================

/// Get current permission mode
pub async fn get_permission_mode(
    State(state): State<AppState>,
) -> Result<Json<PermissionModeResponse>, ApiError> {
    let mode = state.permission_manager.mode().await;
    Ok(Json(mode.into()))
}

/// Set permission mode
pub async fn set_permission_mode(
    State(state): State<AppState>,
    Json(request): Json<SetPermissionModeRequest>,
) -> Result<Json<PermissionModeResponse>, ApiError> {
    let mode = request.parse_mode()?;

    tracing::info!(
        mode = ?mode,
        session_id = ?request.session_id,
        "Setting permission mode"
    );

    state.permission_manager.set_mode(mode).await;
    Ok(Json(mode.into()))
}

/// List permission rules
pub async fn list_permission_rules(
    State(state): State<AppState>,
) -> Result<Json<PermissionRulesListResponse>, ApiError> {
    let context = state.permission_manager.build_context(None, None).await;

    let rules: Vec<PermissionRuleResponse> = context
        .rules
        .iter()
        .map(|(rule, behavior)| PermissionRuleResponse {
            tool: rule.tool.clone(),
            path: rule.path.clone(),
            command: rule.command.clone(),
            behavior: match behavior {
                PermissionBehavior::Allow => "allow",
                PermissionBehavior::Deny => "deny",
                PermissionBehavior::Ask => "ask",
            }
            .to_string(),
        })
        .collect();

    let count = rules.len();
    Ok(Json(PermissionRulesListResponse { rules, count }))
}

/// Add permission rules
pub async fn add_permission_rules(
    State(state): State<AppState>,
    Json(request): Json<AddPermissionRulesRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let behavior = parse_behavior(&request.behavior)?;
    let destination = parse_destination(&request.destination)?;

    let rules: Vec<PermissionRule> = request.rules.into_iter().map(Into::into).collect();
    let count = rules.len();

    tracing::info!(
        count = count,
        behavior = ?behavior,
        destination = ?destination,
        "Adding permission rules"
    );

    state
        .permission_manager
        .apply_update(PermissionUpdate::AddRules {
            rules,
            behavior,
            destination,
        })
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "added": count
    })))
}

/// Remove permission rules
pub async fn remove_permission_rules(
    State(state): State<AppState>,
    Json(request): Json<RemovePermissionRulesRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let behavior = parse_behavior(&request.behavior)?;
    let destination = parse_destination(&request.destination)?;

    let rules: Vec<PermissionRule> = request.rules.into_iter().map(Into::into).collect();
    let count = rules.len();

    tracing::info!(
        count = count,
        behavior = ?behavior,
        destination = ?destination,
        "Removing permission rules"
    );

    state
        .permission_manager
        .apply_update(PermissionUpdate::RemoveRules {
            rules,
            behavior,
            destination,
        })
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "removed": count
    })))
}

/// Clear all session rules
pub async fn clear_session_rules(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    tracing::info!("Clearing all session permission rules");

    state.permission_manager.clear_session_rules().await;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Session rules cleared"
    })))
}

/// List allowed directories
pub async fn list_allowed_directories(
    State(state): State<AppState>,
) -> Result<Json<AllowedDirectoriesResponse>, ApiError> {
    let context = state.permission_manager.build_context(None, None).await;
    let directories = context.allowed_directories;
    let count = directories.len();

    Ok(Json(AllowedDirectoriesResponse { directories, count }))
}

/// Add allowed directory
pub async fn add_allowed_directory(
    State(state): State<AppState>,
    Json(request): Json<AllowedDirectoryRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    tracing::info!(directory = %request.directory, "Adding allowed directory");

    state
        .permission_manager
        .add_allowed_directory(&request.directory)
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "directory": request.directory
    })))
}

/// Remove allowed directory
pub async fn remove_allowed_directory(
    State(state): State<AppState>,
    Json(request): Json<AllowedDirectoryRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    tracing::info!(directory = %request.directory, "Removing allowed directory");

    state
        .permission_manager
        .remove_allowed_directory(&request.directory)
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "directory": request.directory
    })))
}

/// Apply permission update (batch)
pub async fn apply_permission_update(
    State(state): State<AppState>,
    Json(request): Json<PermissionUpdateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let update = match request {
        PermissionUpdateRequest::AddRules {
            rules,
            behavior,
            destination,
        } => PermissionUpdate::AddRules {
            rules: rules.into_iter().map(Into::into).collect(),
            behavior: parse_behavior(&behavior)?,
            destination: parse_destination(&destination)?,
        },
        PermissionUpdateRequest::ReplaceRules {
            rules,
            behavior,
            destination,
        } => PermissionUpdate::ReplaceRules {
            rules: rules.into_iter().map(Into::into).collect(),
            behavior: parse_behavior(&behavior)?,
            destination: parse_destination(&destination)?,
        },
        PermissionUpdateRequest::RemoveRules {
            rules,
            behavior,
            destination,
        } => PermissionUpdate::RemoveRules {
            rules: rules.into_iter().map(Into::into).collect(),
            behavior: parse_behavior(&behavior)?,
            destination: parse_destination(&destination)?,
        },
        PermissionUpdateRequest::SetMode { mode, destination } => PermissionUpdate::SetMode {
            mode: parse_mode(&mode)?,
            destination: parse_destination(&destination)?,
        },
        PermissionUpdateRequest::AddDirectories {
            directories,
            destination,
        } => PermissionUpdate::AddDirectories {
            directories,
            destination: parse_destination(&destination)?,
        },
        PermissionUpdateRequest::RemoveDirectories {
            directories,
            destination,
        } => PermissionUpdate::RemoveDirectories {
            directories,
            destination: parse_destination(&destination)?,
        },
    };

    tracing::info!(update = ?update, "Applying permission update");

    state.permission_manager.apply_update(update).await;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Permission update applied"
    })))
}

/// Check tool permission (preview)
pub async fn check_tool_permission(
    State(state): State<AppState>,
    Json(request): Json<CheckToolPermissionRequest>,
) -> Result<Json<CheckToolPermissionResponse>, ApiError> {
    let result = state
        .permission_manager
        .check_tool(
            &request.tool_name,
            &request.tool_input,
            request.session_id.as_deref(),
            request.cwd.as_deref(),
        )
        .await;

    let response = match result {
        gateway_core::agent::PermissionResult::Allow { updated_input, .. } => {
            CheckToolPermissionResponse {
                result: "allow".to_string(),
                updated_input,
                message: None,
                interrupt: None,
            }
        }
        gateway_core::agent::PermissionResult::Deny { message, interrupt } => {
            CheckToolPermissionResponse {
                result: "deny".to_string(),
                updated_input: None,
                message: Some(message),
                interrupt: Some(interrupt),
            }
        }
        gateway_core::agent::PermissionResult::Ask { question, .. } => {
            CheckToolPermissionResponse {
                result: "ask".to_string(),
                updated_input: None,
                message: Some(question),
                interrupt: None,
            }
        }
    };

    Ok(Json(response))
}
