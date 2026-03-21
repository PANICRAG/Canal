//! Unified Tool System
//!
//! Single registry + single execution path for all tool sources:
//! - Agent built-in tools (Read, Write, Edit, Bash, etc.)
//! - MCP builtin tools (filesystem, executor, browser, mac, automation)
//! - External MCP server tools (from mcp-servers.yaml)
//!
//! Replaces the dual registry / triple execution path architecture.

pub mod executor;
pub mod registry;
pub mod resolver;
#[cfg(test)]
mod tests;
pub mod types;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::agent::types::{
    PermissionBehavior, PermissionContext, PermissionMode, PermissionResult, PermissionRule,
};
use crate::error::{Error, Result};
use crate::mcp::builtin::BuiltinToolExecutor;
use crate::mcp::connection::McpConnection;
use crate::mcp::gateway::{McpServerConfig, McpTransport, PermissionRequest};
use crate::mcp::protocol::{McpToolDef, ToolCallResult};

pub use executor::UnifiedToolExecutor;
pub use registry::UnifiedToolRegistry;
pub use resolver::ToolResolver;
pub use types::*;

/// Unified Tool System facade
///
/// Provides a single API for tool registration, resolution, and execution
/// across all tool sources (agent, MCP builtin, MCP external).
pub struct ToolSystem {
    /// Unified registry of all tools
    pub registry: Arc<RwLock<UnifiedToolRegistry>>,
    /// Unified executor
    pub executor: Arc<UnifiedToolExecutor>,
    /// MCP server configs (shared with McpGateway)
    configs: Arc<RwLock<HashMap<String, McpServerConfig>>>,
    /// MCP connections (shared with McpGateway)
    connections: Arc<RwLock<HashMap<String, McpConnection>>>,
    /// Permission context for tool execution
    permission_ctx: Arc<RwLock<PermissionContext>>,
}

impl ToolSystem {
    /// Create a new ToolSystem with its own configs and connections
    pub fn new() -> Self {
        let configs = Arc::new(RwLock::new(HashMap::new()));
        let connections = Arc::new(RwLock::new(HashMap::new()));

        // Default permission context: bypass for backward compatibility
        let mut default_ctx = PermissionContext::default();
        default_ctx.mode = PermissionMode::BypassPermissions;

        Self {
            registry: Arc::new(RwLock::new(UnifiedToolRegistry::new())),
            executor: Arc::new(UnifiedToolExecutor::new(
                connections.clone(),
                configs.clone(),
            )),
            configs,
            connections,
            permission_ctx: Arc::new(RwLock::new(default_ctx)),
        }
    }

    /// Create a ToolSystem with shared connections and configs
    ///
    /// This allows ToolSystem and McpGateway to share the same connection pool.
    pub fn new_with_shared(
        configs: Arc<RwLock<HashMap<String, McpServerConfig>>>,
        connections: Arc<RwLock<HashMap<String, McpConnection>>>,
    ) -> Self {
        let mut default_ctx = PermissionContext::default();
        default_ctx.mode = PermissionMode::BypassPermissions;

        Self {
            registry: Arc::new(RwLock::new(UnifiedToolRegistry::new())),
            executor: Arc::new(UnifiedToolExecutor::new(
                connections.clone(),
                configs.clone(),
            )),
            configs,
            connections,
            permission_ctx: Arc::new(RwLock::new(default_ctx)),
        }
    }

    /// Get a reference to the shared connections Arc
    pub fn connections_ref(&self) -> &Arc<RwLock<HashMap<String, McpConnection>>> {
        &self.connections
    }

    /// Get a reference to the shared configs Arc
    pub fn configs_ref(&self) -> &Arc<RwLock<HashMap<String, McpServerConfig>>> {
        &self.configs
    }

    // ========================================================================
    // Permission Management
    // ========================================================================

    /// Set the permission mode
    pub async fn set_permission_mode(&self, mode: PermissionMode) {
        let mut ctx = self.permission_ctx.write().await;
        ctx.mode = mode;
    }

    /// Get the current permission mode
    pub async fn get_permission_mode(&self) -> PermissionMode {
        let ctx = self.permission_ctx.read().await;
        ctx.mode
    }

    /// Add a permission rule
    pub async fn add_permission_rule(&self, rule: PermissionRule, behavior: PermissionBehavior) {
        let mut ctx = self.permission_ctx.write().await;
        ctx.rules.push((rule, behavior));
    }

