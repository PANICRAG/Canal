//! MCP Gateway implementation

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::builtin::BuiltinToolExecutor;
use super::connection::{McpConnection, McpHttpConfig, McpSpawnConfig};
use super::protocol::{McpToolDef, ToolCallResult};
use super::registry::{Tool, ToolRegistry};
use crate::agent::types::{
    PermissionBehavior, PermissionContext, PermissionMode, PermissionResult, PermissionRule,
};
use crate::error::{Error, Result};

/// MCP Transport type
#[derive(Debug, Clone)]
pub enum McpTransport {
    /// STDIO transport - runs command as subprocess
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    /// HTTP transport - connects to HTTP endpoint via JSON-RPC over HTTP
    Http { url: String },
}

/// MCP Server configuration
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub name: String,
    pub transport: McpTransport,
    pub enabled: bool,
    pub namespace: String,
    pub startup_timeout_secs: u64,
    pub auto_restart: bool,
    /// Optional Bearer token for HTTP transport authentication
    pub auth_token: Option<String>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            transport: McpTransport::Http { url: String::new() },
            enabled: true,
            namespace: String::new(),
            startup_timeout_secs: 30,
            auto_restart: false,
            auth_token: None,
        }
    }
}

/// MCP Server info for API responses
#[derive(Debug, Clone)]
pub struct McpServerInfo {
    pub tool_count: usize,
    pub connected: bool,
    pub transport_type: String,
    pub location: String,
    pub server_name: String,
    pub description: String,
}

/// Tool source type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    /// Built-in tool (filesystem, executor, etc.)
    Builtin,
    /// External MCP server tool
    McpServer,
}

/// Metadata for a registered tool
#[derive(Debug, Clone)]
pub struct ToolMeta {
    /// Tool source (builtin or MCP server)
    pub source: ToolSource,
    /// Transport type used to connect to the server ("stdio", "http", "builtin")
    pub transport_type: String,
    /// Server name that provides this tool
    pub server_name: String,
    /// Location (e.g., "local" or remote URL)
    pub location: String,
}

/// Result of a permission check that requires user confirmation
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    /// Tool name that needs permission
    pub tool_name: String,
    /// Question to display to user
    pub question: String,
    /// Tool arguments
    pub arguments: serde_json::Value,
}

/// MCP Gateway
///
/// Manages connections to MCP servers and provides a unified interface
/// for tool discovery and invocation.
pub struct McpGateway {
    configs: Arc<RwLock<HashMap<String, McpServerConfig>>>,
    connections: Arc<RwLock<HashMap<String, McpConnection>>>,
    tool_registry: Arc<RwLock<ToolRegistry>>,
    builtin_executor: Arc<RwLock<Option<BuiltinToolExecutor>>>,
    permission_ctx: Arc<RwLock<PermissionContext>>,
    tool_metadata: Arc<RwLock<HashMap<String, ToolMeta>>>,
}

impl McpGateway {
    /// Create a new MCP gateway
    pub fn new() -> Self {
        // Default permission context allows all operations (bypass mode)
        // for backward compatibility
        let mut default_ctx = PermissionContext::default();
        default_ctx.mode = PermissionMode::BypassPermissions;

        Self {
            configs: Arc::new(RwLock::new(HashMap::new())),
            connections: Arc::new(RwLock::new(HashMap::new())),
            tool_registry: Arc::new(RwLock::new(ToolRegistry::new())),
            builtin_executor: Arc::new(RwLock::new(None)),
            permission_ctx: Arc::new(RwLock::new(default_ctx)),
            tool_metadata: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set the permission mode for the gateway
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

    /// Check permission for a tool call (used internally)
    async fn check_permission(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> PermissionResult {
        let ctx = self.permission_ctx.read().await;
        ctx.check_tool(tool_name, arguments)
    }

    /// Set the builtin tool executor for handling filesystem and executor namespaces
    pub async fn set_builtin_executor(&self, executor: BuiltinToolExecutor) {
        let mut builtin = self.builtin_executor.write().await;
        *builtin = Some(executor);

        // Register builtin tools in the registry
        self.register_builtin_tools().await;
    }

    /// Set the screen controller on the builtin executor
    pub async fn set_builtin_screen_controller(
        &self,
        controller: Arc<dyn canal_cv::ScreenController>,
    ) {
        let mut builtin = self.builtin_executor.write().await;
        if let Some(ref mut executor) = *builtin {
            executor.set_screen_controller(controller);
            tracing::info!("Screen controller registered with builtin executor");
        }
    }

    /// Set the CDP screen controller on the builtin executor
    pub async fn set_builtin_cdp_controller(&self, cdp: Arc<crate::screen::CdpScreenController>) {
        let mut builtin = self.builtin_executor.write().await;
        if let Some(ref mut executor) = *builtin {
            executor.set_cdp_controller(cdp);
            tracing::info!("CDP controller registered with builtin executor");
        }
    }

    /// Set the automation orchestrator on the builtin executor
    /// Call this after automation orchestrator is initialized
    pub async fn set_builtin_automation(
        &self,
        automation: Arc<crate::agent::automation::BrowserAutomationOrchestrator>,
    ) {
        let mut builtin = self.builtin_executor.write().await;
        if let Some(ref mut executor) = *builtin {
            executor.set_automation(automation);
            tracing::info!("Automation orchestrator registered with builtin executor");
        }
    }

    /// Register builtin tools in the tool registry
    async fn register_builtin_tools(&self) {
        let mut registry = self.tool_registry.write().await;
        let mut metadata = self.tool_metadata.write().await;

        // Filesystem tools
        registry.register(Tool {
            namespace: "filesystem".to_string(),
            name: "read_file".to_string(),
            description: "Read contents of a file from the local filesystem".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read"
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "filesystem".to_string(),
            name: "write_file".to_string(),
            description: "Write content to a file on the local filesystem".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path", "content"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to write to"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write"
                    },
                    "create_dirs": {
                        "type": "boolean",
                        "description": "Create parent directories if they don't exist",
                        "default": true
                    },
                    "overwrite": {
                        "type": "boolean",
                        "description": "Overwrite existing file",
                        "default": true
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "filesystem".to_string(),
            name: "list_directory".to_string(),
            description: "List contents of a directory".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory path"
                    },
                    "recursive": {
                        "type": "boolean",
                        "default": false
                    },
                    "include_hidden": {
                        "type": "boolean",
                        "default": false
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "filesystem".to_string(),
            name: "search".to_string(),
            description: "Search for content in files using ripgrep".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path", "pattern"],
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory to search in"
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Search pattern (regex)"
                    },
                    "file_pattern": {
                        "type": "string",
                        "description": "File glob pattern (e.g., '*.rs')"
                    },
                    "max_results": {
                        "type": "integer",
                        "default": 100
                    }
                }
            }),
        });

