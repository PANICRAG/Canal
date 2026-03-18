//! Tool Registry - Unified tool management for Agent SDK
//!
//! Provides a single interface for managing both built-in tools and MCP tools.

#[cfg(unix)]
use super::BrowserTool;
use super::{
    AgentTool, BashTool, ClaudeCodeTool, DynamicTool, EditTool, GlobTool, GrepTool,
    LocalComputerTool, ReadTool, ToolContext, ToolError, ToolMetadata, ToolWrapper,
    UnifiedComputerTool, WriteTool,
};
use crate::agent::r#loop::{AgentError, ToolExecutor};
use crate::mcp::McpGateway;
use crate::tool_system::ToolSystem;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

pub use gateway_tool_types::ToolFilterContext;

/// Unified tool registry that manages both built-in and MCP tools
pub struct ToolRegistry {
    /// Built-in tools (Read, Write, Edit, Bash, Glob, Grep)
    builtin_tools: HashMap<String, Arc<dyn DynamicTool>>,
    /// MCP Gateway for external tools (optional, legacy - prefer tool_system)
    mcp_gateway: Option<Arc<McpGateway>>,
    /// Unified Tool System (preferred over mcp_gateway)
    tool_system: Option<Arc<ToolSystem>>,
    /// Cached MCP tool schemas (populated by cache_mcp_tools)
    cached_mcp_tools: Vec<Value>,
    /// Enabled namespaces for MCP tool filtering (None = all namespaces)
    enabled_namespaces: Option<Vec<String>>,
    /// Tool discovery mode (A46): only send initial_tools + search_tools to LLM.
    /// Execution layer still has access to ALL registered tools.
    discovery_enabled: bool,
    /// Tools to send to LLM initially when discovery is enabled.
    /// Empty = all tools (legacy behavior, same as discovery_enabled=false).
    initial_tools: Vec<String>,
}

impl ToolRegistry {
    /// Create a new tool registry with default built-in tools
    pub fn new() -> Self {
        let mut builtin_tools: HashMap<String, Arc<dyn DynamicTool>> = HashMap::new();

        // Register all built-in tools
        Self::register_builtin::<ReadTool>(&mut builtin_tools, ReadTool::default());
        Self::register_builtin::<WriteTool>(&mut builtin_tools, WriteTool);
        Self::register_builtin::<EditTool>(&mut builtin_tools, EditTool);
        Self::register_builtin::<BashTool>(&mut builtin_tools, BashTool::new());
        Self::register_builtin::<GlobTool>(&mut builtin_tools, GlobTool::default());
        Self::register_builtin::<GrepTool>(&mut builtin_tools, GrepTool::default());
        // Claude Code CLI tool for autonomous coding tasks
        Self::register_builtin::<ClaudeCodeTool>(&mut builtin_tools, ClaudeCodeTool::new());
        // CodeAct computer tool for code execution (uses local execution by default)
        Self::register_builtin::<LocalComputerTool>(&mut builtin_tools, LocalComputerTool::new());

        Self {
            builtin_tools,
            mcp_gateway: None,
            tool_system: None,
            cached_mcp_tools: Vec::new(),
            enabled_namespaces: None,
            discovery_enabled: false,
            initial_tools: Vec::new(),
        }
    }

    /// Create a read-only tool registry for research/exploration agents.
    ///
    /// Only includes tools that cannot modify the filesystem or execute commands:
    /// Read, Glob, Grep. No Write, Edit, Bash, ClaudeCode, or ComputerTool.
    ///
    /// Used by ResearchPlanner (A43) to safely explore the codebase without
    /// any risk of unintended modifications.
    pub fn new_read_only() -> Self {
        let mut builtin_tools: HashMap<String, Arc<dyn DynamicTool>> = HashMap::new();

        // Read-only tools only
        Self::register_builtin::<ReadTool>(&mut builtin_tools, ReadTool::default());
        Self::register_builtin::<GlobTool>(&mut builtin_tools, GlobTool::default());
        Self::register_builtin::<GrepTool>(&mut builtin_tools, GrepTool::default());

        Self {
            builtin_tools,
            mcp_gateway: None,
            tool_system: None,
            cached_mcp_tools: Vec::new(),
            enabled_namespaces: None,
            discovery_enabled: false,
            initial_tools: Vec::new(),
        }
    }

