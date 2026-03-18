//! Database Tools — Per-user database operations via Platform Service API
//!
//! Provides agent tools for schema management, data manipulation, SQL execution,
//! migration tracking, and GitOps schema deployment. All operations go through
//! HTTP calls to the Platform Service database API, following the Engine/Platform
//! separation principle.

pub mod git_deploy;
pub mod migration;
pub mod tools;
pub mod types;

pub use types::*;

pub use git_deploy::{DatabaseGitDeployTool, GitDeployInput, GitDeployOutput};
pub use migration::{
    DatabaseMigrationApplyTool, DatabaseMigrationListTool, DatabaseMigrationRollbackTool,
    MigrationApplyInput, MigrationApplyOutput, MigrationListInput, MigrationListOutput,
    MigrationRecord, MigrationRollbackInput, MigrationRollbackOutput,
};
pub use tools::{
    DatabaseCreateTableTool, DatabaseDropTableTool, DatabaseExecuteSqlTool,
    DatabaseExplainSqlTool, DatabaseListTablesTool, DatabaseQueryRowsTool,
    DatabaseSchemaContextTool,
};

use super::ToolError;
use std::sync::Arc;

/// Dynamic token provider for service-to-service authentication.
/// Called on each request to get a fresh token (e.g., RS256 service JWT).
pub type TokenProvider = Arc<dyn Fn() -> String + Send + Sync>;

/// Configuration for database tools (Platform Service database API).
#[derive(Clone)]
pub struct DatabaseToolConfig {
    /// Base URL for the platform-service (e.g., "http://localhost:8080")
    pub base_url: String,
    /// Static fallback token (used when no token_provider is set)
    auth_token: String,
    /// Dynamic token generator (e.g., RS256 service token via KeyPair).
    /// Takes priority over static auth_token when present.
    token_provider: Option<TokenProvider>,
}

impl std::fmt::Debug for DatabaseToolConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseToolConfig")
            .field("base_url", &self.base_url)
            .field("auth_token", &"<redacted>")
            .field("has_token_provider", &self.token_provider.is_some())
            .finish()
    }
}

impl DatabaseToolConfig {
    /// Create config with a static auth token (legacy/fallback).
    pub fn new(base_url: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            auth_token: auth_token.into(),
            token_provider: None,
        }
    }

    /// Create config with a dynamic token provider (preferred).
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

/// Validate that an ID from LLM input is safe for URL path interpolation.
pub(crate) fn validate_path_id(id: &str, field_name: &str) -> Result<(), ToolError> {
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

/// Validate SQL input is non-empty and within a reasonable length.
pub(crate) fn validate_sql(sql: &str) -> Result<(), ToolError> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidInput(
            "SQL query cannot be empty".to_string(),
        ));
    }
    if trimmed.len() > 100_000 {
        return Err(ToolError::InvalidInput(
            "SQL query exceeds maximum length (100KB)".to_string(),
        ));
    }
    Ok(())
}

