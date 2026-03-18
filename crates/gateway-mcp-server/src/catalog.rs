//! Tool catalog for the MCP server
//!
//! Manages tool registrations across namespaces with scope-based filtering.

use dashmap::DashMap;
use canal_identity::types::AgentScope;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A single tool definition in the catalog
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogTool {
    /// Full namespaced name (e.g., "engine.chat")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for input
    pub input_schema: serde_json::Value,
    /// Namespace this tool belongs to
    pub namespace: String,
    /// Required scope to access this tool
    pub required_scope: AgentScope,
}

/// Tool catalog — manages tool registrations across namespaces
pub struct ToolCatalog {
    /// All registered tools, keyed by full name
    tools: DashMap<String, CatalogTool>,
}

impl ToolCatalog {
    pub fn new() -> Self {
        Self {
            tools: DashMap::new(),
        }
    }

    /// Register a tool in the catalog
    pub fn register(&self, tool: CatalogTool) {
        self.tools.insert(tool.name.clone(), tool);
    }

    /// Register multiple tools at once
    pub fn register_many(&self, tools: Vec<CatalogTool>) {
        for tool in tools {
            self.register(tool);
        }
    }

    /// List all tools visible to the given scopes
    pub fn list_for_scopes(&self, scopes: &[AgentScope]) -> Vec<CatalogTool> {
        self.tools
            .iter()
            .filter(|entry| scopes.contains(&entry.value().required_scope))
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// List all tools (no scope filtering)
    pub fn list_all(&self) -> Vec<CatalogTool> {
        self.tools
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get a tool by full name
    pub fn get(&self, name: &str) -> Option<CatalogTool> {
        self.tools.get(name).map(|entry| entry.value().clone())
    }

    /// Remove a tool by name
    pub fn remove(&self, name: &str) -> Option<CatalogTool> {
        self.tools.remove(name).map(|(_, tool)| tool)
    }

    /// Number of registered tools
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Check if catalog is empty
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Extract namespace from a tool name (e.g., "engine.chat" → "engine")
    pub fn extract_namespace(tool_name: &str) -> Option<&str> {
        tool_name.split('.').next()
    }
}

impl Default for ToolCatalog {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the default tool catalog with all built-in tools
pub fn build_default_catalog() -> Arc<ToolCatalog> {
    let catalog = ToolCatalog::new();

    // Engine namespace — AI chat and streaming
    catalog.register_many(vec![
        CatalogTool {
            name: "engine.chat".to_string(),
            description: "Send a chat message to an LLM through the AI Gateway".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "messages": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "role": { "type": "string", "enum": ["user", "assistant", "system"] },
                                "content": { "type": "string" }
                            },
                            "required": ["role", "content"]
                        },
                        "description": "Messages to send"
                    },
                    "model": {
                        "type": "string",
                        "description": "Model to use (e.g., 'claude-sonnet-4-5-20250929', 'gpt-4o')"
                    },
                    "max_tokens": {
                        "type": "integer",
                        "description": "Maximum tokens in response"
                    }
                },
                "required": ["messages"]
            }),
            namespace: "engine".to_string(),
            required_scope: AgentScope::EngineChat,
        },
        CatalogTool {
            name: "engine.models".to_string(),
            description: "List available LLM models".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            namespace: "engine".to_string(),
            required_scope: AgentScope::EngineChat,
        },
        CatalogTool {
            name: "engine.capabilities".to_string(),
            description: "Get full engine capabilities including execution modes, models, and feature flags".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            namespace: "engine".to_string(),
            required_scope: AgentScope::ToolsRead,
        },
    ]);

    // Workflow namespace — list and execute workflow templates
    catalog.register_many(vec![
        CatalogTool {
            name: "workflow.list_templates".to_string(),
            description: "List available workflow templates (e.g., Simple, PlanExecute, Research)"
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            namespace: "workflow".to_string(),
            required_scope: AgentScope::ToolsRead,
        },
        CatalogTool {
            name: "workflow.execute".to_string(),
            description: "Execute a workflow by template name with given input".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "template": {
                        "type": "string",
                        "description": "Template name (e.g., 'simple', 'plan_execute', 'research')"
                    },
                    "input": {
                        "type": "string",
                        "description": "Input text/query for the workflow"
                    },
                    "model": {
                        "type": "string",
                        "description": "Model to use for LLM calls"
                    },
                    "max_iterations": {
                        "type": "integer",
                        "description": "Maximum iterations for verification loops"
                    }
                },
                "required": ["template", "input"]
            }),
            namespace: "workflow".to_string(),
            required_scope: AgentScope::EngineChat,
        },
    ]);

    // Platform namespace — health, status, info
    catalog.register_many(vec![
        CatalogTool {
            name: "platform.health".to_string(),
            description: "Check platform health status".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            namespace: "platform".to_string(),
            required_scope: AgentScope::ToolsRead,
        },
        CatalogTool {
            name: "platform.info".to_string(),
            description: "Get platform information and version".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            namespace: "platform".to_string(),
            required_scope: AgentScope::ToolsRead,
        },
    ]);

    // Tools namespace — list and call tools from connected MCP servers
    catalog.register_many(vec![
        CatalogTool {
            name: "tools.list".to_string(),
            description: "List all tools available from connected MCP servers".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "namespace": {
                        "type": "string",
                        "description": "Filter by namespace"
                    }
                }
            }),
            namespace: "tools".to_string(),
            required_scope: AgentScope::ToolsRead,
        },
        CatalogTool {
            name: "tools.call".to_string(),
            description: "Call a tool on a connected MCP server".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "server": {
                        "type": "string",
                        "description": "MCP server namespace"
                    },
                    "tool": {
                        "type": "string",
                        "description": "Tool name"
                    },
                    "arguments": {
                        "type": "object",
                        "description": "Arguments to pass to the tool"
                    }
                },
                "required": ["server", "tool"]
            }),
            namespace: "tools".to_string(),
            required_scope: AgentScope::ToolsWrite,
        },
    ]);

    // Billing namespace — usage, balance, and cost estimation
    catalog.register_many(vec![
        CatalogTool {
            name: "billing.usage".to_string(),
            description: "Get current usage summary and recent call records".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            namespace: "billing".to_string(),
            required_scope: AgentScope::ToolsRead,
        },
        CatalogTool {
            name: "billing.balance".to_string(),
            description: "Get current balance and remaining budget".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            namespace: "billing".to_string(),
            required_scope: AgentScope::ToolsRead,
        },
        CatalogTool {
            name: "billing.estimate".to_string(),
            description: "Estimate cost of a tool call before execution".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "tool_name": {
                        "type": "string",
                        "description": "Tool name to estimate cost for (e.g., 'engine.chat')"
                    }
                },
                "required": ["tool_name"]
            }),
            namespace: "billing".to_string(),
            required_scope: AgentScope::ToolsRead,
        },
    ]);

    // Runtime namespace — sandboxed code execution + browser automation
    {
        let runtime_tools = canal_runtime::tools::runtime_tools();
        let runtime_catalog_tools: Vec<CatalogTool> = runtime_tools
            .into_iter()
            .map(|t| CatalogTool {
                name: t.name.clone(),
                description: t.description,
                input_schema: t.input_schema,
                namespace: "runtime".to_string(),
                required_scope: if t.name.starts_with("runtime.browser") {
                    AgentScope::BrowserControl
                } else {
                    AgentScope::RuntimeExecute
                },
            })
            .collect();
        catalog.register_many(runtime_catalog_tools);
    }

    // Control namespace — platform instance management (15 tools)
    register_control_tools(&catalog);

    // MCP proxy namespace — proxy to external MCP servers
    catalog.register_many(vec![
        CatalogTool {
            name: "mcp.servers".to_string(),
            description: "List connected MCP servers".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            namespace: "mcp".to_string(),
            required_scope: AgentScope::McpProxy,
        },
        CatalogTool {
            name: "mcp.call".to_string(),
            description: "Proxy a tool call to an external MCP server".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "server": {
                        "type": "string",
                        "description": "External MCP server name"
                    },
                    "method": {
                        "type": "string",
                        "description": "MCP method to call"
                    },
                    "params": {
                        "type": "object",
                        "description": "Method parameters"
                    }
                },
                "required": ["server", "method"]
            }),
            namespace: "mcp".to_string(),
            required_scope: AgentScope::McpProxy,
        },
    ]);

    Arc::new(catalog)
}