    /// Create a new tool registry with MCP gateway for external tools
    pub fn with_mcp_gateway(mcp_gateway: Arc<McpGateway>) -> Self {
        let mut registry = Self::new();
        registry.mcp_gateway = Some(mcp_gateway);
        registry
    }

    /// Create a new tool registry with unified ToolSystem
    pub fn with_tool_system(tool_system: Arc<ToolSystem>) -> Self {
        let mut registry = Self::new();
        registry.tool_system = Some(tool_system);
        registry
    }

    /// Set the MCP gateway
    pub fn set_mcp_gateway(&mut self, mcp_gateway: Arc<McpGateway>) {
        self.mcp_gateway = Some(mcp_gateway);
    }

    /// Set the unified ToolSystem
    pub fn set_tool_system(&mut self, tool_system: Arc<ToolSystem>) {
        self.tool_system = Some(tool_system);
    }

    /// Set enabled namespaces for MCP tool filtering.
    ///
    /// When set, only MCP tools from these namespaces will be included
    /// in tool schemas sent to the LLM.
    ///
    /// # Arguments
    /// * `namespaces` - List of namespace names to enable (e.g., ["filesystem", "browser"])
    pub fn set_enabled_namespaces(&mut self, namespaces: Vec<String>) {
        self.enabled_namespaces = Some(namespaces);
    }

    /// Clear enabled namespaces (allow all namespaces)
    pub fn clear_enabled_namespaces(&mut self) {
        self.enabled_namespaces = None;
    }

    /// Enable tool discovery mode (A46).
    ///
    /// When enabled, `get_filtered_tool_schemas()` only returns `initial_tools`
    /// plus the `search_tools` meta-tool. The LLM discovers additional tools
    /// on demand via `search_tools`, reducing per-turn token usage by ~65%.
    ///
    /// The execution layer is NOT affected — all registered tools can still
    /// be executed when called by name.
    pub fn enable_discovery(&mut self, initial_tools: Vec<String>) {
        // Build the searchable catalog from ALL currently registered tools
        let all_metadata = self.get_tool_metadata();
        let catalog = Arc::new(super::SearchableToolCatalog::new(all_metadata));

        // Register the search_tools meta-tool
        let search_tool = super::SearchToolsTool::new(catalog);
        self.register_tool(search_tool);

        // Store discovery config
        self.discovery_enabled = true;
        self.initial_tools = initial_tools;
        // Ensure search_tools is always in the initial set
        if !self.initial_tools.iter().any(|t| t == "search_tools") {
            self.initial_tools.push("search_tools".to_string());
        }

        tracing::info!(
            initial_count = self.initial_tools.len(),
            total_tools = self.builtin_tools.len(),
            "Tool discovery enabled — LLM starts with {} tools, {} discoverable",
            self.initial_tools.len(),
            self.builtin_tools.len() - self.initial_tools.len(),
        );
    }

    /// Check if tool discovery mode is enabled.
    pub fn is_discovery_enabled(&self) -> bool {
        self.discovery_enabled
    }

    /// Cache MCP tool schemas for synchronous access.
    ///
    /// This method should be called after creating the registry to populate
    /// the cached MCP tool schemas. It respects the enabled_namespaces filter.
    ///
    /// # Example
    /// ```ignore
    /// let mut registry = ToolRegistry::with_mcp_gateway(gateway);
    /// registry.set_enabled_namespaces(vec!["filesystem".into(), "browser".into()]);
    /// registry.cache_mcp_tools().await;
    /// ```
    pub async fn cache_mcp_tools(&mut self) {
        if let Some(mcp) = &self.mcp_gateway {
            let tools = match &self.enabled_namespaces {
                Some(namespaces) => mcp.get_tools_filtered(namespaces).await,
                None => mcp.get_tools().await,
            };

            self.cached_mcp_tools = tools
                .into_iter()
                .map(|tool| {
                    serde_json::json!({
                        "name": format!("{}_{}", tool.namespace, tool.name),
                        "description": tool.description,
                        "input_schema": tool.input_schema,
                    })
                })
                .collect();

            tracing::debug!(
                tool_count = self.cached_mcp_tools.len(),
                namespaces = ?self.enabled_namespaces,
                "Cached MCP tool schemas"
            );
        }
    }

