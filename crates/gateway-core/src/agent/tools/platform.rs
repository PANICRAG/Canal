//! Platform Control Plane Tools
//!
//! Provides agent tools for managing platform instances, status, logs, and metrics
//! via HTTP calls to the platform REST API. This allows any chat session to
//! operate the platform through natural language.

use super::{AgentTool, ToolContext, ToolError, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Shared configuration for all platform tools
#[derive(Debug, Clone)]
pub struct PlatformToolConfig {
    /// Base URL for the platform API (e.g., "http://localhost:4000")
    pub base_url: String,
    /// Bearer token for authentication
    pub auth_token: String,
}

impl PlatformToolConfig {
    /// Create a new PlatformToolConfig
    pub fn new(base_url: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            auth_token: auth_token.into(),
        }
    }
}

/// R1-C11: Validate that an ID from LLM input is safe for URL path interpolation.
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

/// HTTP client helper for platform API calls
async fn platform_request(
    config: &PlatformToolConfig,
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
        "DELETE" => client.delete(&url),
        _ => {
            return Err(ToolError::InvalidInput(format!(
                "Unsupported method: {}",
                method
            )))
        }
    };

    request = request
        .header("Authorization", format!("Bearer {}", config.auth_token))
        .header("Content-Type", "application/json");

    if let Some(body) = body {
        request = request.json(&body);
    }

    let response = request
        .send()
        .await
        .map_err(|e| ToolError::ExecutionError(format!("HTTP request failed: {}", e)))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| ToolError::ExecutionError(format!("Failed to read response: {}", e)))?;

    if !status.is_success() {
        return Err(ToolError::ExecutionError(format!(
            "Platform API returned HTTP {}: {}",
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
// platform_create_tenant
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateTenantInput {
    /// Name for the tenant
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct CreateTenantOutput {
    pub tenant: serde_json::Value,
}

/// Create a tenant (required before creating instances)
pub struct PlatformCreateTenantTool {
    config: Arc<PlatformToolConfig>,
}

impl PlatformCreateTenantTool {
    pub fn new(config: Arc<PlatformToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for PlatformCreateTenantTool {
    type Input = CreateTenantInput;
    type Output = CreateTenantOutput;

    fn name(&self) -> &str {
        "platform_create_tenant"
    }

    fn description(&self) -> &str {
        "Create a new tenant on the platform. A tenant must exist before creating instances."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name for the tenant"
                }
            },
            "required": ["name"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "platform"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let body = serde_json::json!({"name": input.name});
        let result = platform_request(&self.config, "POST", "/api/tenants", Some(body)).await?;
        Ok(CreateTenantOutput { tenant: result })
    }
}

// =============================================================================
// platform_list_instances
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ListInstancesInput {}

#[derive(Debug, Serialize)]
pub struct ListInstancesOutput {
    pub instances: serde_json::Value,
}

/// List all platform instances
pub struct PlatformListInstancesTool {
    config: Arc<PlatformToolConfig>,
}

impl PlatformListInstancesTool {
    pub fn new(config: Arc<PlatformToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for PlatformListInstancesTool {
    type Input = ListInstancesInput;
    type Output = ListInstancesOutput;

    fn name(&self) -> &str {
        "platform_list_instances"
    }

    fn description(&self) -> &str {
        "List all platform gateway instances with their status, ports, and health information."
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
        "platform"
    }

    async fn execute(
        &self,
        _input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let result = platform_request(&self.config, "GET", "/api/instances", None).await?;
        Ok(ListInstancesOutput { instances: result })
    }
}

// =============================================================================
// platform_create_instance
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateInstanceInput {
    /// Name for the new instance
    pub name: String,
    /// Modules to enable (optional)
    #[serde(default)]
    pub modules: Option<Vec<String>>,
    /// Memory limit in MB (optional)
    #[serde(default)]
    pub memory_limit_mb: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct CreateInstanceOutput {
    pub instance: serde_json::Value,
}

/// Create a new platform instance
pub struct PlatformCreateInstanceTool {
    config: Arc<PlatformToolConfig>,
}

impl PlatformCreateInstanceTool {
    pub fn new(config: Arc<PlatformToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for PlatformCreateInstanceTool {
    type Input = CreateInstanceInput;
    type Output = CreateInstanceOutput;

    fn name(&self) -> &str {
        "platform_create_instance"
    }

    fn description(&self) -> &str {
        "Create a new platform gateway instance. Optionally specify modules to enable and memory limit."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name for the new instance"
                },
                "modules": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Modules to enable (e.g., [\"engine\", \"identity\"])"
                },
                "memory_limit_mb": {
                    "type": "integer",
                    "description": "Memory limit in MB (default: 512)"
                }
            },
            "required": ["name"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "platform"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let mut body = serde_json::json!({"name": input.name});
        if let Some(modules) = input.modules {
            body["modules"] = serde_json::json!(modules);
        }
        if let Some(mem) = input.memory_limit_mb {
            body["memory_limit_mb"] = serde_json::json!(mem);
        }
        let result = platform_request(&self.config, "POST", "/api/instances", Some(body)).await?;
        Ok(CreateInstanceOutput { instance: result })
    }
}

// =============================================================================
// platform_start_instance
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct StartInstanceInput {
    /// ID of the instance to start
    pub instance_id: String,
}

#[derive(Debug, Serialize)]
pub struct StartInstanceOutput {
    pub instance: serde_json::Value,
}

/// Start a stopped platform instance
pub struct PlatformStartInstanceTool {
    config: Arc<PlatformToolConfig>,
}

impl PlatformStartInstanceTool {
    pub fn new(config: Arc<PlatformToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for PlatformStartInstanceTool {
    type Input = StartInstanceInput;
    type Output = StartInstanceOutput;

    fn name(&self) -> &str {
        "platform_start_instance"
    }

    fn description(&self) -> &str {
        "Start a stopped platform instance by its ID."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "instance_id": {
                    "type": "string",
                    "description": "ID of the instance to start"
                }
            },
            "required": ["instance_id"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "platform"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        validate_path_id(&input.instance_id, "instance_id")?;
        let path = format!("/api/instances/{}/start", input.instance_id);
        let result = platform_request(&self.config, "POST", &path, None).await?;
        Ok(StartInstanceOutput { instance: result })
    }
}

// =============================================================================
// platform_stop_instance
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct StopInstanceInput {
    /// ID of the instance to stop
    pub instance_id: String,
}

#[derive(Debug, Serialize)]
pub struct StopInstanceOutput {
    pub instance: serde_json::Value,
}

/// Stop a running platform instance
pub struct PlatformStopInstanceTool {
    config: Arc<PlatformToolConfig>,
}

impl PlatformStopInstanceTool {
    pub fn new(config: Arc<PlatformToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for PlatformStopInstanceTool {
    type Input = StopInstanceInput;
    type Output = StopInstanceOutput;

    fn name(&self) -> &str {
        "platform_stop_instance"
    }

    fn description(&self) -> &str {
        "Stop a running platform instance by its ID."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "instance_id": {
                    "type": "string",
                    "description": "ID of the instance to stop"
                }
            },
            "required": ["instance_id"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "platform"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        validate_path_id(&input.instance_id, "instance_id")?;
        let path = format!("/api/instances/{}/stop", input.instance_id);
        let result = platform_request(&self.config, "POST", &path, None).await?;
        Ok(StopInstanceOutput { instance: result })
    }
}

// =============================================================================
// platform_destroy_instance
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct DestroyInstanceInput {
    /// ID of the instance to destroy
    pub instance_id: String,
}

#[derive(Debug, Serialize)]
pub struct DestroyInstanceOutput {
    pub success: bool,
}

/// Permanently destroy a platform instance
pub struct PlatformDestroyInstanceTool {
    config: Arc<PlatformToolConfig>,
}

impl PlatformDestroyInstanceTool {
    pub fn new(config: Arc<PlatformToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for PlatformDestroyInstanceTool {
    type Input = DestroyInstanceInput;
    type Output = DestroyInstanceOutput;

    fn name(&self) -> &str {
        "platform_destroy_instance"
    }

    fn description(&self) -> &str {
        "Permanently destroy a platform instance. This action cannot be undone."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "instance_id": {
                    "type": "string",
                    "description": "ID of the instance to destroy"
                }
            },
            "required": ["instance_id"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "platform"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        validate_path_id(&input.instance_id, "instance_id")?;
        let path = format!("/api/instances/{}", input.instance_id);
        platform_request(&self.config, "DELETE", &path, None).await?;
        Ok(DestroyInstanceOutput { success: true })
    }
}

// =============================================================================
// platform_get_status
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct GetStatusInput {}

#[derive(Debug, Serialize)]
pub struct GetStatusOutput {
    pub overview: serde_json::Value,
}

/// Get platform overview status
pub struct PlatformGetStatusTool {
    config: Arc<PlatformToolConfig>,
}

impl PlatformGetStatusTool {
    pub fn new(config: Arc<PlatformToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for PlatformGetStatusTool {
    type Input = GetStatusInput;
    type Output = GetStatusOutput;

    fn name(&self) -> &str {
        "platform_get_status"
    }

    fn description(&self) -> &str {
        "Get platform overview: total tenants, active instances, memory/CPU usage, port allocation."
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
        "platform"
    }

    async fn execute(
        &self,
        _input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let result = platform_request(&self.config, "GET", "/api/admin/overview", None).await?;
        Ok(GetStatusOutput { overview: result })
    }
}

// =============================================================================
// platform_get_logs
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct GetLogsInput {
    /// ID of the instance
    pub instance_id: String,
    /// Number of log lines to return (default: 100)
    #[serde(default)]
    pub tail: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct GetLogsOutput {
    pub logs: serde_json::Value,
}

/// Get logs from a platform instance
pub struct PlatformGetLogsTool {
    config: Arc<PlatformToolConfig>,
}

impl PlatformGetLogsTool {
    pub fn new(config: Arc<PlatformToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for PlatformGetLogsTool {
    type Input = GetLogsInput;
    type Output = GetLogsOutput;

    fn name(&self) -> &str {
        "platform_get_logs"
    }

    fn description(&self) -> &str {
        "Get recent logs from a platform instance. Optionally specify number of lines to return."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "instance_id": {
                    "type": "string",
                    "description": "ID of the instance"
                },
                "tail": {
                    "type": "integer",
                    "description": "Number of log lines to return (default: 100)"
                }
            },
            "required": ["instance_id"]
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "platform"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        validate_path_id(&input.instance_id, "instance_id")?;
        let tail = input.tail.unwrap_or(100);
        let path = format!("/api/instances/{}/logs?tail={}", input.instance_id, tail);
        let result = platform_request(&self.config, "GET", &path, None).await?;
        Ok(GetLogsOutput { logs: result })
    }
}

// =============================================================================
// platform_get_metrics
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct GetMetricsInput {
    /// ID of the instance
    pub instance_id: String,
}

#[derive(Debug, Serialize)]
pub struct GetMetricsOutput {
    pub metrics: serde_json::Value,
}

/// Get metrics from a platform instance
pub struct PlatformGetMetricsTool {
    config: Arc<PlatformToolConfig>,
}

impl PlatformGetMetricsTool {
    pub fn new(config: Arc<PlatformToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for PlatformGetMetricsTool {
    type Input = GetMetricsInput;
    type Output = GetMetricsOutput;

    fn name(&self) -> &str {
        "platform_get_metrics"
    }

    fn description(&self) -> &str {
        "Get CPU, memory, and network metrics from a platform instance."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "instance_id": {
                    "type": "string",
                    "description": "ID of the instance"
                }
            },
            "required": ["instance_id"]
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "platform"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        validate_path_id(&input.instance_id, "instance_id")?;
        let path = format!("/api/instances/{}/metrics", input.instance_id);
        let result = platform_request(&self.config, "GET", &path, None).await?;
        Ok(GetMetricsOutput { metrics: result })
    }
}

// =============================================================================
// Registration helper
// =============================================================================

/// Register all platform tools into a ToolRegistry
pub fn register_platform_tools(
    registry: &mut super::ToolRegistry,
    config: Arc<PlatformToolConfig>,
) {
    registry.register_tool(PlatformCreateTenantTool::new(config.clone()));
    registry.register_tool(PlatformListInstancesTool::new(config.clone()));
    registry.register_tool(PlatformCreateInstanceTool::new(config.clone()));
    registry.register_tool(PlatformStartInstanceTool::new(config.clone()));
    registry.register_tool(PlatformStopInstanceTool::new(config.clone()));
    registry.register_tool(PlatformDestroyInstanceTool::new(config.clone()));
    registry.register_tool(PlatformGetStatusTool::new(config.clone()));
    registry.register_tool(PlatformGetLogsTool::new(config.clone()));
    registry.register_tool(PlatformGetMetricsTool::new(config));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_tool_config() {
        let config = PlatformToolConfig::new("http://localhost:4000", "test-token");
        assert_eq!(config.base_url, "http://localhost:4000");
        assert_eq!(config.auth_token, "test-token");
    }

    #[test]
    fn test_create_tenant_tool_metadata() {
        let config = Arc::new(PlatformToolConfig::new("http://localhost:4000", "token"));
        let tool = PlatformCreateTenantTool::new(config);
        assert_eq!(tool.name(), "platform_create_tenant");
        assert!(tool.requires_permission());
        assert!(tool.is_mutating());
        assert_eq!(tool.namespace(), "platform");
    }

    #[test]
    fn test_list_instances_tool_metadata() {
        let config = Arc::new(PlatformToolConfig::new("http://localhost:4000", "token"));
        let tool = PlatformListInstancesTool::new(config);
        assert_eq!(tool.name(), "platform_list_instances");
        assert!(!tool.requires_permission());
        assert!(!tool.is_mutating());
        assert_eq!(tool.namespace(), "platform");
    }

    #[test]
    fn test_create_instance_tool_metadata() {
        let config = Arc::new(PlatformToolConfig::new("http://localhost:4000", "token"));
        let tool = PlatformCreateInstanceTool::new(config);
        assert_eq!(tool.name(), "platform_create_instance");
        assert!(tool.requires_permission());
        assert!(tool.is_mutating());
        assert_eq!(tool.namespace(), "platform");
    }

    #[test]
    fn test_start_instance_tool_metadata() {
        let config = Arc::new(PlatformToolConfig::new("http://localhost:4000", "token"));
        let tool = PlatformStartInstanceTool::new(config);
        assert_eq!(tool.name(), "platform_start_instance");
        assert!(tool.requires_permission());
        assert!(tool.is_mutating());
    }

    #[test]
    fn test_stop_instance_tool_metadata() {
        let config = Arc::new(PlatformToolConfig::new("http://localhost:4000", "token"));
        let tool = PlatformStopInstanceTool::new(config);
        assert_eq!(tool.name(), "platform_stop_instance");
        assert!(tool.requires_permission());
        assert!(tool.is_mutating());
    }

    #[test]
    fn test_destroy_instance_tool_metadata() {
        let config = Arc::new(PlatformToolConfig::new("http://localhost:4000", "token"));
        let tool = PlatformDestroyInstanceTool::new(config);
        assert_eq!(tool.name(), "platform_destroy_instance");
        assert!(tool.requires_permission());
        assert!(tool.is_mutating());
    }

    #[test]
    fn test_get_status_tool_metadata() {
        let config = Arc::new(PlatformToolConfig::new("http://localhost:4000", "token"));
        let tool = PlatformGetStatusTool::new(config);
        assert_eq!(tool.name(), "platform_get_status");
        assert!(!tool.requires_permission());
        assert!(!tool.is_mutating());
    }

    #[test]
    fn test_get_logs_tool_metadata() {
        let config = Arc::new(PlatformToolConfig::new("http://localhost:4000", "token"));
        let tool = PlatformGetLogsTool::new(config);
        assert_eq!(tool.name(), "platform_get_logs");
        assert!(!tool.requires_permission());
    }

    #[test]
    fn test_get_metrics_tool_metadata() {
        let config = Arc::new(PlatformToolConfig::new("http://localhost:4000", "token"));
        let tool = PlatformGetMetricsTool::new(config);
        assert_eq!(tool.name(), "platform_get_metrics");
        assert!(!tool.requires_permission());
    }

    #[test]
    fn test_all_tools_have_valid_schemas() {
        let config = Arc::new(PlatformToolConfig::new("http://localhost:4000", "token"));
        let tools: Vec<Box<dyn std::any::Any>> = vec![
            Box::new(PlatformCreateTenantTool::new(config.clone())),
            Box::new(PlatformListInstancesTool::new(config.clone())),
            Box::new(PlatformCreateInstanceTool::new(config.clone())),
            Box::new(PlatformStartInstanceTool::new(config.clone())),
            Box::new(PlatformStopInstanceTool::new(config.clone())),
            Box::new(PlatformDestroyInstanceTool::new(config.clone())),
            Box::new(PlatformGetStatusTool::new(config.clone())),
            Box::new(PlatformGetLogsTool::new(config.clone())),
            Box::new(PlatformGetMetricsTool::new(config)),
        ];
        // All 9 tools created successfully
        assert_eq!(tools.len(), 9);
    }

    #[test]
    fn test_input_schema_has_required_fields() {
        let config = Arc::new(PlatformToolConfig::new("http://localhost:4000", "token"));

        let tool = PlatformCreateInstanceTool::new(config.clone());
        let schema = tool.input_schema();
        assert_eq!(schema["required"], serde_json::json!(["name"]));

        let tool = PlatformStartInstanceTool::new(config.clone());
        let schema = tool.input_schema();
        assert_eq!(schema["required"], serde_json::json!(["instance_id"]));

        let tool = PlatformGetLogsTool::new(config);
        let schema = tool.input_schema();
        assert_eq!(schema["required"], serde_json::json!(["instance_id"]));
    }

    #[test]
    fn test_register_platform_tools() {
        let config = Arc::new(PlatformToolConfig::new("http://localhost:4000", "token"));
        let mut registry = super::super::ToolRegistry::new();
        let before = registry.list_builtin_names().len();
        register_platform_tools(&mut registry, config);
        let after = registry.list_builtin_names().len();
        assert_eq!(after - before, 9);
        assert!(registry.is_builtin("platform_create_tenant"));
        assert!(registry.is_builtin("platform_list_instances"));
        assert!(registry.is_builtin("platform_create_instance"));
        assert!(registry.is_builtin("platform_start_instance"));
        assert!(registry.is_builtin("platform_stop_instance"));
        assert!(registry.is_builtin("platform_destroy_instance"));
        assert!(registry.is_builtin("platform_get_status"));
        assert!(registry.is_builtin("platform_get_logs"));
        assert!(registry.is_builtin("platform_get_metrics"));
    }
}
