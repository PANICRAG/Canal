//! DevTools Observation Tools — Monitor infrastructure and app health
//!
//! Provides agent tools for querying the Weir DevTools server to observe
//! container status, infrastructure health, and database metrics.

use super::{AgentTool, ToolContext, ToolError, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Configuration for devtools observation tools
#[derive(Debug, Clone)]
pub struct DevtoolsToolConfig {
    /// Base URL for the devtools server (e.g., "http://localhost:4200")
    pub base_url: String,
    /// API key for authentication
    pub api_key: String,
}

impl DevtoolsToolConfig {
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
        }
    }
}

/// HTTP client helper for devtools API calls
async fn devtools_request(
    config: &DevtoolsToolConfig,
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
        _ => {
            return Err(ToolError::InvalidInput(format!(
                "Unsupported method: {}",
                method
            )))
        }
    };

    request = request
        .header("Authorization", format!("Bearer {}", config.api_key))
        .header("Content-Type", "application/json");

    if let Some(body) = body {
        request = request.json(&body);
    }

    let response = request
        .send()
        .await
        .map_err(|e| ToolError::ExecutionError(format!("DevTools API request failed: {}", e)))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| ToolError::ExecutionError(format!("Failed to read response: {}", e)))?;

    if !status.is_success() {
        return Err(ToolError::ExecutionError(format!(
            "DevTools API returned HTTP {}: {}",
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
// devtools_containers — List Docker containers with resource usage
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ContainersInput {}

#[derive(Debug, Serialize)]
pub struct ContainersOutput {
    pub containers: serde_json::Value,
}

/// List all running Docker containers with CPU, memory, and network stats
pub struct DevtoolsContainersTool {
    config: Arc<DevtoolsToolConfig>,
}

impl DevtoolsContainersTool {
    pub fn new(config: Arc<DevtoolsToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DevtoolsContainersTool {
    type Input = ContainersInput;
    type Output = ContainersOutput;

    fn name(&self) -> &str {
        "devtools_containers"
    }

    fn description(&self) -> &str {
        "List all running Docker containers with resource usage (CPU%, memory MB, network I/O, uptime). \
         Shows all canal-managed containers including deployed apps."
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
        "devtools"
    }

    async fn execute(
        &self,
        _input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let result = devtools_request(&self.config, "GET", "/v1/metrics/containers", None).await?;
        Ok(ContainersOutput { containers: result })
    }
}

// =============================================================================
// devtools_health — Infrastructure health check
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct HealthInput {}

#[derive(Debug, Serialize)]
pub struct HealthOutput {
    pub health: serde_json::Value,
}

/// Check overall infrastructure health (Docker, Prometheus, scrape targets)
pub struct DevtoolsHealthTool {
    config: Arc<DevtoolsToolConfig>,
}

impl DevtoolsHealthTool {
    pub fn new(config: Arc<DevtoolsToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DevtoolsHealthTool {
    type Input = HealthInput;
    type Output = HealthOutput;

    fn name(&self) -> &str {
        "devtools_health"
    }

    fn description(&self) -> &str {
        "Check overall infrastructure health: Docker daemon, Prometheus, and scrape targets. \
         Returns healthy/degraded status per component."
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
        "devtools"
    }

    async fn execute(
        &self,
        _input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let result = devtools_request(&self.config, "GET", "/v1/metrics/health", None).await?;
        Ok(HealthOutput { health: result })
    }
}

// =============================================================================
// devtools_database_health — Database health score and stats
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct DatabaseHealthInput {}

#[derive(Debug, Serialize)]
pub struct DatabaseHealthOutput {
    pub database: serde_json::Value,
}

/// Get database health score (0-100) with cache hit ratio, connection usage, and more
pub struct DevtoolsDatabaseHealthTool {
    config: Arc<DevtoolsToolConfig>,
}

impl DevtoolsDatabaseHealthTool {
    pub fn new(config: Arc<DevtoolsToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DevtoolsDatabaseHealthTool {
    type Input = DatabaseHealthInput;
    type Output = DatabaseHealthOutput;

    fn name(&self) -> &str {
        "devtools_database_health"
    }

    fn description(&self) -> &str {
        "Get database health score (0-100, A-F grade) with detailed metrics: \
         cache hit ratio, connection usage, replication lag, dead tuples, slow queries."
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
        "devtools"
    }

    async fn execute(
        &self,
        _input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let result = devtools_request(&self.config, "GET", "/v1/database/health", None).await?;
        Ok(DatabaseHealthOutput { database: result })
    }
}

// =============================================================================
// devtools_logs — Query logs via Loki
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct LogsQueryInput {
    /// LogQL query (e.g., '{container_name="canal-myapp"}')
    pub query: String,
    /// Maximum number of log lines (default: 50)
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct LogsQueryOutput {
    pub logs: serde_json::Value,
}

/// Query application logs via LogQL (Loki)
pub struct DevtoolsLogsTool {
    config: Arc<DevtoolsToolConfig>,
}

impl DevtoolsLogsTool {
    pub fn new(config: Arc<DevtoolsToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DevtoolsLogsTool {
    type Input = LogsQueryInput;
    type Output = LogsQueryOutput;

    fn name(&self) -> &str {
        "devtools_logs"
    }

    fn description(&self) -> &str {
        "Query application logs using LogQL syntax. Use {container_name=\"canal-APPNAME\"} \
         to filter by app. Returns recent log entries sorted newest first."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "LogQL query (e.g., '{container_name=\"canal-myapp\"}')"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max log lines to return (default: 50)"
                }
            },
            "required": ["query"]
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "devtools"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let body = serde_json::json!({
            "query": input.query,
            "limit": input.limit.unwrap_or(50),
            "direction": "backward",
        });
        let result = devtools_request(&self.config, "POST", "/v1/logs/query", Some(body)).await?;
        Ok(LogsQueryOutput { logs: result })
    }
}

// =============================================================================
// Registration
// =============================================================================

/// Register all devtools observation tools into a ToolRegistry
pub fn register_devtools_tools(
    registry: &mut super::ToolRegistry,
    config: Arc<DevtoolsToolConfig>,
) {
    registry.register_tool(DevtoolsContainersTool::new(config.clone()));
    registry.register_tool(DevtoolsHealthTool::new(config.clone()));
    registry.register_tool(DevtoolsDatabaseHealthTool::new(config.clone()));
    registry.register_tool(DevtoolsLogsTool::new(config));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_devtools_tool_config() {
        let config = DevtoolsToolConfig::new("http://localhost:4200", "test-key");
        assert_eq!(config.base_url, "http://localhost:4200");
        assert_eq!(config.api_key, "test-key");
    }

    #[test]
    fn test_containers_tool_metadata() {
        let config = Arc::new(DevtoolsToolConfig::new("http://localhost:4200", "key"));
        let tool = DevtoolsContainersTool::new(config);
        assert_eq!(tool.name(), "devtools_containers");
        assert!(!tool.requires_permission());
        assert_eq!(tool.namespace(), "devtools");
    }

    #[test]
    fn test_register_devtools_tools() {
        let config = Arc::new(DevtoolsToolConfig::new("http://localhost:4200", "key"));
        let mut registry = super::super::ToolRegistry::new();
        register_devtools_tools(&mut registry, config);
        let tools = registry.get_tool_metadata();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"devtools_containers"));
        assert!(names.contains(&"devtools_health"));
        assert!(names.contains(&"devtools_database_health"));
        assert!(names.contains(&"devtools_logs"));
    }
}