    /// Refresh cached MCP tools with new enabled namespaces.
    ///
    /// This is a convenience method that combines set_enabled_namespaces and cache_mcp_tools.
    pub async fn refresh_mcp_tools(&mut self, namespaces: Option<Vec<String>>) {
        if let Some(ns) = namespaces {
            self.enabled_namespaces = Some(ns);
        } else {
            self.enabled_namespaces = None;
        }
        self.cache_mcp_tools().await;
    }

    /// Replace the default LocalComputerTool with a UnifiedComputerTool
    /// that routes execution through the UnifiedCodeActRouter.
    ///
    /// This enables automatic routing to K8s, Docker, or Firecracker backends
    /// with transparent fallback to local execution.
    pub fn with_router(&mut self, router: Arc<crate::executor::UnifiedCodeActRouter>) {
        let unified = UnifiedComputerTool::new(router);
        let name = unified.name().to_string();
        self.builtin_tools
            .insert(name, Arc::new(ToolWrapper::new(unified)));
    }

    /// Register a BrowserTool for browser automation via Firecracker VMs.
    #[cfg(unix)]
    pub fn with_browser_tool(&mut self, vm_manager: Arc<crate::vm::VmManager>) {
        let browser_tool = BrowserTool::new(vm_manager);
        let name = browser_tool.name().to_string();
        self.builtin_tools
            .insert(name, Arc::new(ToolWrapper::new(browser_tool)));
    }

    /// Register screen tools backed by a ScreenController.
    ///
    /// Registers: computer_screenshot, computer_click, computer_type,
    /// computer_key, computer_scroll, computer_drag, and optionally computer_navigate.
    pub fn with_screen_controller(
        &mut self,
        controller: Arc<dyn canal_cv::ScreenController>,
        cdp: Option<Arc<crate::screen::CdpScreenController>>,
    ) {
        crate::screen::register_screen_tools(self, controller, cdp);
    }

    /// Register the OrchestrateTool for Orchestrator-Worker pattern support.
    ///
    /// This enables the agentic loop to spawn parallel worker agents via
    /// tool calling.
    pub fn with_orchestrate_tool(
        &mut self,
        worker_manager: Arc<crate::agent::worker::WorkerManager>,
    ) {
        let orchestrate_tool = super::OrchestrateTool::new(worker_manager);
        self.register_tool(orchestrate_tool);
    }

    /// Register the CodeOrchestrationTool for programmatic tool calling.
    ///
    /// This enables the agentic loop to execute LLM-generated Python/JS code
    /// that programmatically orchestrates multiple tool calls in a Docker sandbox.
    pub fn with_code_orchestration_tool(
        &mut self,
        runtime: Arc<crate::agent::code_orchestration::CodeOrchestrationRuntime>,
    ) {
        let code_orch_tool = super::CodeOrchestrationTool::new(runtime);
        self.register_tool(code_orch_tool);
    }

    /// Register platform control plane tools for managing instances.
    ///
    /// Registers 8 tools: platform_list_instances, platform_create_instance,
    /// platform_start_instance, platform_stop_instance, platform_destroy_instance,
    /// platform_get_status, platform_get_logs, platform_get_metrics.
    pub fn with_platform_tools(&mut self, config: Arc<super::platform::PlatformToolConfig>) {
        super::platform::register_platform_tools(self, config);
    }

    /// Register hosting tools for deploying and managing web applications.
    ///
    /// Registers 8 tools: hosting_deploy_app, hosting_list_apps,
    /// hosting_app_status, hosting_app_logs, hosting_stop_app, hosting_delete_app,
    /// hosting_create_database, hosting_database_status.
    pub fn with_hosting_tools(&mut self, config: Arc<super::hosting::HostingToolConfig>) {
        super::hosting::register_hosting_tools(self, config);
    }

    /// Register devtools observation tools for monitoring infrastructure.
    ///
    /// Registers 4 tools: devtools_containers, devtools_health,
    /// devtools_database_health, devtools_logs.
    pub fn with_devtools_tools(&mut self, config: Arc<super::devtools::DevtoolsToolConfig>) {
        super::devtools::register_devtools_tools(self, config);
    }

    /// Register database tools for per-user database management via Platform Service.
    ///
    /// Registers 11 tools: list_tables, query_rows, execute_sql, create_table,
    /// drop_table, schema_context, explain_sql, migration_list, migration_apply,
    /// migration_rollback, git_deploy.
    #[cfg(feature = "database")]
    pub fn with_database_tools(&mut self, config: Arc<super::database::DatabaseToolConfig>) {
        super::database::register_database_tools(self, config);
    }