    /// Clear all permission rules
    pub async fn clear_permission_rules(&self) {
        let mut ctx = self.permission_ctx.write().await;
        ctx.rules.clear();
    }

    /// Set allowed directories for the permission context
    pub async fn set_allowed_directories(&self, directories: Vec<String>) {
        let mut ctx = self.permission_ctx.write().await;
        ctx.allowed_directories = directories;
    }

    /// Set the current working directory for permission checks
    pub async fn set_cwd(&self, cwd: Option<String>) {
        let mut ctx = self.permission_ctx.write().await;
        ctx.cwd = cwd;
    }

    /// Check permission for a tool call
    async fn check_permission(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> PermissionResult {
        let ctx = self.permission_ctx.read().await;
        ctx.check_tool(tool_name, arguments)
    }

    // ========================================================================
    // Registration
    // ========================================================================

    /// Register the builtin tool executor and all 27 MCP builtin tools
    pub async fn register_builtin_backend(&self, executor: BuiltinToolExecutor) {
        // Set the executor
        self.executor.set_builtin_executor(executor).await;

        // Register all 27 builtin tool definitions in the registry
        let mut registry = self.registry.write().await;
        Self::register_all_builtin_tools(&mut registry);
    }

    /// Register the screen controller on the builtin executor
    pub async fn set_builtin_screen_controller(
        &self,
        controller: Arc<dyn canal_cv::ScreenController>,
    ) {
        let builtin_ref = self.executor.builtin_executor_ref();
        let mut builtin = builtin_ref.write().await;
        if let Some(ref mut executor) = *builtin {
            executor.set_screen_controller(controller);
            tracing::info!("Screen controller registered with ToolSystem builtin executor");
        }
    }

    /// Register the CDP screen controller on the builtin executor
    pub async fn set_builtin_cdp_controller(&self, cdp: Arc<crate::screen::CdpScreenController>) {
        let builtin_ref = self.executor.builtin_executor_ref();
        let mut builtin = builtin_ref.write().await;
        if let Some(ref mut executor) = *builtin {
            executor.set_cdp_controller(cdp);
            tracing::info!("CDP controller registered with ToolSystem builtin executor");
        }
    }

    /// Register the automation orchestrator on the builtin executor
    pub async fn set_builtin_automation(
        &self,
        automation: Arc<crate::agent::automation::BrowserAutomationOrchestrator>,
    ) {
        let builtin_ref = self.executor.builtin_executor_ref();
        let mut builtin = builtin_ref.write().await;
        if let Some(ref mut executor) = *builtin {
            executor.set_automation(automation);
            tracing::info!("Automation orchestrator registered with ToolSystem builtin executor");
        }
    }

    /// Register external tools discovered from an MCP server connection
    pub async fn register_external_tools(
        &self,
        namespace: &str,
        server_name: &str,
        transport: &McpTransport,
        tools: &[McpToolDef],
    ) {
        let mut registry = self.registry.write().await;

        let (transport_type, location) = match transport {
            McpTransport::Stdio { command, .. } => {
                ("stdio".to_string(), format!("local:{}", command))
            }
            McpTransport::Http { url } => ("http".to_string(), url.clone()),
        };

        for mcp_tool in tools {
            registry.register(ToolEntry {
                id: ToolId::new(namespace, &mcp_tool.name),
                description: mcp_tool.description.clone().unwrap_or_default(),
                input_schema: mcp_tool.input_schema.clone(),
                source: ToolSource::McpExternal {
                    server_name: server_name.to_string(),
                },
                meta: ToolMeta {
                    transport_type: transport_type.clone(),
                    location: location.clone(),
                    server_name: server_name.to_string(),
                },
            });
        }
    }

    /// Register an MCP server config (for tool_system awareness)
    pub async fn register_mcp_server(&self, config: McpServerConfig) {
        let mut configs = self.configs.write().await;
        configs.insert(config.name.clone(), config);
    }

    // ========================================================================
    // Execution
    // ========================================================================

    /// Execute a tool by namespace and name (no permission check)
    pub async fn execute(
        &self,
        namespace: &str,
        name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let registry = self.registry.read().await;
        let id = ToolId::new(namespace, name);
        let entry = registry
            .get(&id)
            .ok_or_else(|| Error::NotFound(format!("Tool not found: {}.{}", namespace, name)))?;

        let result = self.executor.execute(entry, input).await?;

        // Convert ToolCallResult to JSON Value
        let output = if result.is_error {
            serde_json::json!({
                "success": false,
                "error": result.text_content().unwrap_or_else(|| "Unknown error".to_string())
            })
        } else {
            serde_json::json!({
                "success": true,
                "content": result.content
            })
        };

        Ok(output)
    }

    /// Execute a tool call from LLM format (namespace_toolname or bare agent name).
    ///
    /// Includes permission checking. Returns `Err(PermissionDenied)` if approval needed.
    pub async fn execute_llm_tool_call(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallResult> {
        // Check permission first
        let permission_result = self.check_permission(tool_name, &arguments).await;

        // Get the (potentially updated) arguments
        let arguments = match &permission_result {
            PermissionResult::Allow {
                updated_input: Some(input),
                ..
            } => input.clone(),
            _ => arguments,
        };

        match permission_result {
            PermissionResult::Allow { .. } => {
                // Permission granted, continue with execution
            }
            PermissionResult::Deny { message, .. } => {
                return Err(Error::PermissionDenied(message));
            }
            PermissionResult::Ask { question, .. } => {
                return Err(Error::PermissionDenied(format!(
                    "PERMISSION_REQUIRED:{}:{}:{}",
                    tool_name,
                    question,
                    serde_json::to_string(&arguments).unwrap_or_default()
                )));
            }
        }

        // Resolve and execute
        let registry = self.registry.read().await;
        if let Some(entry) = registry.get_by_llm_name(tool_name) {
            return self.executor.execute(entry, arguments).await;
        }
        drop(registry);

        // If not found in unified registry, try parsing as namespace_name for legacy compat
        let parts: Vec<&str> = tool_name.splitn(2, '_').collect();
        if parts.len() == 2 {
            Ok(ToolCallResult::error(format!(
                "Tool not found: '{}' (namespace='{}', name='{}')",
                tool_name, parts[0], parts[1]
            )))
        } else {
            Ok(ToolCallResult::error(format!(
                "Tool not found: '{}'",
                tool_name
            )))
        }
    }

    /// Execute a tool call with pre-approved permission (bypasses permission checks)
    pub async fn execute_llm_tool_call_approved(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallResult> {
        let registry = self.registry.read().await;
        if let Some(entry) = registry.get_by_llm_name(tool_name) {
            return self.executor.execute(entry, arguments).await;
        }
        drop(registry);

        Ok(ToolCallResult::error(format!(
            "Tool not found: '{}'",
            tool_name
        )))
    }

    /// Parse a permission required error message
    pub fn parse_permission_request(error: &Error) -> Option<PermissionRequest> {
        if let Error::PermissionDenied(msg) = error {
            if msg.starts_with("PERMISSION_REQUIRED:") {
                let parts: Vec<&str> = msg.splitn(4, ':').collect();
                if parts.len() >= 4 {
                    let tool_name = parts[1].to_string();
                    let question = parts[2].to_string();
                    let arguments = serde_json::from_str(parts[3]).unwrap_or_default();
                    return Some(PermissionRequest {
                        tool_name,
                        question,
                        arguments,
                    });
                }
            }
        }
        None
    }

    // ========================================================================
    // Query
    // ========================================================================

    /// List all registered tools
    pub async fn list_tools(&self) -> Vec<ToolEntry> {
        let registry = self.registry.read().await;
        registry.list().into_iter().cloned().collect()
    }

    /// List tools filtered by enabled namespaces
    pub async fn list_tools_filtered(&self, enabled_namespaces: &[String]) -> Vec<ToolEntry> {
        let registry = self.registry.read().await;
        registry
            .list_filtered(enabled_namespaces)
            .into_iter()
            .cloned()
            .collect()
    }

    /// List tools by namespace
    pub async fn list_by_namespace(&self, namespace: &str) -> Vec<ToolEntry> {
        let registry = self.registry.read().await;
        registry
            .list_by_namespace(namespace)
            .into_iter()
            .cloned()
            .collect()
    }

    /// Get tool metadata for a specific tool (by LLM name)
    pub async fn get_tool_meta(&self, llm_name: &str) -> Option<ToolMeta> {
        let registry = self.registry.read().await;
        registry.get_by_llm_name(llm_name).map(|e| e.meta.clone())
    }

    /// Get all tool metadata as a map of llm_name -> ToolMeta
    pub async fn get_all_tool_meta(&self) -> HashMap<String, ToolMeta> {
        let registry = self.registry.read().await;
        registry
            .list()
            .into_iter()
            .map(|e| (e.id.llm_name(), e.meta.clone()))
            .collect()
    }

    /// Get tools formatted for LLM consumption
    pub async fn tools_for_llm(&self) -> Vec<serde_json::Value> {
        let registry = self.registry.read().await;
        let resolver = ToolResolver::new(&registry);
        resolver.schemas_all()
    }

    /// Get filtered tools for LLM consumption
    pub async fn schemas_for_llm(&self, filter: &ToolFilter) -> Vec<serde_json::Value> {
        let registry = self.registry.read().await;
        let resolver = ToolResolver::new(&registry);
        resolver.schemas_for_llm(filter)
    }

    /// Get tools for a specific namespace
    pub async fn get_tools_for_namespace(&self, namespace: &str) -> Vec<ToolEntry> {
        self.list_by_namespace(namespace).await
    }

    /// Get tool count
    pub async fn tool_count(&self) -> usize {
        let registry = self.registry.read().await;
        registry.count()
    }

    // ========================================================================
    // MCP Server Management (delegated to McpGateway for connection lifecycle)
    // ========================================================================

    /// List servers with their info
    pub async fn list_servers(&self) -> Vec<(String, crate::mcp::gateway::McpServerInfo)> {
        let configs = self.configs.read().await;
        let connections = self.connections.read().await;
        let registry = self.registry.read().await;

        configs
            .iter()
            .map(|(name, config)| {
                let connected = connections.contains_key(name);
                let tool_count = registry.list_by_namespace(&config.namespace).len();

                let (transport_type, location) = match &config.transport {
                    McpTransport::Stdio { command, .. } => {
                        ("stdio".to_string(), format!("local:{}", command))
                    }
                    McpTransport::Http { url } => ("http".to_string(), url.clone()),
                };

                (
                    config.namespace.clone(),
                    crate::mcp::gateway::McpServerInfo {
                        tool_count,
                        connected,
                        transport_type,
                        location,
                        server_name: config.name.clone(),
                        description: String::new(),
                    },
                )
            })
            .collect()
    }

    /// Health check - check which servers are connected
    pub async fn health_check_map(&self) -> HashMap<String, bool> {
        let connections = self.connections.read().await;
        let configs = self.configs.read().await;

        let mut status = HashMap::new();
        for name in configs.keys() {
            status.insert(name.clone(), connections.contains_key(name));
        }
        status
    }

    /// Health check for a specific namespace
    pub async fn health_check(&self, namespace: &str) -> Result<bool> {
        let configs = self.configs.read().await;

        let server_name = configs
            .iter()
            .find(|(_, c)| c.namespace == namespace)
            .map(|(n, _)| n.clone());
        drop(configs);

        let server_name = server_name.ok_or_else(|| {
            Error::NotFound(format!("No server found for namespace: {}", namespace))
        })?;

        let connections = self.connections.read().await;
        Ok(connections.contains_key(&server_name))
    }

    // ========================================================================
    // Private helpers
    // ========================================================================

    /// Register all 27 MCP builtin tool definitions
    pub fn register_all_builtin_tools(registry: &mut UnifiedToolRegistry) {
        let builtin_meta = ToolMeta {
            transport_type: "builtin".to_string(),
            location: "local".to_string(),
            server_name: "builtin".to_string(),
        };

        // Helper to register a builtin tool
        let mut reg = |ns: &str, name: &str, desc: &str, schema: serde_json::Value| {
            registry.register(ToolEntry {
                id: ToolId::new(ns, name),
                description: desc.to_string(),
                input_schema: schema,
                source: ToolSource::McpBuiltin,
                meta: builtin_meta.clone(),
            });
        };

        // Filesystem tools (4)
        reg(
            "filesystem",
            "read_file",
            "Read contents of a file from the local filesystem",
            serde_json::json!({
                "type": "object", "required": ["path"],
                "properties": { "path": { "type": "string", "description": "Path to the file to read" } }
            }),
        );
        reg(
            "filesystem",
            "write_file",
            "Write content to a file on the local filesystem",
            serde_json::json!({
                "type": "object", "required": ["path", "content"],
                "properties": {
                    "path": { "type": "string", "description": "Path to write to" },
                    "content": { "type": "string", "description": "Content to write" },
                    "create_dirs": { "type": "boolean", "description": "Create parent directories if they don't exist", "default": true },
                    "overwrite": { "type": "boolean", "description": "Overwrite existing file", "default": true }
                }
            }),
        );
        reg(
            "filesystem",
            "list_directory",
            "List contents of a directory",
            serde_json::json!({
                "type": "object", "required": ["path"],
                "properties": {
                    "path": { "type": "string", "description": "Directory path" },
                    "recursive": { "type": "boolean", "default": false },
                    "include_hidden": { "type": "boolean", "default": false }
                }
            }),
        );
        reg(
            "filesystem",
            "search",
            "Search for content in files using ripgrep",
            serde_json::json!({
                "type": "object", "required": ["path", "pattern"],
                "properties": {
                    "path": { "type": "string", "description": "Directory to search in" },
                    "pattern": { "type": "string", "description": "Search pattern (regex)" },
                    "file_pattern": { "type": "string", "description": "File glob pattern (e.g., '*.rs')" },
                    "max_results": { "type": "integer", "default": 100 }
                }
            }),
        );

        // Executor tools (3)
        reg(
            "executor",
            "bash",
            "Execute a bash command",
            serde_json::json!({
                "type": "object", "required": ["command"],
                "properties": {
                    "command": { "type": "string", "description": "Bash command to execute" },
                    "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds" },
                    "working_dir": { "type": "string", "description": "Working directory for command execution" }
                }
            }),
        );
        reg(
            "executor",
            "python",
            "Execute Python code",
            serde_json::json!({
                "type": "object", "required": ["code"],
                "properties": {
                    "code": { "type": "string", "description": "Python code to execute" },
                    "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds" }
                }
            }),
        );
        reg(
            "executor",
            "run_code",
            "Execute code in a supported language",
            serde_json::json!({
                "type": "object", "required": ["code", "language"],
                "properties": {
                    "code": { "type": "string", "description": "Code to execute" },
                    "language": { "type": "string", "enum": ["python", "bash", "javascript", "typescript", "go", "rust"], "description": "Programming language" },
                    "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds" }
                }
            }),
        );

        // Browser tools (8)
        reg(
            "browser",
            "navigate",
            "Navigate the browser to a URL.",
            serde_json::json!({
                "type": "object", "required": ["url"],
                "properties": { "url": { "type": "string", "description": "URL to navigate to" } }
            }),
        );
        reg(
            "browser",
            "snapshot",
            "Get the current page content as an accessibility tree.",
            serde_json::json!({
                "type": "object", "properties": {}
            }),
        );
        reg(
            "browser",
            "click",
            "Click on an element identified by a CSS selector.",
            serde_json::json!({
                "type": "object", "required": ["selector"],
                "properties": { "selector": { "type": "string", "description": "CSS selector of the element to click" } }
            }),
        );
        reg(
            "browser",
            "fill",
            "Fill a form field with text.",
            serde_json::json!({
                "type": "object", "required": ["selector", "text"],
                "properties": {
                    "selector": { "type": "string", "description": "CSS selector of the input element" },
                    "text": { "type": "string", "description": "Text to fill into the element" }
                }
            }),
        );
        reg(
            "browser",
            "screenshot",
            "Take a screenshot of the current page.",
            serde_json::json!({
                "type": "object", "properties": { "full_page": { "type": "boolean", "description": "Capture the full scrollable page", "default": false } }
            }),
        );
        reg(
            "browser",
            "scroll",
            "Scroll the page.",
            serde_json::json!({
                "type": "object", "properties": {
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"], "description": "Scroll direction", "default": "down" },
                    "amount": { "type": "integer", "description": "Scroll amount in pixels", "default": 500 }
                }
            }),
        );
        reg(
            "browser",
            "wait",
            "Wait for an element to appear or for navigation to complete.",
            serde_json::json!({
                "type": "object", "properties": {
                    "selector": { "type": "string", "description": "CSS selector to wait for (optional)" },
                    "timeout": { "type": "integer", "description": "Maximum wait time in milliseconds", "default": 30000 }
                }
            }),
        );
        reg(
            "browser",
            "evaluate",
            "Execute JavaScript in the browser context and return the result.",
            serde_json::json!({
                "type": "object", "required": ["script"],
                "properties": { "script": { "type": "string", "description": "JavaScript code to execute" } }
            }),
        );

        // Mac tools (9)
        reg(
            "mac",
            "osascript",
            "Execute AppleScript commands on macOS.",
            serde_json::json!({
                "type": "object", "required": ["script"],
                "properties": {
                    "script": { "type": "string", "description": "The AppleScript code to execute" },
                    "timeout": { "type": "integer", "description": "Timeout in seconds (default: 30)", "default": 30 }
                }
            }),
        );
        reg(
            "mac",
            "screenshot",
            "Capture a screenshot of the screen or a specific region.",
            serde_json::json!({
                "type": "object", "properties": {
                    "path": { "type": "string", "description": "Path to save the screenshot" },
                    "region": { "type": "object", "description": "Optional region to capture", "properties": {
                        "x": {"type": "integer"}, "y": {"type": "integer"}, "width": {"type": "integer"}, "height": {"type": "integer"}
                    }}
                }
            }),
        );
        reg(
            "mac",
            "app_control",
            "Control Mac applications - launch, quit, hide, show, or minimize apps.",
            serde_json::json!({
                "type": "object", "required": ["action", "app"],
                "properties": {
                    "action": { "type": "string", "enum": ["launch", "quit", "hide", "show", "minimize"], "description": "The action to perform" },
                    "app": { "type": "string", "description": "The application name" }
                }
            }),
        );
        reg(
            "mac",
            "open_url",
            "Open a URL in the default browser.",
            serde_json::json!({
                "type": "object", "required": ["url"],
                "properties": { "url": { "type": "string", "description": "The URL to open" } }
            }),
        );
        reg(
            "mac",
            "notify",
            "Show a macOS notification with title and message.",
            serde_json::json!({
                "type": "object", "required": ["message"],
                "properties": {
                    "title": { "type": "string", "description": "Notification title", "default": "Notification" },
                    "message": { "type": "string", "description": "Notification message content" }
                }
            }),
        );
        reg(
            "mac",
            "clipboard_read",
            "Read the current content from the system clipboard.",
            serde_json::json!({
                "type": "object", "properties": {}
            }),
        );
        reg(
            "mac",
            "clipboard_write",
            "Write content to the system clipboard.",
            serde_json::json!({
                "type": "object", "required": ["content"],
                "properties": { "content": { "type": "string", "description": "Content to copy to the clipboard" } }
            }),
        );
        reg(
            "mac",
            "get_frontmost_app",
            "Get the name of the currently active (frontmost) application.",
            serde_json::json!({
                "type": "object", "properties": {}
            }),
        );
        reg(
            "mac",
            "list_running_apps",
            "List all currently running applications.",
            serde_json::json!({
                "type": "object", "properties": {}
            }),
        );

        // Automation tools (3)
        reg(
            "automation",
            "analyze",
            "Analyze a browser automation task and get the optimal execution path.",
            serde_json::json!({
                "type": "object", "required": ["task"],
                "properties": {
                    "task": { "type": "string", "description": "Description of the automation task to analyze" },
                    "data_count": { "type": "integer", "description": "Number of data items to process" }
                }
            }),
        );
        reg(
            "automation",
            "execute",
            "Execute a browser automation task through the five-layer pipeline.",
            serde_json::json!({
                "type": "object", "required": ["task", "target_url"],
                "properties": {
                    "task": { "type": "string", "description": "Description of the automation task" },
                    "target_url": { "type": "string", "description": "URL of the target page" },
                    "data": { "type": "array", "items": { "type": "object" }, "description": "Array of data objects to process" },
                    "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds", "default": 300000 },
                    "force_explore": { "type": "boolean", "description": "Force re-exploration", "default": false }
                }
            }),
        );
        reg(
            "automation",
            "status",
            "Get the status of the automation orchestrator.",
            serde_json::json!({
                "type": "object", "properties": {}
            }),
        );
    }
}

impl Default for ToolSystem {
    fn default() -> Self {
        Self::new()
    }
}

// Tests moved to tool_system/tests.rs for comprehensive coverage
