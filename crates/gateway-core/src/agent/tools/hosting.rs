//! Hosting Tools — Deploy and manage web applications
//!
//! Provides agent tools for creating, deploying, and managing hosted web apps
//! via HTTP calls to the platform-service Hosting API. This enables the AI agent
//! to deploy apps from GitHub repos through natural language conversation.

use super::{AgentTool, ToolContext, ToolError, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// LLMs sometimes send numeric values as strings (e.g., `"3000"` instead of `3000`).
/// This deserializer accepts both.
fn deserialize_optional_u16_lenient<'de, D>(deserializer: D) -> Result<Option<u16>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct U16LenientVisitor;
    impl<'de> de::Visitor<'de> for U16LenientVisitor {
        type Value = Option<u16>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a u16 integer or numeric string")
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_some<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_any(U16InnerVisitor).map(Some)
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            u16::try_from(v).map(Some).map_err(de::Error::custom)
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            u16::try_from(v).map(Some).map_err(de::Error::custom)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            v.parse::<u16>().map(Some).map_err(de::Error::custom)
        }
    }
    struct U16InnerVisitor;
    impl<'de> de::Visitor<'de> for U16InnerVisitor {
        type Value = u16;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a u16 integer or numeric string")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
            u16::try_from(v).map_err(de::Error::custom)
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
            u16::try_from(v).map_err(de::Error::custom)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            v.parse::<u16>().map_err(de::Error::custom)
        }
    }
    deserializer.deserialize_option(U16LenientVisitor)
}

/// Dynamic token provider for service-to-service authentication.
/// Called on each request to get a fresh token (e.g., RS256 service JWT).
pub type TokenProvider = Arc<dyn Fn() -> String + Send + Sync>;

/// Configuration for hosting tools (platform-service API)
#[derive(Clone)]
pub struct HostingToolConfig {
    /// Base URL for the platform-service (e.g., "http://localhost:8080")
    pub base_url: String,
    /// Static fallback token (used when no token_provider is set)
    auth_token: String,
    /// Dynamic token generator (e.g., RS256 service token via KeyPair).
    /// Takes priority over static auth_token when present.
    token_provider: Option<TokenProvider>,
}

impl std::fmt::Debug for HostingToolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostingToolConfig")
            .field("base_url", &self.base_url)
            .field("auth_token", &"<redacted>")
            .field("has_token_provider", &self.token_provider.is_some())
            .finish()
    }
}

impl HostingToolConfig {
    /// Create config with a static auth token (legacy/fallback).
    pub fn new(base_url: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            auth_token: auth_token.into(),
            token_provider: None,
        }
    }

    /// Create config with a dynamic token provider (preferred).
    /// The provider is called on each request to generate a fresh token.
    pub fn with_token_provider(base_url: impl Into<String>, provider: TokenProvider) -> Self {
        Self {
            base_url: base_url.into(),
            auth_token: String::new(),
            token_provider: Some(provider),
        }
    }

    /// Get the auth token — uses dynamic provider if available, else static token.
    pub fn get_token(&self) -> String {
        if let Some(provider) = &self.token_provider {
            provider()
        } else {
            self.auth_token.clone()
        }
    }
}

/// R1-C11: Validate that an ID from LLM input is safe for URL path interpolation.
/// Rejects IDs containing path traversal characters (/, \, ..) or whitespace.
fn validate_path_id(id: &str, field_name: &str) -> Result<(), ToolError> {
    if id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id.contains(char::is_whitespace)
    {
        return Err(ToolError::InvalidInput(format!(
            "Invalid {}: contains path traversal or whitespace characters",
            field_name
        )));
    }
    Ok(())
}