/// HTTP client helper for database API calls.
pub(crate) async fn database_request(
    config: &DatabaseToolConfig,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, ToolError> {
    let url = format!("{}{}", config.base_url, path);

    static HTTP_CLIENT: std::sync::LazyLock<reqwest::Client> =
        std::sync::LazyLock::new(reqwest::Client::new);
    let client = &*HTTP_CLIENT;

    let mut request = match method {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        "PATCH" => client.patch(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        _ => {
            return Err(ToolError::InvalidInput(format!(
                "Unsupported HTTP method: {}",
                method
            )));
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
        .map_err(|e| ToolError::ExecutionError(format!("Database API request failed: {}", e)))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| ToolError::ExecutionError(format!("Failed to read response: {}", e)))?;

    if !status.is_success() {
        return Err(ToolError::ExecutionError(format!(
            "Database API returned HTTP {}: {}",
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

/// SQL to create the migrations tracking table.
pub(crate) const CREATE_MIGRATIONS_TABLE_SQL: &str = "\
CREATE TABLE IF NOT EXISTS _canal_migrations (\
    id BIGSERIAL PRIMARY KEY,\
    name TEXT NOT NULL UNIQUE,\
    checksum TEXT NOT NULL,\
    executed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),\
    rolled_back_at TIMESTAMPTZ,\
    source TEXT NOT NULL DEFAULT 'agent'\
)";

/// Execute a SQL statement against an app's database via the hosting API.
pub(crate) async fn execute_sql(
    config: &DatabaseToolConfig,
    app_id: &str,
    sql: &str,
) -> Result<serde_json::Value, ToolError> {
    let path = format!("/api/hosting/apps/{}/db/sql/execute", app_id);
    let body = serde_json::json!({ "sql": sql });
    database_request(config, "POST", &path, Some(body)).await
}

/// Ensure the _canal_migrations table exists, creating it if needed.
pub(crate) async fn ensure_migrations_table(
    config: &DatabaseToolConfig,
    app_id: &str,
) -> Result<(), ToolError> {
    execute_sql(config, app_id, CREATE_MIGRATIONS_TABLE_SQL).await?;
    Ok(())
}

/// Register all database tools into a ToolRegistry.
pub fn register_database_tools(
    registry: &mut super::ToolRegistry,
    config: Arc<DatabaseToolConfig>,
) {
    // Core tools (7)
    registry.register_tool(DatabaseListTablesTool::new(config.clone()));
    registry.register_tool(DatabaseQueryRowsTool::new(config.clone()));
    registry.register_tool(DatabaseExecuteSqlTool::new(config.clone()));
    registry.register_tool(DatabaseCreateTableTool::new(config.clone()));
    registry.register_tool(DatabaseDropTableTool::new(config.clone()));
    registry.register_tool(DatabaseSchemaContextTool::new(config.clone()));
    registry.register_tool(DatabaseExplainSqlTool::new(config.clone()));
    // Migration tools (3)
    registry.register_tool(DatabaseMigrationListTool::new(config.clone()));
    registry.register_tool(DatabaseMigrationApplyTool::new(config.clone()));
    registry.register_tool(DatabaseMigrationRollbackTool::new(config.clone()));
    // GitOps tool (1)
    registry.register_tool(DatabaseGitDeployTool::new(config));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_static_token() {
        let config = DatabaseToolConfig::new("http://localhost:8080", "test-token");
        assert_eq!(config.base_url, "http://localhost:8080");
        assert_eq!(config.get_token(), "test-token");
    }

    #[test]
    fn test_config_dynamic_token() {
        let provider: TokenProvider = Arc::new(|| "dynamic-token".to_string());
        let config = DatabaseToolConfig::with_token_provider("http://localhost:8080", provider);
        assert_eq!(config.get_token(), "dynamic-token");
    }

    #[test]
    fn test_config_debug_redacts_token() {
        let config = DatabaseToolConfig::new("http://localhost:8080", "secret-123");
        let debug = format!("{:?}", config);
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret-123"));
    }

    #[test]
    fn test_validate_path_id() {
        assert!(validate_path_id("abc-123", "id").is_ok());
        assert!(validate_path_id("550e8400-e29b-41d4-a716-446655440000", "id").is_ok());
        assert!(validate_path_id("", "id").is_err());
        assert!(validate_path_id("../etc", "id").is_err());
        assert!(validate_path_id("a/b", "id").is_err());
        assert!(validate_path_id("a b", "id").is_err());
    }

    #[test]
    fn test_validate_sql() {
        assert!(validate_sql("SELECT 1").is_ok());
        assert!(validate_sql("").is_err());
        assert!(validate_sql("   ").is_err());
        let long = "x".repeat(100_001);
        assert!(validate_sql(&long).is_err());
    }

    #[test]
    fn test_register_database_tools() {
        let config = Arc::new(DatabaseToolConfig::new("http://localhost:8080", "token"));
        let mut registry = super::super::ToolRegistry::new();
        register_database_tools(&mut registry, config);
        let tools = registry.get_tool_metadata();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"database_list_tables"));
        assert!(names.contains(&"database_execute_sql"));
        assert!(names.contains(&"database_migration_list"));
        assert!(names.contains(&"database_git_deploy"));
    }
}