        // Executor tools
        registry.register(Tool {
            namespace: "executor".to_string(),
            name: "bash".to_string(),
            description: "Execute a bash command".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["command"],
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Bash command to execute"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory for command execution"
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "executor".to_string(),
            name: "python".to_string(),
            description: "Execute Python code".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["code"],
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python code to execute"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds"
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "executor".to_string(),
            name: "run_code".to_string(),
            description: "Execute code in a supported language".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["code", "language"],
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Code to execute"
                    },
                    "language": {
                        "type": "string",
                        "enum": ["python", "bash", "javascript", "typescript", "go", "rust"],
                        "description": "Programming language"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds"
                    }
                }
            }),
        });

        // Browser tools (via Chrome extension)
        registry.register(Tool {
            namespace: "browser".to_string(),
            name: "navigate".to_string(),
            description:
                "Navigate the browser to a URL. Opens the page in Chrome via the browser extension."
                    .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to navigate to"
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "browser".to_string(),
            name: "snapshot".to_string(),
            description: "Get the current page content as an accessibility tree. Use this to read text, find elements, and understand the page structure.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        });

        registry.register(Tool {
            namespace: "browser".to_string(),
            name: "click".to_string(),
            description: "Click on an element identified by a CSS selector.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["selector"],
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector of the element to click"
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "browser".to_string(),
            name: "fill".to_string(),
            description: "Fill a form field with text.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["selector", "text"],
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector of the input element"
                    },
                    "text": {
                        "type": "string",
                        "description": "Text to fill into the element"
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "browser".to_string(),
            name: "screenshot".to_string(),
            description: "Take a screenshot of the current page.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "full_page": {
                        "type": "boolean",
                        "description": "Capture the full scrollable page",
                        "default": false
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "browser".to_string(),
            name: "scroll".to_string(),
            description: "Scroll the page.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "direction": {
                        "type": "string",
                        "enum": ["up", "down", "left", "right"],
                        "description": "Scroll direction",
                        "default": "down"
                    },
                    "amount": {
                        "type": "integer",
                        "description": "Scroll amount in pixels",
                        "default": 500
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "browser".to_string(),
            name: "wait".to_string(),
            description: "Wait for an element to appear or for navigation to complete.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector to wait for (optional)"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Maximum wait time in milliseconds",
                        "default": 30000
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "browser".to_string(),
            name: "evaluate".to_string(),
            description: "Execute JavaScript in the browser context and return the result."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["script"],
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "JavaScript code to execute"
                    }
                }
            }),
        });

        // ============================================================================
        // macOS Automation Tools (Native Rust Implementation)
        // ============================================================================
        // These tools provide native macOS automation without requiring external
        // Node.js/Python MCP servers (like osascript-dxt or similar).
        //
        // Advantages over external MCP servers:
        // - Zero runtime dependencies (no Node.js/Python)
        // - Faster startup (~1ms vs ~100-500ms)
        // - Lower memory footprint (~5-10MB vs ~30-50MB)
        // - Single binary deployment
        // - Type-safe with compile-time checks

        registry.register(Tool {
            namespace: "mac".to_string(),
            name: "osascript".to_string(),
            description: "Execute AppleScript commands on macOS. Can automate Mac applications like Finder, Mail, Safari, System Preferences, Chrome, Notes, and more. Use this to control Mac apps, get/set system settings, interact with the desktop, open applications, click menu items, etc.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["script"],
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "The AppleScript code to execute. Examples: 'tell application \"Finder\" to activate', 'tell application \"Chrome\" to open location \"https://google.com\"'"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 30)",
                        "default": 30
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "mac".to_string(),
            name: "screenshot".to_string(),
            description: "Capture a screenshot of the screen or a specific region. Requires Screen Recording permission in System Preferences > Privacy & Security.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to save the screenshot (default: ~/Library/Caches/canal/screenshots/screenshot.png)"
                    },
                    "region": {
                        "type": "object",
                        "description": "Optional region to capture",
                        "properties": {
                            "x": {"type": "integer", "description": "X coordinate"},
                            "y": {"type": "integer", "description": "Y coordinate"},
                            "width": {"type": "integer", "description": "Width in pixels"},
                            "height": {"type": "integer", "description": "Height in pixels"}
                        }
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "mac".to_string(),
            name: "app_control".to_string(),
            description: "Control Mac applications - launch, quit, hide, show, or minimize apps."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["action", "app"],
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["launch", "quit", "hide", "show", "minimize"],
                        "description": "The action to perform on the application"
                    },
                    "app": {
                        "type": "string",
                        "description": "The application name (e.g., 'Chrome', 'Finder', 'Safari')"
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "mac".to_string(),
            name: "open_url".to_string(),
            description: "Open a URL in the default browser.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to open (e.g., 'https://google.com')"
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "mac".to_string(),
            name: "notify".to_string(),
            description: "Show a macOS notification with title and message.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["message"],
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Notification title",
                        "default": "Notification"
                    },
                    "message": {
                        "type": "string",
                        "description": "Notification message content"
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "mac".to_string(),
            name: "clipboard_read".to_string(),
            description: "Read the current content from the system clipboard.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        });

        registry.register(Tool {
            namespace: "mac".to_string(),
            name: "clipboard_write".to_string(),
            description: "Write content to the system clipboard.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["content"],
                "properties": {
                    "content": {
                        "type": "string",
                        "description": "Content to copy to the clipboard"
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "mac".to_string(),
            name: "get_frontmost_app".to_string(),
            description: "Get the name of the currently active (frontmost) application."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        });

        registry.register(Tool {
            namespace: "mac".to_string(),
            name: "list_running_apps".to_string(),
            description:
                "List all currently running applications (visible apps, not background processes)."
                    .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        });

        // ============================================================================
        // Automation Tools (Five-Layer Architecture for Massive Token Savings)
        // ============================================================================
        // These tools provide 99.85% token savings compared to pure CV approaches:
        // - Pure CV: ~4,100,000 tokens for 1000 items
        // - Five-Layer: ~6,000 tokens for same task
        //
        // IMPORTANT: Use these tools for repetitive browser automation tasks with data.
        // The system will explore once, generate a reusable script, and execute it.

        registry.register(Tool {
            namespace: "automation".to_string(),
            name: "analyze".to_string(),
            description: "Analyze a browser automation task and get the optimal execution path. \
                Returns routing recommendation (script reuse, exploration, pure CV, or API call) \
                with estimated token costs. Use this FIRST before executing any repetitive browser task \
                with multiple data items to understand the most efficient approach.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["task"],
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Description of the automation task to analyze, e.g., 'Fill Google Sheets with customer data'"
                    },
                    "data_count": {
                        "type": "integer",
                        "description": "Number of data items to process (helps determine optimal path)"
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "automation".to_string(),
            name: "execute".to_string(),
            description: "Execute a browser automation task through the five-layer pipeline. \
                This is the RECOMMENDED way to handle repetitive browser tasks with data. \
                The system will: 1) Analyze the optimal path, 2) Explore the page (if needed), \
                3) Generate a reusable script, 4) Execute with zero token cost per item. \
                MASSIVE TOKEN SAVINGS: ~6,000 tokens vs ~4,100,000 for 1000 items."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["task", "target_url"],
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Description of the automation task"
                    },
                    "target_url": {
                        "type": "string",
                        "description": "URL of the target page"
                    },
                    "data": {
                        "type": "array",
                        "items": { "type": "object" },
                        "description": "Array of data objects to process"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds (default: 300000 = 5 minutes)",
                        "default": 300000
                    },
                    "force_explore": {
                        "type": "boolean",
                        "description": "Force re-exploration even if a cached script exists",
                        "default": false
                    }
                }
            }),
        });

        registry.register(Tool {
            namespace: "automation".to_string(),
            name: "status".to_string(),
            description:
                "Get the status of the automation orchestrator, including browser connection, \
                cached scripts count, and usage metrics."
                    .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        });

        // Register metadata for all builtin tools
        let builtin_meta = ToolMeta {
            source: ToolSource::Builtin,
            transport_type: "builtin".to_string(),
            server_name: "builtin".to_string(),
            location: "local".to_string(),
        };

        for tool_name in &[
            // Filesystem tools
            "filesystem_read_file",
            "filesystem_write_file",
            "filesystem_list_directory",
            "filesystem_search",
            // Executor tools
            "executor_bash",
            "executor_python",
            "executor_run_code",
            // Browser tools
            "browser_navigate",
            "browser_snapshot",
            "browser_click",
            "browser_fill",
            "browser_screenshot",
            "browser_scroll",
            "browser_wait",
            "browser_evaluate",
            // macOS automation tools (9 tools - native Rust implementation)
            "mac_osascript",
            "mac_screenshot",
            "mac_app_control",
            "mac_open_url",
            "mac_notify",
            "mac_clipboard_read",
            "mac_clipboard_write",
            "mac_get_frontmost_app",
            "mac_list_running_apps",
            // Automation tools (five-layer architecture for massive token savings)
            "automation_analyze",
            "automation_execute",
            "automation_status",
        ] {
            metadata.insert(tool_name.to_string(), builtin_meta.clone());
        }
    }

    /// Register an MCP server configuration
    pub async fn register_server_config(&self, config: McpServerConfig) -> Result<()> {
        let name = config.name.clone();

        tracing::info!(server = %name, "Registering MCP server");

        let mut configs = self.configs.write().await;
        configs.insert(name, config);

        Ok(())
    }

    /// Unregister an MCP server by name
    pub async fn unregister_server_by_name(&self, name: &str) -> Result<()> {
        tracing::info!(server = %name, "Unregistering MCP server");

        let mut configs = self.configs.write().await;
        configs.remove(name);

        // Also disconnect if connected
        let mut connections = self.connections.write().await;
        connections.remove(name);

        Ok(())
    }

    /// Get server configuration
    pub async fn get_server(&self, name: &str) -> Option<McpServerConfig> {
        let configs = self.configs.read().await;
        configs.get(name).cloned()
    }

    /// List all registered server configs
    pub async fn list_server_configs(&self) -> Vec<McpServerConfig> {
        let configs = self.configs.read().await;
        configs.values().cloned().collect()
    }

    /// Connect to a specific server
    pub async fn connect_server(&self, name: &str) -> Result<()> {
        let configs = self.configs.read().await;
        let config = configs
            .get(name)
            .ok_or_else(|| Error::NotFound(format!("Server config not found: {}", name)))?
            .clone();
        drop(configs);

        if !config.enabled {
            tracing::info!(server = %name, "Server is disabled, skipping connection");
            return Ok(());
        }

        match &config.transport {
            McpTransport::Stdio { command, args, env } => {
                let spawn_config = McpSpawnConfig {
                    name: config.name.clone(),
                    command: command.clone(),
                    args: args.clone(),
                    env: env.clone(),
                    startup_timeout_secs: config.startup_timeout_secs,
                    request_timeout_secs: 30,
                };

                let connection = McpConnection::spawn(spawn_config).await?;

                // Register tools from this connection
                let tools = connection.tools().await;
                self.register_tools_from_server(
                    &config.namespace,
                    &config.name,
                    &config.transport,
                    &tools,
                )
                .await;

                let mut connections = self.connections.write().await;
                connections.insert(config.name.clone(), connection);

                tracing::info!(
                    server = %name,
                    tool_count = tools.len(),
                    "Connected to MCP server"
                );
            }
            McpTransport::Http { url } => {
                let http_config = McpHttpConfig {
                    name: config.name.clone(),
                    url: url.clone(),
                    startup_timeout_secs: config.startup_timeout_secs,
                    auth_token: config.auth_token.clone(),
                    request_timeout_secs: 30,
                };

                let connection = McpConnection::connect_http(http_config).await?;

                // Register tools from this connection
                let tools = connection.tools().await;
                self.register_tools_from_server(
                    &config.namespace,
                    &config.name,
                    &config.transport,
                    &tools,
                )
                .await;

                let mut connections = self.connections.write().await;
                connections.insert(config.name.clone(), connection);

                tracing::info!(
                    server = %name,
                    url = %url,
                    tool_count = tools.len(),
                    "Connected to MCP server via HTTP"
                );
            }
        }

        Ok(())
    }

    /// Connect to all enabled servers
    pub async fn connect_all(&self) -> Result<()> {
        let configs = self.configs.read().await;
        let names: Vec<String> = configs
            .iter()
            .filter(|(_, c)| c.enabled)
            .map(|(n, _)| n.clone())
            .collect();
        drop(configs);

        for name in names {
            if let Err(e) = self.connect_server(&name).await {
                tracing::error!(server = %name, error = %e, "Failed to connect to server");
            }
        }

        Ok(())
    }

    /// Disconnect from all servers
    pub async fn disconnect_all(&self) -> Result<()> {
        let mut connections = self.connections.write().await;
        let names: Vec<String> = connections.keys().cloned().collect();

        for name in names {
            tracing::info!(server = %name, "Disconnecting from MCP server");
            connections.remove(&name);
        }

        Ok(())
    }

    /// Register tools from a connected server
    async fn register_tools_from_server(
        &self,
        namespace: &str,
        server_name: &str,
        transport: &McpTransport,
        tools: &[McpToolDef],
    ) {
        let mut registry = self.tool_registry.write().await;
        let mut metadata = self.tool_metadata.write().await;

        let (transport_type, location) = match transport {
            McpTransport::Stdio { command, .. } => {
                ("stdio".to_string(), format!("local:{}", command))
            }
            McpTransport::Http { url } => ("http".to_string(), url.clone()),
        };

        for mcp_tool in tools {
            let tool = Tool {
                namespace: namespace.to_string(),
                name: mcp_tool.name.clone(),
                // R3-M: Provide meaningful default when MCP tool has no description
                description: mcp_tool
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("Tool: {}", mcp_tool.name)),
                input_schema: mcp_tool.input_schema.clone(),
            };

            // R3-H14: Use `__` (double underscore) separator to avoid namespace collision.
            // Single `_` was ambiguous: "my_server_my_tool" could split as
            // (my, server_my_tool) or (my_server, my_tool).
            let full_name = format!("{}__{}", namespace, mcp_tool.name);
            metadata.insert(
                full_name,
                ToolMeta {
                    source: ToolSource::McpServer,
                    transport_type: transport_type.clone(),
                    server_name: server_name.to_string(),
                    location: location.clone(),
                },
            );

            registry.register(tool);
        }
    }

    /// Get available tools from registry
    pub async fn get_tools(&self) -> Vec<Tool> {
        let registry = self.tool_registry.read().await;
        registry.list_tools()
    }

    /// Get tools filtered by enabled namespaces
    pub async fn get_tools_filtered(&self, enabled_namespaces: &[String]) -> Vec<Tool> {
        let registry = self.tool_registry.read().await;
        registry
            .list_tools()
            .into_iter()
            .filter(|t| enabled_namespaces.contains(&t.namespace))
            .collect()
    }

    /// Get tool metadata for a specific tool (namespace_name format)
    pub async fn get_tool_meta(&self, full_tool_name: &str) -> Option<ToolMeta> {
        let metadata = self.tool_metadata.read().await;
        metadata.get(full_tool_name).cloned()
    }

    /// Get all tool metadata
    pub async fn get_all_tool_meta(&self) -> HashMap<String, ToolMeta> {
        let metadata = self.tool_metadata.read().await;
        metadata.clone()
    }

    /// Get tools by namespace
    pub async fn get_tools_by_namespace(&self, namespace: &str) -> Vec<Tool> {
        let registry = self.tool_registry.read().await;
        registry.list_by_namespace(namespace)
    }

    /// Call a tool
    pub async fn call_tool(
        &self,
        namespace: &str,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value> {
        // Check builtin executor first (handles filesystem, executor, browser, mac, etc.)
        {
            let builtin = self.builtin_executor.read().await;
            if let Some(ref executor) = *builtin {
                if executor.handles_namespace(namespace) {
                    drop(builtin);
                    let builtin = self.builtin_executor.read().await;
                    if let Some(ref executor) = *builtin {
                        let result = executor.execute(namespace, tool_name, input).await?;
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
                        return Ok(output);
                    }
                }
            }
        }

        // Fall back to external MCP server
        let configs = self.configs.read().await;
        let server_name = configs
            .iter()
            .find(|(_, c)| c.namespace == namespace)
            .map(|(n, _)| n.clone());
        drop(configs);

        let server_name = server_name.ok_or_else(|| {
            Error::NotFound(format!("No server found for namespace: {}", namespace))
        })?;

        // Get the connection
        let connections = self.connections.read().await;
        let connection = connections.get(&server_name).ok_or_else(|| {
            Error::NotFound(format!(
                "Server '{}' is not connected. Call connect_server first.",
                server_name
            ))
        })?;

        tracing::info!(
            namespace = namespace,
            tool = tool_name,
            server = %server_name,
            "Calling MCP tool"
        );

        // Call the tool
        let result = connection.call_tool(tool_name, input).await?;

        // Convert result to JSON
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

    /// Get tools formatted for LLM
    pub async fn tools_for_llm(&self) -> Vec<serde_json::Value> {
        let tools = self.get_tools().await;

        tools
            .into_iter()
            .map(|tool| {
                serde_json::json!({
                    "name": format!("{}__{}", tool.namespace, tool.name),
                    "description": tool.description,
                    "input_schema": tool.input_schema
                })
            })
            .collect()
    }

    /// Execute a tool call from LLM format (namespace__toolname)
    ///
    /// Returns `Ok(ToolCallResult)` for successful execution or errors,
    /// or `Err(Error::PermissionDenied)` if the tool requires user approval.
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
                // Return a special error that indicates user confirmation is needed
                // The caller should handle this by showing a permission dialog
                return Err(Error::PermissionDenied(format!(
                    "PERMISSION_REQUIRED:{}:{}:{}",
                    tool_name,
                    question,
                    serde_json::to_string(&arguments).unwrap_or_default()
                )));
            }
        }

        // R3-H14: Parse namespace__toolname format (double underscore separator)
        let (namespace, actual_tool_name) = match tool_name.split_once("__") {
            Some((ns, tn)) => (ns, tn),
            None => {
                return Ok(ToolCallResult::error(format!(
                    "Invalid tool name format '{}'. Expected 'namespace__toolname'",
                    tool_name
                )));
            }
        };

        // Check if this is a builtin tool first
        {
            let builtin = self.builtin_executor.read().await;
            if let Some(ref executor) = *builtin {
                if executor.handles_namespace(namespace) {
                    // We need to drop the lock and re-acquire to call async method
                    let handles = true;
                    drop(builtin);
                    if handles {
                        let builtin = self.builtin_executor.read().await;
                        if let Some(ref executor) = *builtin {
                            return executor
                                .execute(namespace, actual_tool_name, arguments)
                                .await;
                        }
                    }
                }
            }
        }

        // Fall back to external MCP server
        let result = self.call_tool(namespace, actual_tool_name, arguments).await;

        match result {
            Ok(output) => {
                if output.get("success") == Some(&serde_json::json!(true)) {
                    Ok(ToolCallResult::text(
                        serde_json::to_string_pretty(&output).unwrap_or_default(),
                    ))
                } else {
                    Ok(ToolCallResult::error(
                        output
                            .get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("Unknown error")
                            .to_string(),
                    ))
                }
            }
            Err(e) => Ok(ToolCallResult::error(e.to_string())),
        }
    }

    /// Execute a tool call with pre-approved permission
    ///
    /// Use this when the user has already approved the permission request.
    /// This bypasses permission checks entirely.
    pub async fn execute_llm_tool_call_approved(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolCallResult> {
        // R3-H14: Parse namespace__toolname format (double underscore separator)
        let (namespace, actual_tool_name) = match tool_name.split_once("__") {
            Some((ns, tn)) => (ns, tn),
            None => {
                return Ok(ToolCallResult::error(format!(
                    "Invalid tool name format '{}'. Expected 'namespace__toolname'",
                    tool_name
                )));
            }
        };

        // Check if this is a builtin tool first
        {
            let builtin = self.builtin_executor.read().await;
            if let Some(ref executor) = *builtin {
                if executor.handles_namespace(namespace) {
                    let handles = true;
                    drop(builtin);
                    if handles {
                        let builtin = self.builtin_executor.read().await;
                        if let Some(ref executor) = *builtin {
                            return executor
                                .execute(namespace, actual_tool_name, arguments)
                                .await;
                        }
                    }
                }
            }
        }

        // Fall back to external MCP server
        let result = self.call_tool(namespace, actual_tool_name, arguments).await;

        match result {
            Ok(output) => {
                if output.get("success") == Some(&serde_json::json!(true)) {
                    Ok(ToolCallResult::text(
                        serde_json::to_string_pretty(&output).unwrap_or_default(),
                    ))
                } else {
                    Ok(ToolCallResult::error(
                        output
                            .get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("Unknown error")
                            .to_string(),
                    ))
                }
            }
            Err(e) => Ok(ToolCallResult::error(e.to_string())),
        }
    }

    /// Parse a permission required error message
    ///
    /// Returns `Some(PermissionRequest)` if the error is a permission request,
    /// or `None` if it's a regular error.
    pub fn parse_permission_request(error: &Error) -> Option<PermissionRequest> {
        if let Error::PermissionDenied(msg) = error {
            if let Some(rest) = msg.strip_prefix("PERMISSION_REQUIRED:") {
                // Format: PERMISSION_REQUIRED:tool_name:question:json_arguments
                // Use splitn(3, ':') on the rest to get tool_name, then question+json
                // Since question can contain ':', find JSON start by searching for last ':{' pattern
                let parts: Vec<&str> = rest.splitn(2, ':').collect();
                if parts.len() >= 2 {
                    let tool_name = parts[0].to_string();
                    let remainder = parts[1];
                    // Find the last occurrence of ':{' which marks the start of JSON arguments
                    if let Some(json_start) = remainder.rfind(":{") {
                        let question = remainder[..json_start].to_string();
                        let arguments =
                            serde_json::from_str(&remainder[json_start + 1..]).unwrap_or_default();
                        return Some(PermissionRequest {
                            tool_name,
                            question,
                            arguments,
                        });
                    } else {
                        // No JSON arguments, entire remainder is the question
                        return Some(PermissionRequest {
                            tool_name,
                            question: remainder.to_string(),
                            arguments: serde_json::Value::Object(Default::default()),
                        });
                    }
                }
            }
        }
        None
    }

    /// Health check - check which servers are connected (legacy - returns HashMap)
    pub async fn health_check_map(&self) -> HashMap<String, bool> {
        let connections = self.connections.read().await;
        let configs = self.configs.read().await;

        let mut status = HashMap::new();
        for name in configs.keys() {
            status.insert(name.clone(), connections.contains_key(name));
        }

        status
    }

    /// Health check for a specific server
    pub async fn health_check(&self, namespace: &str) -> Result<bool> {
        let configs = self.configs.read().await;

        // Find server by namespace
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

    /// List servers with their info (for API)
    pub async fn list_servers(&self) -> Vec<(String, McpServerInfo)> {
        let configs = self.configs.read().await;
        let connections = self.connections.read().await;
        let registry = self.tool_registry.read().await;

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
                    McpServerInfo {
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

    /// Get server info by namespace
    pub async fn get_server_info(&self, namespace: &str) -> Option<McpServerInfo> {
        let configs = self.configs.read().await;
        let connections = self.connections.read().await;
        let registry = self.tool_registry.read().await;

        configs
            .iter()
            .find(|(_, c)| c.namespace == namespace)
            .map(|(name, config)| {
                let connected = connections.contains_key(name);
                let tool_count = registry.list_by_namespace(&config.namespace).len();

                let (transport_type, location) = match &config.transport {
                    McpTransport::Stdio { command, .. } => {
                        ("stdio".to_string(), format!("local:{}", command))
                    }
                    McpTransport::Http { url } => ("http".to_string(), url.clone()),
                };

                McpServerInfo {
                    tool_count,
                    connected,
                    transport_type,
                    location,
                    server_name: config.name.clone(),
                    description: String::new(),
                }
            })
    }

    /// Register a server with individual parameters (for API)
    pub async fn register_server(
        &self,
        namespace: &str,
        command: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> Result<()> {
        let config = McpServerConfig {
            name: namespace.to_string(),
            transport: McpTransport::Stdio {
                command: command.to_string(),
                args,
                env,
            },
            enabled: true,
            namespace: namespace.to_string(),
            startup_timeout_secs: 30,
            auto_restart: false,
            auth_token: None,
        };

        self.register_server_config(config).await
    }

    /// Unregister a server by namespace (for API - doesn't return Result)
    pub async fn unregister_server(&self, namespace: &str) {
        let configs = self.configs.read().await;

        // Find server name by namespace
        let server_name = configs
            .iter()
            .find(|(_, c)| c.namespace == namespace)
            .map(|(n, _)| n.clone());
        drop(configs);

        if let Some(name) = server_name {
            let _ = self.unregister_server_by_name(&name).await;
        }
    }

    /// Get tools for a specific namespace (for API)
    pub async fn get_tools_for_namespace(&self, namespace: &str) -> Vec<Tool> {
        let registry = self.tool_registry.read().await;
        registry.list_by_namespace(namespace)
    }

    /// Restart a specific server
    pub async fn restart_server(&self, name: &str) -> Result<()> {
        tracing::info!(server = %name, "Restarting MCP server");

        // Disconnect first
        {
            let mut connections = self.connections.write().await;
            connections.remove(name);
        }

        // Reconnect
        self.connect_server(name).await
    }
}

impl Default for McpGateway {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for creating an McpGateway with builtin tools
pub struct McpGatewayBuilder {
    gateway: McpGateway,
}

impl McpGatewayBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            gateway: McpGateway::new(),
        }
    }

    /// Set the builtin tool executor
    pub async fn with_builtin_executor(self, executor: BuiltinToolExecutor) -> Self {
        self.gateway.set_builtin_executor(executor).await;
        self
    }

    /// Set the permission mode
    pub async fn with_permission_mode(self, mode: PermissionMode) -> Self {
        self.gateway.set_permission_mode(mode).await;
        self
    }

    /// Add a permission rule
    pub async fn with_permission_rule(
        self,
        rule: PermissionRule,
        behavior: PermissionBehavior,
    ) -> Self {
        self.gateway.add_permission_rule(rule, behavior).await;
        self
    }

    /// Build the gateway
    pub fn build(self) -> McpGateway {
        self.gateway
    }
}

impl Default for McpGatewayBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_server() {
        let gateway = McpGateway::new();

        let config = McpServerConfig {
            name: "test-server".to_string(),
            transport: McpTransport::Http {
                url: "http://localhost:8080".to_string(),
            },
            enabled: true,
            namespace: "test".to_string(),
            startup_timeout_secs: 30,
            auto_restart: false,
            auth_token: None,
        };

        gateway.register_server_config(config).await.unwrap();

        let servers = gateway.list_servers().await;
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].0, "test"); // namespace
    }

    #[tokio::test]
    async fn test_unregister_server() {
        let gateway = McpGateway::new();

        let config = McpServerConfig {
            name: "test-server".to_string(),
            transport: McpTransport::Http {
                url: "http://localhost:8080".to_string(),
            },
            enabled: true,
            namespace: "test".to_string(),
            startup_timeout_secs: 30,
            auto_restart: false,
            auth_token: None,
        };

        gateway.register_server_config(config).await.unwrap();
        gateway.unregister_server("test").await;

        let servers = gateway.list_servers().await;
        assert_eq!(servers.len(), 0);
    }

    #[tokio::test]
    async fn test_health_check() {
        let gateway = McpGateway::new();

        let config = McpServerConfig {
            name: "test-server".to_string(),
            transport: McpTransport::Http {
                url: "http://localhost:8080".to_string(),
            },
            enabled: true,
            namespace: "test".to_string(),
            startup_timeout_secs: 30,
            auto_restart: false,
            auth_token: None,
        };

        gateway.register_server_config(config).await.unwrap();

        let status = gateway.health_check_map().await;
        assert_eq!(status.get("test-server"), Some(&false)); // Not connected yet
    }

    #[tokio::test]
    async fn test_tools_for_llm_format() {
        let gateway = McpGateway::new();

        // Register a tool directly in the registry for testing
        {
            let mut registry = gateway.tool_registry.write().await;
            registry.register(Tool {
                namespace: "videocli".to_string(),
                name: "list_ideas".to_string(),
                description: "List all video ideas".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "number" }
                    }
                }),
            });
        }

        let tools = gateway.tools_for_llm().await;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "videocli__list_ideas");
    }

    #[tokio::test]
    async fn test_permission_mode_default_bypass() {
        let gateway = McpGateway::new();

        // Default mode should be BypassPermissions for backward compatibility
        let mode = gateway.get_permission_mode().await;
        assert_eq!(mode, PermissionMode::BypassPermissions);
    }

    #[tokio::test]
    async fn test_set_permission_mode() {
        let gateway = McpGateway::new();

        gateway.set_permission_mode(PermissionMode::Plan).await;
        let mode = gateway.get_permission_mode().await;
        assert_eq!(mode, PermissionMode::Plan);

        gateway.set_permission_mode(PermissionMode::Default).await;
        let mode = gateway.get_permission_mode().await;
        assert_eq!(mode, PermissionMode::Default);
    }

    #[tokio::test]
    async fn test_add_permission_rule() {
        let gateway = McpGateway::new();

        // Set to Default mode so rules are checked
        gateway.set_permission_mode(PermissionMode::Default).await;

        // Add a rule to allow read_file
        gateway
            .add_permission_rule(
                PermissionRule::tool("*_read_file"),
                PermissionBehavior::Allow,
            )
            .await;

        // Check permission for read_file - should be allowed
        let result = gateway
            .check_permission(
                "filesystem_read_file",
                &serde_json::json!({"path": "/tmp/test"}),
            )
            .await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_permission_plan_mode_denies_writes() {
        let gateway = McpGateway::new();

        // Set Plan mode
        gateway.set_permission_mode(PermissionMode::Plan).await;

        // Check permission for Write - should be denied
        let result = gateway
            .check_permission(
                "filesystem_Write",
                &serde_json::json!({"path": "/tmp/test"}),
            )
            .await;
        assert!(result.is_denied());

        // Check permission for read - should ask (default behavior)
        let result = gateway
            .check_permission("filesystem_read", &serde_json::json!({"path": "/tmp/test"}))
            .await;
        assert!(!result.is_denied()); // Read is not denied in Plan mode
    }

    #[tokio::test]
    async fn test_parse_permission_request() {
        let error = Error::PermissionDenied(
            "PERMISSION_REQUIRED:test_tool:Allow test_tool to execute?:{\"key\":\"value\"}"
                .to_string(),
        );

        let request = McpGateway::parse_permission_request(&error);
        assert!(request.is_some());

        let request = request.unwrap();
        assert_eq!(request.tool_name, "test_tool");
        assert_eq!(request.question, "Allow test_tool to execute?");
        assert_eq!(request.arguments, serde_json::json!({"key": "value"}));
    }

    #[tokio::test]
    async fn test_parse_permission_request_not_permission() {
        let error = Error::NotFound("Something not found".to_string());
        let request = McpGateway::parse_permission_request(&error);
        assert!(request.is_none());
    }

    #[tokio::test]
    async fn test_builder_with_permission_mode() {
        let gateway = McpGatewayBuilder::new()
            .with_permission_mode(PermissionMode::AcceptEdits)
            .await
            .build();

        let mode = gateway.get_permission_mode().await;
        assert_eq!(mode, PermissionMode::AcceptEdits);
    }

    #[tokio::test]
    async fn test_builder_with_permission_rule() {
        let gateway = McpGatewayBuilder::new()
            .with_permission_mode(PermissionMode::Default)
            .await
            .with_permission_rule(PermissionRule::tool("*_bash"), PermissionBehavior::Deny)
            .await
            .build();

        // Check that bash is denied
        let result = gateway
            .check_permission("executor_bash", &serde_json::json!({"command": "ls"}))
            .await;
        assert!(result.is_denied());
    }
}