    /// Register a built-in tool
    fn register_builtin<T: AgentTool + 'static>(
        tools: &mut HashMap<String, Arc<dyn DynamicTool>>,
        tool: T,
    ) where
        T::Output: 'static,
    {
        let name = tool.name().to_string();
        tools.insert(name, Arc::new(ToolWrapper::new(tool)));
    }

    /// Register a custom built-in tool
    pub fn register_tool<T: AgentTool + 'static>(&mut self, tool: T)
    where
        T::Output: 'static,
    {
        let name = tool.name().to_string();
        self.builtin_tools
            .insert(name, Arc::new(ToolWrapper::new(tool)));
    }

    /// Get a built-in tool by name
    pub fn get_builtin(&self, name: &str) -> Option<&Arc<dyn DynamicTool>> {
        self.builtin_tools.get(name)
    }

    /// List all built-in tool names
    pub fn list_builtin_names(&self) -> Vec<String> {
        self.builtin_tools.keys().cloned().collect()
    }

    /// Get all tool definitions for sending to LLM
    pub async fn get_tool_definitions(&self) -> Vec<Value> {
        let mut definitions = Vec::new();

        // Add built-in tool definitions
        for (_, tool) in &self.builtin_tools {
            let metadata = tool.metadata();
            definitions.push(serde_json::json!({
                "name": metadata.name,
                "description": metadata.description,
                "input_schema": metadata.input_schema,
            }));
        }

        // Add MCP tool definitions (prefer ToolSystem)
        if let Some(ts) = &self.tool_system {
            definitions.extend(ts.tools_for_llm().await);
        } else if let Some(mcp) = &self.mcp_gateway {
            definitions.extend(mcp.tools_for_llm().await);
        }

        definitions
    }

    /// Get tool schemas in the format expected by AgentRunner.
    ///
    /// Returns both built-in tool schemas and cached MCP tool schemas.
    /// MCP tools are included from the cache (populated by `cache_mcp_tools`).
    ///
    /// Note: Call `cache_mcp_tools()` after creating the registry to include
    /// MCP tools. If not called, only built-in tools will be returned.
    pub fn get_tool_schemas(&self) -> Vec<Value> {
        let mut schemas: Vec<Value> = self
            .builtin_tools
            .values()
            .map(|tool| {
                let metadata = tool.metadata();
                serde_json::json!({
                    "name": metadata.name,
                    "description": metadata.description,
                    "input_schema": metadata.input_schema,
                })
            })
            .collect();

        // Include cached MCP tools if available
        schemas.extend(self.cached_mcp_tools.clone());

        schemas
    }

    /// Get tool schemas filtered by context to reduce token consumption.
    ///
    /// When discovery mode is enabled (A46), only returns `initial_tools` +
    /// `search_tools`. The LLM discovers additional tools via `search_tools`.
    ///
    /// When discovery is disabled, filters based on task context:
    /// - Core tools (Read, Write, Edit, Bash, Glob, Grep, Computer) are always included
    /// - Browser tools are only included for browser-related tasks
    /// - Orchestrate tool is only included when workers are enabled
    /// - CodeOrchestration tool is only included when code orchestration is enabled
    pub fn get_filtered_tool_schemas(&self, context: &ToolFilterContext) -> Vec<Value> {
        // Discovery mode: only return initial tools (LLM discovers the rest via search_tools)
        if self.discovery_enabled && !self.initial_tools.is_empty() {
            return self
                .builtin_tools
                .iter()
                .filter(|(name, _)| {
                    self.initial_tools
                        .iter()
                        .any(|t| t.as_str() == name.as_str())
                })
                .map(|(_, tool)| {
                    let metadata = tool.metadata();
                    serde_json::json!({
                        "name": metadata.name,
                        "description": metadata.description,
                        "input_schema": metadata.input_schema,
                    })
                })
                .collect();
        }

        // Legacy mode: context-based filtering
        // Core tools that are always included
        let core_tools = [
            "Read",
            "Write",
            "Edit",
            "Bash",
            "Glob",
            "Grep",
            "Computer",
            "ClaudeCode",
        ];

        self.builtin_tools
            .iter()
            .filter(|(name, _)| {
                let name_str = name.as_str();

                // Core tools always included
                if core_tools.contains(&name_str) {
                    return true;
                }

                // Computer Use tools (PRIMARY) and Browser tools (FALLBACK) only for browser tasks
                // Note: computer_click is excluded - always use computer_click_ref for more reliable clicking
                if name_str.starts_with("computer_")
                    || name_str.starts_with("browser_")
                    || name_str == "BrowserTool"
                {
                    // Exclude computer_click - LLM should use computer_click_ref instead
                    // click_ref uses accessibility tree refs which are more reliable than coordinates
                    if name_str == "computer_click" {
                        return false;
                    }
                    return context.is_browser_task;
                }

                // Orchestrate tool only when workers enabled
                if name_str == "Orchestrate" {
                    return context.workers_enabled;
                }

                // CodeOrchestration tool only when enabled
                if name_str == "CodeOrchestration" {
                    return context.code_orchestration_enabled;
                }

                // Platform tools always included
                if name_str.starts_with("platform_") {
                    return true;
                }

                // Include other tools by default
                true
            })
            .map(|(_, tool)| {
                let metadata = tool.metadata();
                serde_json::json!({
                    "name": metadata.name,
                    "description": metadata.description,
                    "input_schema": metadata.input_schema,
                })
            })
            .collect()
    }

    /// Get filtered tool schemas asynchronously (includes MCP tools)
    pub async fn get_filtered_tool_definitions(&self, context: &ToolFilterContext) -> Vec<Value> {
        let mut definitions = self.get_filtered_tool_schemas(context);

        // Add MCP tool definitions (prefer ToolSystem)
        if let Some(ts) = &self.tool_system {
            definitions.extend(ts.tools_for_llm().await);
        } else if let Some(mcp) = &self.mcp_gateway {
            definitions.extend(mcp.tools_for_llm().await);
        }

        definitions
    }

    /// Get all tool metadata
    pub fn get_tool_metadata(&self) -> Vec<ToolMetadata> {
        self.builtin_tools
            .values()
            .map(|t| t.metadata().clone())
            .collect()
    }

    /// List all agent built-in tools with full metadata.
    ///
    /// This is used by the API layer to expose agent tools alongside MCP tools,
    /// allowing users to see all available tools with clear source attribution.
    // R1-M138: Delegates to get_tool_metadata() — was a duplicate implementation
    pub fn list_agent_tools(&self) -> Vec<ToolMetadata> {
        self.get_tool_metadata()
    }

    /// Check if a tool exists (built-in or MCP)
    pub fn has_tool(&self, name: &str) -> bool {
        // R1-H10: Check built-in tools and cached MCP tools by name (not just underscore presence)
        self.builtin_tools.contains_key(name)
            || self
                .cached_mcp_tools
                .iter()
                .any(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
    }

    /// Check if a tool is built-in
    pub fn is_builtin(&self, name: &str) -> bool {
        self.builtin_tools.contains_key(name)
    }

    /// Execute a tool by name
    ///
    /// Routes to built-in tools first, then falls back to ToolSystem (preferred) or MCP gateway.
    pub async fn execute(
        &self,
        name: &str,
        input: Value,
        context: &ToolContext,
    ) -> Result<Value, ToolError> {
        // Try built-in tools first (highest priority)
        if let Some(tool) = self.builtin_tools.get(name) {
            return tool.execute_json(input, context).await;
        }

        // Try ToolSystem (preferred path for MCP tools)
        if let Some(ts) = &self.tool_system {
            let result = ts.execute_llm_tool_call(name, input).await;

            return match result {
                Ok(tool_result) => {
                    if tool_result.is_error {
                        Ok(serde_json::json!({
                            "error": tool_result.text_content().unwrap_or_else(|| "Unknown error".to_string()),
                            "is_error": true
                        }))
                    } else {
                        Ok(serde_json::json!({
                            "content": tool_result.content,
                            "is_error": false
                        }))
                    }
                }
                Err(e) => Err(ToolError::ExecutionError(e.to_string())),
            };
        }

        // Fall back to legacy MCP gateway
        if let Some(mcp) = &self.mcp_gateway {
            let result = mcp.execute_llm_tool_call(name, input).await;

            match result {
                Ok(tool_result) => {
                    if tool_result.is_error {
                        Ok(serde_json::json!({
                            "error": tool_result.text_content().unwrap_or_else(|| "Unknown error".to_string()),
                            "is_error": true
                        }))
                    } else {
                        Ok(serde_json::json!({
                            "content": tool_result.content,
                            "is_error": false
                        }))
                    }
                }
                Err(e) => Err(ToolError::ExecutionError(e.to_string())),
            }
        } else {
            Err(ToolError::NotFound(format!("Tool not found: {}", name)))
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Implement ToolExecutor trait for ToolRegistry
/// This allows AgentRunner to use ToolRegistry directly
#[async_trait]
impl ToolExecutor for ToolRegistry {
    async fn execute(
        &self,
        tool_name: &str,
        tool_input: Value,
        context: &ToolContext,
    ) -> Result<Value, AgentError> {
        self.execute(tool_name, tool_input, context)
            .await
            .map_err(|e| AgentError::ToolError(e.to_string()))
    }

    fn get_tool_schemas(&self) -> Vec<Value> {
        ToolRegistry::get_tool_schemas(self)
    }

    fn get_filtered_tool_schemas(&self, context: &ToolFilterContext) -> Vec<Value> {
        ToolRegistry::get_filtered_tool_schemas(self, context)
    }
}

/// Builder for ToolRegistry with fluent configuration
pub struct ToolRegistryBuilder {
    registry: ToolRegistry,
}

impl ToolRegistryBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            registry: ToolRegistry::new(),
        }
    }

    /// Add MCP gateway for external tools
    pub fn with_mcp_gateway(mut self, gateway: Arc<McpGateway>) -> Self {
        self.registry.mcp_gateway = Some(gateway);
        self
    }

    /// Add unified ToolSystem
    pub fn with_tool_system(mut self, tool_system: Arc<ToolSystem>) -> Self {
        self.registry.tool_system = Some(tool_system);
        self
    }

    /// Register a custom tool
    pub fn with_tool<T: AgentTool + 'static>(mut self, tool: T) -> Self
    where
        T::Output: 'static,
    {
        self.registry.register_tool(tool);
        self
    }

    /// Replace the computer tool with a unified router-backed version
    pub fn with_router(mut self, router: Arc<crate::executor::UnifiedCodeActRouter>) -> Self {
        self.registry.with_router(router);
        self
    }

    /// Add browser tool for browser automation
    #[cfg(unix)]
    pub fn with_browser_tool(mut self, vm_manager: Arc<crate::vm::VmManager>) -> Self {
        self.registry.with_browser_tool(vm_manager);
        self
    }

    /// Add screen tools backed by a ScreenController
    pub fn with_screen_controller(
        mut self,
        controller: Arc<dyn canal_cv::ScreenController>,
        cdp: Option<Arc<crate::screen::CdpScreenController>>,
    ) -> Self {
        self.registry.with_screen_controller(controller, cdp);
        self
    }

    /// Add orchestrate tool for worker management
    pub fn with_orchestrate_tool(
        mut self,
        worker_manager: Arc<crate::agent::worker::WorkerManager>,
    ) -> Self {
        self.registry.with_orchestrate_tool(worker_manager);
        self
    }

    /// Add code orchestration tool for programmatic tool calling
    pub fn with_code_orchestration_tool(
        mut self,
        runtime: Arc<crate::agent::code_orchestration::CodeOrchestrationRuntime>,
    ) -> Self {
        self.registry.with_code_orchestration_tool(runtime);
        self
    }

    /// Add platform control plane tools
    pub fn with_platform_tools(mut self, config: Arc<super::platform::PlatformToolConfig>) -> Self {
        self.registry.with_platform_tools(config);
        self
    }

    /// Build the registry
    pub fn build(self) -> ToolRegistry {
        self.registry
    }
}