/// Register all control.* tools for platform instance management.
fn register_control_tools(catalog: &ToolCatalog) {
    catalog.register_many(vec![
        // Tenant (3)
        CatalogTool {
            name: "control.create_tenant".to_string(),
            description: "Create a platform tenant for the current user".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Tenant name" },
                    "plan_id": { "type": "string", "description": "Billing plan (free, pro, team, enterprise)", "default": "free" }
                },
                "required": ["name"]
            }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        CatalogTool {
            name: "control.get_tenant".to_string(),
            description: "Get tenant info for the current user".to_string(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        CatalogTool {
            name: "control.list_tenants".to_string(),
            description: "List all platform tenants (admin only)".to_string(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        // Instance Lifecycle (7)
        CatalogTool {
            name: "control.create_instance".to_string(),
            description: "Provision a new canal-server instance".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Instance name" },
                    "modules": { "type": "array", "items": { "type": "string" }, "default": ["engine", "session"] },
                    "memory_limit_mb": { "type": "integer", "default": 512 },
                    "cpu_limit": { "type": "number", "default": 1.0 }
                },
                "required": ["name"]
            }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        CatalogTool {
            name: "control.start_instance".to_string(),
            description: "Start a stopped instance".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "instance_id": { "type": "string", "description": "Instance UUID" }
                },
                "required": ["instance_id"]
            }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        CatalogTool {
            name: "control.stop_instance".to_string(),
            description: "Stop a running instance".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "instance_id": { "type": "string", "description": "Instance UUID" }
                },
                "required": ["instance_id"]
            }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        CatalogTool {
            name: "control.restart_instance".to_string(),
            description: "Restart an instance".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "instance_id": { "type": "string", "description": "Instance UUID" }
                },
                "required": ["instance_id"]
            }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        CatalogTool {
            name: "control.destroy_instance".to_string(),
            description: "Permanently delete an instance".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "instance_id": { "type": "string", "description": "Instance UUID" }
                },
                "required": ["instance_id"]
            }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        CatalogTool {
            name: "control.list_instances".to_string(),
            description: "List instances for the current tenant".to_string(),
            input_schema: serde_json::json!({ "type": "object", "properties": {} }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        CatalogTool {
            name: "control.get_instance".to_string(),
            description: "Get instance details including health and metrics".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "instance_id": { "type": "string", "description": "Instance UUID" }
                },
                "required": ["instance_id"]
            }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        // Instance Interaction (3)
        CatalogTool {
            name: "control.proxy_chat".to_string(),
            description: "Send a chat message to a specific instance".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "instance_id": { "type": "string", "description": "Target instance UUID" },
                    "message": { "type": "string", "description": "Chat message to send" },
                    "model": { "type": "string", "description": "Model override (optional)" }
                },
                "required": ["instance_id", "message"]
            }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        CatalogTool {
            name: "control.get_logs".to_string(),
            description: "Get instance logs".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "instance_id": { "type": "string", "description": "Instance UUID" },
                    "tail": { "type": "integer", "description": "Number of lines", "default": 100 }
                },
                "required": ["instance_id"]
            }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        CatalogTool {
            name: "control.check_health".to_string(),
            description: "Health check a specific instance".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "instance_id": { "type": "string", "description": "Instance UUID" }
                },
                "required": ["instance_id"]
            }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        // Cross-Instance (2)
        CatalogTool {
            name: "control.compare_instances".to_string(),
            description: "Compare metrics across multiple instances".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "instance_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of instance UUIDs to compare"
                    }
                },
                "required": ["instance_ids"]
            }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
        CatalogTool {
            name: "control.migrate_config".to_string(),
            description: "Copy configuration from one instance to another".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "source_id": { "type": "string", "description": "Source instance UUID" },
                    "target_id": { "type": "string", "description": "Target instance UUID" }
                },
                "required": ["source_id", "target_id"]
            }),
            namespace: "control".to_string(),
            required_scope: AgentScope::Admin,
        },
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_register_and_get() {
        let catalog = ToolCatalog::new();
        catalog.register(CatalogTool {
            name: "test.tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            namespace: "test".to_string(),
            required_scope: AgentScope::ToolsRead,
        });

        assert_eq!(catalog.len(), 1);
        let tool = catalog.get("test.tool").unwrap();
        assert_eq!(tool.description, "A test tool");
    }

    #[test]
    fn test_catalog_scope_filtering() {
        let catalog = ToolCatalog::new();
        catalog.register(CatalogTool {
            name: "engine.chat".to_string(),
            description: "Chat".to_string(),
            input_schema: serde_json::json!({}),
            namespace: "engine".to_string(),
            required_scope: AgentScope::EngineChat,
        });
        catalog.register(CatalogTool {
            name: "admin.config".to_string(),
            description: "Config".to_string(),
            input_schema: serde_json::json!({}),
            namespace: "admin".to_string(),
            required_scope: AgentScope::Admin,
        });

        // Only EngineChat scope — should see 1 tool
        let visible = catalog.list_for_scopes(&[AgentScope::EngineChat]);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].name, "engine.chat");

        // Both scopes — should see 2 tools
        let visible = catalog.list_for_scopes(&[AgentScope::EngineChat, AgentScope::Admin]);
        assert_eq!(visible.len(), 2);
    }

    #[test]
    fn test_catalog_remove() {
        let catalog = ToolCatalog::new();
        catalog.register(CatalogTool {
            name: "test.tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({}),
            namespace: "test".to_string(),
            required_scope: AgentScope::ToolsRead,
        });

        assert_eq!(catalog.len(), 1);
        let removed = catalog.remove("test.tool");
        assert!(removed.is_some());
        assert_eq!(catalog.len(), 0);
    }

    #[test]
    fn test_extract_namespace() {
        assert_eq!(
            ToolCatalog::extract_namespace("engine.chat"),
            Some("engine")
        );
        assert_eq!(
            ToolCatalog::extract_namespace("platform.health"),
            Some("platform")
        );
        assert_eq!(
            ToolCatalog::extract_namespace("standalone"),
            Some("standalone")
        );
    }

    #[test]
    fn test_default_catalog() {
        let catalog = build_default_catalog();
        assert!(catalog.len() >= 8);

        // Verify engine.chat exists
        let chat = catalog.get("engine.chat");
        assert!(chat.is_some());
        assert_eq!(chat.unwrap().namespace, "engine");
    }
}