/// HTTP client helper for hosting API calls
async fn hosting_request(
    config: &HostingToolConfig,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, ToolError> {
    let url = format!("{}{}", config.base_url, path);
    // R1-M: Reuse shared client for connection pooling
    static HTTP_CLIENT: std::sync::LazyLock<reqwest::Client> =
        std::sync::LazyLock::new(reqwest::Client::new);
    let client = &*HTTP_CLIENT;

    let mut request = match method {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        _ => {
            return Err(ToolError::InvalidInput(format!(
                "Unsupported method: {}",
                method
            )))
        }
    };

    request = request
        .header("Authorization", format!("Bearer {}", config.get_token()))
        .header("Content-Type", "application/json");

    if let Some(body) = body {
        request = request.json(&body);
    }

    let response = request
        .send()
        .await
        .map_err(|e| ToolError::ExecutionError(format!("Hosting API request failed: {}", e)))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| ToolError::ExecutionError(format!("Failed to read response: {}", e)))?;

    if !status.is_success() {
        return Err(ToolError::ExecutionError(format!(
            "Hosting API returned HTTP {}: {}",
            status, text
        )));
    }

    if text.is_empty() {
        Ok(serde_json::json!({"success": true}))
    } else {
        serde_json::from_str(&text)
            .map_err(|e| ToolError::ExecutionError(format!("Invalid JSON response: {}", e)))
    }
}