impl Default for ToolRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_registry_has_builtin_tools() {
        let registry = ToolRegistry::new();

        assert!(registry.is_builtin("Read"));
        assert!(registry.is_builtin("Write"));
        assert!(registry.is_builtin("Edit"));
        assert!(registry.is_builtin("Bash"));
        assert!(registry.is_builtin("Glob"));
        assert!(registry.is_builtin("Grep"));

        assert!(!registry.is_builtin("NonExistent"));
    }

    #[test]
    fn test_registry_list_tools() {
        let registry = ToolRegistry::new();
        let names = registry.list_builtin_names();

        assert!(names.contains(&"Read".to_string()));
        assert!(names.contains(&"Write".to_string()));
        assert!(names.contains(&"Edit".to_string()));
        assert!(names.contains(&"Bash".to_string()));
        assert!(names.contains(&"Glob".to_string()));
        assert!(names.contains(&"Grep".to_string()));
    }

    #[test]
    fn test_registry_get_schemas() {
        let registry = ToolRegistry::new();
        let schemas = registry.get_tool_schemas();

        assert_eq!(schemas.len(), 8); // 8 built-in tools (Read, Write, Edit, Bash, Glob, Grep, ClaudeCode, Computer)

        // Check that schemas have required fields
        for schema in &schemas {
            assert!(schema.get("name").is_some());
            assert!(schema.get("description").is_some());
            assert!(schema.get("input_schema").is_some());
        }
    }

    #[tokio::test]
    async fn test_registry_execute_read() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        std::fs::write(&file_path, "Hello, World!").unwrap();

        let registry = ToolRegistry::new();
        let context = ToolContext::new("test-session", temp_dir.path())
            .with_allowed_directory(temp_dir.path());

        let input = serde_json::json!({
            "file_path": file_path.to_string_lossy()
        });

        let result = registry.execute("Read", input, &context).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert!(output.get("content").is_some());
    }

    #[tokio::test]
    async fn test_registry_execute_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let registry = ToolRegistry::new();
        let context = ToolContext::new("test-session", temp_dir.path());

        let result = registry
            .execute("NonExistent", serde_json::json!({}), &context)
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_builder() {
        let registry = ToolRegistryBuilder::new().build();

        assert!(registry.is_builtin("Read"));
        assert!(registry.is_builtin("Bash"));
    }

    #[test]
    fn test_list_agent_tools() {
        let registry = ToolRegistry::new();
        let tools = registry.list_agent_tools();

        // Should return metadata for all 8 built-in tools
        assert_eq!(tools.len(), 8);

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(tool_names.contains(&"Read"));
        assert!(tool_names.contains(&"Write"));
        assert!(tool_names.contains(&"Edit"));
        assert!(tool_names.contains(&"Bash"));
        assert!(tool_names.contains(&"Glob"));
        assert!(tool_names.contains(&"Grep"));

        // Verify metadata fields are populated
        for tool in &tools {
            assert!(!tool.name.is_empty());
            assert!(!tool.description.is_empty());
            assert!(!tool.namespace.is_empty());
        }
    }

    #[test]
    fn test_tool_filter_context_default() {
        let context = ToolFilterContext::new();
        assert!(!context.is_browser_task);
        assert!(!context.workers_enabled);
        assert!(!context.code_orchestration_enabled);
    }

    #[test]
    fn test_tool_filter_context_builder() {
        let context = ToolFilterContext::new()
            .browser_task(true)
            .workers_enabled(true)
            .code_orchestration_enabled(false);

        assert!(context.is_browser_task);
        assert!(context.workers_enabled);
        assert!(!context.code_orchestration_enabled);
    }

    #[test]
    fn test_detect_browser_task() {
        assert!(ToolFilterContext::detect_browser_task(
            "Navigate to the website"
        ));
        assert!(ToolFilterContext::detect_browser_task(
            "Click the login button"
        ));
        assert!(ToolFilterContext::detect_browser_task("Open Gmail"));
        assert!(ToolFilterContext::detect_browser_task("Fill the form"));
        assert!(ToolFilterContext::detect_browser_task("Take a screenshot"));

        assert!(!ToolFilterContext::detect_browser_task("Write a function"));
        assert!(!ToolFilterContext::detect_browser_task("Read the file"));
        assert!(!ToolFilterContext::detect_browser_task("Run the tests"));
    }

    #[test]
    fn test_filtered_tool_schemas_core_tools() {
        let registry = ToolRegistry::new();

        // Non-browser context should still include core tools
        let context = ToolFilterContext::new();
        let schemas = registry.get_filtered_tool_schemas(&context);

        let tool_names: Vec<String> = schemas
            .iter()
            .filter_map(|s| s.get("name").and_then(|n| n.as_str()).map(String::from))
            .collect();

        assert!(tool_names.contains(&"Read".to_string()));
        assert!(tool_names.contains(&"Write".to_string()));
        assert!(tool_names.contains(&"Edit".to_string()));
        assert!(tool_names.contains(&"Bash".to_string()));
        assert!(tool_names.contains(&"Glob".to_string()));
        assert!(tool_names.contains(&"Grep".to_string()));
    }
}