// =============================================================================
// hosting_deploy_app — The main one-shot deploy tool
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct DeployAppInput {
    /// App name (becomes the subdomain)
    pub name: String,
    /// GitHub repository URL (e.g., "https://github.com/user/repo")
    pub git_repo: String,
    /// Git token for private repos (GitHub PAT). Optional for public repos.
    #[serde(default)]
    pub git_token: Option<String>,
    /// Git branch to deploy (default: "main")
    #[serde(default)]
    pub git_branch: Option<String>,
    /// Framework hint (e.g., "nextjs", "react", "python", "go", "dockerfile"). Auto-detected if omitted.
    #[serde(default)]
    pub framework: Option<String>,
    /// Container port the app listens on (default: 3000)
    #[serde(default, deserialize_with = "deserialize_optional_u16_lenient")]
    pub container_port: Option<u16>,
    /// Environment variables as key-value pairs
    #[serde(default)]
    pub env_vars: Option<std::collections::HashMap<String, String>>,
    /// Provision a managed database (Supabase) for this app. Credentials are auto-injected as env vars.
    #[serde(default)]
    pub provision_database: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct DeployAppOutput {
    /// The created app details
    pub app: serde_json::Value,
    /// Deployment result (if deploy was triggered)
    pub deployment: Option<serde_json::Value>,
    /// The public URL where the app will be accessible
    pub url: String,
    /// Current status
    pub status: String,
}

/// One-shot tool: create a hosted app from a GitHub repo and deploy it.
/// Combines app creation + deployment trigger into a single tool call.
pub struct HostingDeployAppTool {
    config: Arc<HostingToolConfig>,
}

impl HostingDeployAppTool {
    pub fn new(config: Arc<HostingToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for HostingDeployAppTool {
    type Input = DeployAppInput;
    type Output = DeployAppOutput;

    fn name(&self) -> &str {
        "hosting_deploy_app"
    }

    fn description(&self) -> &str {
        "Deploy a web application from a GitHub repository. Creates the app and triggers \
         a build+deploy pipeline. Supports public and private repos (provide git_token for private). \
         The app will be accessible at https://{name}.apps.example.com once deployed. \
         Supports frameworks: Next.js, React/Vite, Vue, Svelte, Python, Go, Rust, static sites, \
         and any project with a Dockerfile."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "App name (lowercase, alphanumeric + hyphens). Becomes the subdomain."
                },
                "git_repo": {
                    "type": "string",
                    "description": "GitHub repository URL (e.g., https://github.com/user/repo)"
                },
                "git_token": {
                    "type": "string",
                    "description": "GitHub Personal Access Token for private repos. Optional for public repos."
                },
                "git_branch": {
                    "type": "string",
                    "description": "Branch to deploy (default: main)"
                },
                "framework": {
                    "type": "string",
                    "enum": ["nextjs", "react", "vue", "svelte", "python", "go", "rust", "static", "dockerfile", "other"],
                    "description": "Framework type. Auto-detected from source if omitted."
                },
                "container_port": {
                    "type": "integer",
                    "description": "Port the app listens on inside the container (default: 3000)"
                },
                "env_vars": {
                    "type": "object",
                    "description": "Environment variables as key-value pairs",
                    "additionalProperties": { "type": "string" }
                },
                "provision_database": {
                    "type": "boolean",
                    "description": "Provision a managed Supabase database. Credentials (SUPABASE_URL, ANON_KEY, etc.) are auto-injected into the app's environment."
                }
            },
            "required": ["name", "git_repo"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "hosting"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        // Step 1: Create the app
        let framework = input.framework.as_deref().unwrap_or("other");
        let branch = input.git_branch.as_deref().unwrap_or("main");
        let port = input.container_port.unwrap_or(3000);

        let mut deploy_config = serde_json::json!({
            "git_repo": input.git_repo,
            "git_branch": branch,
            "container_port": port,
        });

        if let Some(token) = &input.git_token {
            deploy_config["git_token"] = serde_json::json!(token);
        }
        if let Some(env) = &input.env_vars {
            deploy_config["env_vars"] = serde_json::json!(env);
        }

        let create_body = serde_json::json!({
            "name": input.name,
            "framework": framework,
            "git_repo": input.git_repo,
            "deploy_config": deploy_config,
        });

        let app =
            hosting_request(&self.config, "POST", "/api/hosting/apps", Some(create_body)).await?;

        let app_id = app["id"]
            .as_str()
            .ok_or_else(|| ToolError::ExecutionError("No app ID in response".into()))?;
        let url = app["url"].as_str().unwrap_or("unknown").to_string();

        // Step 2: Provision database if requested
        let mut db_result = None;
        if input.provision_database.unwrap_or(false) {
            let db_path = format!("/api/hosting/apps/{}/database", app_id);
            match hosting_request(
                &self.config,
                "POST",
                &db_path,
                Some(serde_json::json!({"region": "ap-southeast-1", "plan": "free"})),
            )
            .await
            {
                Ok(db) => {
                    db_result = Some(db);
                    tracing::info!(
                        app_id,
                        "Database provisioned, credentials injected into app env"
                    );
                }
                Err(e) => {
                    tracing::warn!(app_id, error = %e, "Database provisioning failed, continuing with deploy");
                }
            }
        }

        // Step 3: Trigger deployment
        let deploy_body = serde_json::json!({
            "provision_database": input.provision_database.unwrap_or(false),
        });
        let deploy_path = format!("/api/hosting/apps/{}/deploy", app_id);
        let deploy_result =
            hosting_request(&self.config, "POST", &deploy_path, Some(deploy_body)).await;

        match deploy_result {
            Ok(deployment) => {
                let mut status = "deploying".to_string();
                if db_result.is_some() {
                    status = "deploying (database provisioned)".to_string();
                }
                Ok(DeployAppOutput {
                    app,
                    deployment: Some(deployment),
                    url,
                    status,
                })
            }
            Err(e) => {
                // App was created but deploy failed — report both
                Ok(DeployAppOutput {
                    app,
                    deployment: None,
                    url,
                    status: format!("created but deploy failed: {}", e),
                })
            }
        }
    }
}

// =============================================================================
// hosting_list_apps
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListAppsInput {}

#[derive(Debug, Serialize)]
pub struct ListAppsOutput {
    pub apps: serde_json::Value,
}

/// List all deployed apps with their status and URLs
pub struct HostingListAppsTool {
    config: Arc<HostingToolConfig>,
}

impl HostingListAppsTool {
    pub fn new(config: Arc<HostingToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for HostingListAppsTool {
    type Input = ListAppsInput;
    type Output = ListAppsOutput;

    fn name(&self) -> &str {
        "hosting_list_apps"
    }

    fn description(&self) -> &str {
        "List all hosted web applications with their status, URLs, and framework info."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "hosting"
    }

    async fn execute(
        &self,
        _input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let result = hosting_request(&self.config, "GET", "/api/hosting/apps", None).await?;
        Ok(ListAppsOutput { apps: result })
    }
}

// =============================================================================
// hosting_app_status
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct AppStatusInput {
    /// App ID or subdomain name
    pub app_id: String,
}

#[derive(Debug, Serialize)]
pub struct AppStatusOutput {
    pub app: serde_json::Value,
}

/// Get detailed status of a hosted app
pub struct HostingAppStatusTool {
    config: Arc<HostingToolConfig>,
}

impl HostingAppStatusTool {
    pub fn new(config: Arc<HostingToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for HostingAppStatusTool {
    type Input = AppStatusInput;
    type Output = AppStatusOutput;

    fn name(&self) -> &str {
        "hosting_app_status"
    }

    fn description(&self) -> &str {
        "Get the current status of a hosted app including deployment info, URL, and health."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": {
                    "type": "string",
                    "description": "The app ID (UUID)"
                }
            },
            "required": ["app_id"]
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "hosting"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        let path = format!("/api/hosting/apps/{}", input.app_id);
        let result = hosting_request(&self.config, "GET", &path, None).await?;
        Ok(AppStatusOutput { app: result })
    }
}

// =============================================================================
// hosting_app_logs
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct AppLogsInput {
    /// App ID
    pub app_id: String,
    /// Number of log lines to return (default: 100)
    #[serde(default)]
    pub tail: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct AppLogsOutput {
    pub logs: serde_json::Value,
}

/// Get logs from a hosted app's container
pub struct HostingAppLogsTool {
    config: Arc<HostingToolConfig>,
}

impl HostingAppLogsTool {
    pub fn new(config: Arc<HostingToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for HostingAppLogsTool {
    type Input = AppLogsInput;
    type Output = AppLogsOutput;

    fn name(&self) -> &str {
        "hosting_app_logs"
    }

    fn description(&self) -> &str {
        "Get recent logs from a hosted app's container for debugging."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": {
                    "type": "string",
                    "description": "The app ID (UUID)"
                },
                "tail": {
                    "type": "integer",
                    "description": "Number of log lines (default: 100)"
                }
            },
            "required": ["app_id"]
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "hosting"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        let tail = input.tail.unwrap_or(100);
        let path = format!("/api/hosting/apps/{}/logs?tail={}", input.app_id, tail);
        let result = hosting_request(&self.config, "POST", &path, None).await?;
        Ok(AppLogsOutput { logs: result })
    }
}

// =============================================================================
// hosting_stop_app
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct StopAppInput {
    /// App ID
    pub app_id: String,
}

#[derive(Debug, Serialize)]
pub struct StopAppOutput {
    pub result: serde_json::Value,
}

/// Stop a running hosted app
pub struct HostingStopAppTool {
    config: Arc<HostingToolConfig>,
}

impl HostingStopAppTool {
    pub fn new(config: Arc<HostingToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for HostingStopAppTool {
    type Input = StopAppInput;
    type Output = StopAppOutput;

    fn name(&self) -> &str {
        "hosting_stop_app"
    }

    fn description(&self) -> &str {
        "Stop a running hosted app. The container will be stopped but not deleted."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": {
                    "type": "string",
                    "description": "The app ID (UUID)"
                }
            },
            "required": ["app_id"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "hosting"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        let path = format!("/api/hosting/apps/{}/stop", input.app_id);
        let result = hosting_request(&self.config, "POST", &path, None).await?;
        Ok(StopAppOutput { result })
    }
}

// =============================================================================
// hosting_delete_app
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct DeleteAppInput {
    /// App ID
    pub app_id: String,
}

#[derive(Debug, Serialize)]
pub struct DeleteAppOutput {
    pub result: serde_json::Value,
}

/// Delete a hosted app and its container
pub struct HostingDeleteAppTool {
    config: Arc<HostingToolConfig>,
}

impl HostingDeleteAppTool {
    pub fn new(config: Arc<HostingToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for HostingDeleteAppTool {
    type Input = DeleteAppInput;
    type Output = DeleteAppOutput;

    fn name(&self) -> &str {
        "hosting_delete_app"
    }

    fn description(&self) -> &str {
        "Delete a hosted app. Destroys the container, removes routing, and cleans up all resources."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": {
                    "type": "string",
                    "description": "The app ID (UUID)"
                }
            },
            "required": ["app_id"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "hosting"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        let path = format!("/api/hosting/apps/{}", input.app_id);
        let result = hosting_request(&self.config, "DELETE", &path, None).await?;
        Ok(DeleteAppOutput { result })
    }
}

// =============================================================================
// hosting_create_database — Provision a managed database for an app
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateDatabaseInput {
    /// App ID to provision the database for
    pub app_id: String,
    /// Database region (default: ap-southeast-1)
    #[serde(default)]
    pub region: Option<String>,
    /// Plan: "free" or "pro" (default: free)
    #[serde(default)]
    pub plan: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateDatabaseOutput {
    pub database: serde_json::Value,
}

/// Provision a managed Supabase database for a hosted app
pub struct HostingCreateDatabaseTool {
    config: Arc<HostingToolConfig>,
}

impl HostingCreateDatabaseTool {
    pub fn new(config: Arc<HostingToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for HostingCreateDatabaseTool {
    type Input = CreateDatabaseInput;
    type Output = CreateDatabaseOutput;

    fn name(&self) -> &str {
        "hosting_create_database"
    }

    fn description(&self) -> &str {
        "Provision a managed Supabase database for a hosted app. \
         Database credentials (SUPABASE_URL, ANON_KEY, SERVICE_ROLE_KEY, DB_URL) \
         are automatically injected into the app's environment variables."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": {
                    "type": "string",
                    "description": "The app ID (UUID) to provision a database for"
                },
                "region": {
                    "type": "string",
                    "description": "Database region (default: ap-southeast-1)",
                    "enum": ["ap-southeast-1", "us-east-1", "eu-west-1"]
                },
                "plan": {
                    "type": "string",
                    "description": "Database plan (default: free)",
                    "enum": ["free", "pro"]
                }
            },
            "required": ["app_id"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "hosting"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        let path = format!("/api/hosting/apps/{}/database", input.app_id);
        let body = serde_json::json!({
            "region": input.region.unwrap_or_else(|| "ap-southeast-1".to_string()),
            "plan": input.plan.unwrap_or_else(|| "free".to_string()),
        });
        let result = hosting_request(&self.config, "POST", &path, Some(body)).await?;
        Ok(CreateDatabaseOutput { database: result })
    }
}

// =============================================================================
// hosting_database_status — Check database status for an app
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct DatabaseStatusInput {
    /// App ID
    pub app_id: String,
}

#[derive(Debug, Serialize)]
pub struct DatabaseStatusOutput {
    pub database: serde_json::Value,
}

/// Get the database status and credentials for a hosted app
pub struct HostingDatabaseStatusTool {
    config: Arc<HostingToolConfig>,
}

impl HostingDatabaseStatusTool {
    pub fn new(config: Arc<HostingToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for HostingDatabaseStatusTool {
    type Input = DatabaseStatusInput;
    type Output = DatabaseStatusOutput;

    fn name(&self) -> &str {
        "hosting_database_status"
    }

    fn description(&self) -> &str {
        "Get the database status, connection details, and health for a hosted app's managed database."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": {
                    "type": "string",
                    "description": "The app ID (UUID)"
                }
            },
            "required": ["app_id"]
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "hosting"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        let path = format!("/api/hosting/apps/{}/database", input.app_id);
        let result = hosting_request(&self.config, "GET", &path, None).await?;
        Ok(DatabaseStatusOutput { database: result })
    }
}

// =============================================================================
// hosting_analyze_repo — Analyze a repo for AI-driven deployment planning
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct AnalyzeRepoInput {
    /// GitHub repository URL (e.g., "https://github.com/user/repo")
    pub git_repo: String,
    /// Git token for private repos. Optional for public repos.
    #[serde(default)]
    pub git_token: Option<String>,
    /// Branch to analyze (default: main)
    #[serde(default)]
    pub git_branch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AnalyzeRepoOutput {
    /// Full analysis result from the platform service
    pub analysis: serde_json::Value,
}

/// Analyze a GitHub repository to determine framework, dependencies, required
/// env vars, database needs, and suggest deployment configuration. Use this
/// BEFORE deploying to understand what the app needs.
pub struct HostingAnalyzeRepoTool {
    config: Arc<HostingToolConfig>,
}

impl HostingAnalyzeRepoTool {
    pub fn new(config: Arc<HostingToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for HostingAnalyzeRepoTool {
    type Input = AnalyzeRepoInput;
    type Output = AnalyzeRepoOutput;

    fn name(&self) -> &str {
        "hosting_analyze_repo"
    }

    fn description(&self) -> &str {
        "Analyze a GitHub repository to plan deployment. Clones the repo, detects the framework \
         (Next.js, React, Python, Go, Rust, etc.), identifies required environment variables, \
         determines if a database is needed, and suggests build/start commands. \
         Use this tool FIRST before deploying an app — it tells you everything needed to \
         call hosting_deploy_app with the correct parameters. \
         For private repos, provide a git_token (GitHub Personal Access Token)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "git_repo": {
                    "type": "string",
                    "description": "GitHub repository URL (e.g., https://github.com/user/repo)"
                },
                "git_token": {
                    "type": "string",
                    "description": "GitHub Personal Access Token for private repos. Optional for public repos."
                },
                "git_branch": {
                    "type": "string",
                    "description": "Branch to analyze (default: main)"
                }
            },
            "required": ["git_repo"]
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "hosting"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let mut body = serde_json::json!({
            "git_repo": input.git_repo,
        });
        if let Some(token) = &input.git_token {
            body["git_token"] = serde_json::json!(token);
        }
        if let Some(branch) = &input.git_branch {
            body["git_branch"] = serde_json::json!(branch);
        }

        let result = hosting_request(
            &self.config,
            "POST",
            "/api/hosting/analyze-repo",
            Some(body),
        )
        .await?;

        Ok(AnalyzeRepoOutput { analysis: result })
    }
}

// =============================================================================
// Registration
// =============================================================================

/// Register all hosting tools into a ToolRegistry
pub fn register_hosting_tools(registry: &mut super::ToolRegistry, config: Arc<HostingToolConfig>) {
    registry.register_tool(HostingAnalyzeRepoTool::new(config.clone()));
    registry.register_tool(HostingDeployAppTool::new(config.clone()));
    registry.register_tool(HostingListAppsTool::new(config.clone()));
    registry.register_tool(HostingAppStatusTool::new(config.clone()));
    registry.register_tool(HostingAppLogsTool::new(config.clone()));
    registry.register_tool(HostingStopAppTool::new(config.clone()));
    registry.register_tool(HostingDeleteAppTool::new(config.clone()));
    registry.register_tool(HostingCreateDatabaseTool::new(config.clone()));
    registry.register_tool(HostingDatabaseStatusTool::new(config));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hosting_tool_config() {
        let config = HostingToolConfig::new("http://localhost:8080", "jwt-token");
        assert_eq!(config.base_url, "http://localhost:8080");
        assert_eq!(config.get_token(), "jwt-token");
    }

    #[test]
    fn test_token_provider_takes_priority() {
        let provider: TokenProvider = Arc::new(|| "dynamic-token".to_string());
        let config = HostingToolConfig::with_token_provider("http://localhost:8080", provider);
        assert_eq!(config.get_token(), "dynamic-token");
    }

    #[test]
    fn test_static_token_fallback() {
        let config = HostingToolConfig::new("http://localhost:8080", "static-token");
        // No provider → falls back to static token
        assert_eq!(config.get_token(), "static-token");
    }

    #[test]
    fn test_deploy_tool_metadata() {
        let config = Arc::new(HostingToolConfig::new("http://localhost:8080", "token"));
        let tool = HostingDeployAppTool::new(config);
        assert_eq!(tool.name(), "hosting_deploy_app");
        assert!(tool.requires_permission());
        assert!(tool.is_mutating());
        assert_eq!(tool.namespace(), "hosting");
    }

    #[test]
    fn test_list_apps_tool_metadata() {
        let config = Arc::new(HostingToolConfig::new("http://localhost:8080", "token"));
        let tool = HostingListAppsTool::new(config);
        assert_eq!(tool.name(), "hosting_list_apps");
        assert!(!tool.requires_permission());
        assert_eq!(tool.namespace(), "hosting");
    }

    #[test]
    fn test_register_hosting_tools() {
        let config = Arc::new(HostingToolConfig::new("http://localhost:8080", "token"));
        let mut registry = super::super::ToolRegistry::new();
        register_hosting_tools(&mut registry, config);
        // 9 tools registered
        let tools = registry.get_tool_metadata();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"hosting_analyze_repo"));
        assert!(names.contains(&"hosting_deploy_app"));
        assert!(names.contains(&"hosting_list_apps"));
        assert!(names.contains(&"hosting_app_status"));
        assert!(names.contains(&"hosting_app_logs"));
        assert!(names.contains(&"hosting_stop_app"));
        assert!(names.contains(&"hosting_delete_app"));
        assert!(names.contains(&"hosting_create_database"));
        assert!(names.contains(&"hosting_database_status"));
    }

    #[test]
    fn test_analyze_repo_tool_metadata() {
        let config = Arc::new(HostingToolConfig::new("http://localhost:8080", "token"));
        let tool = HostingAnalyzeRepoTool::new(config);
        assert_eq!(tool.name(), "hosting_analyze_repo");
        assert!(!tool.requires_permission()); // Read-only analysis
        assert_eq!(tool.namespace(), "hosting");
    }
}
