//! Core types for the Unified Tool System

use serde::{Deserialize, Serialize};

/// Unique identifier for a tool: namespace + name
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolId {
    pub namespace: String,
    pub name: String,
}

impl ToolId {
    /// Create a new ToolId with explicit namespace and name
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }

    /// Create a ToolId for an agent built-in tool (namespace = "agent")
    pub fn agent(name: impl Into<String>) -> Self {
        Self {
            namespace: "agent".to_string(),
            name: name.into(),
        }
    }

    /// Parse an LLM tool name (e.g. "filesystem_read_file") into a ToolId.
    ///
    /// Returns `None` if the name contains no underscore (agent tool names like "Read").
    /// Uses `splitn(2, '_')` so tool names with multiple underscores parse correctly:
    /// - "mac_get_frontmost_app" -> namespace="mac", name="get_frontmost_app"
    pub fn from_llm_name(llm_name: &str) -> Option<Self> {
        let parts: Vec<&str> = llm_name.splitn(2, '_').collect();
        if parts.len() == 2 {
            Some(Self::new(parts[0], parts[1]))
        } else {
            None
        }
    }

    /// Convert to LLM-facing tool name.
    ///
    /// Agent tools return bare name ("Read"), MCP tools return "namespace_name".
    pub fn llm_name(&self) -> String {
        if self.namespace == "agent" {
            self.name.clone()
        } else {
            format!("{}_{}", self.namespace, self.name)
        }
    }

    /// Registry key in "namespace.name" format
    pub fn registry_key(&self) -> String {
        format!("{}.{}", self.namespace, self.name)
    }
}

impl std::fmt::Display for ToolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.namespace, self.name)
    }
}

/// A registered tool entry in the unified registry
#[derive(Debug, Clone, Serialize)]
pub struct ToolEntry {
    pub id: ToolId,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub source: ToolSource,
    pub meta: ToolMeta,
}

/// Where a tool originates from
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ToolSource {
    /// Agent built-in tool (Read, Write, Bash, etc.)
    Agent,
    /// MCP builtin tool (filesystem, executor, browser, mac, automation)
    McpBuiltin,
    /// External MCP server tool
    McpExternal { server_name: String },
}

impl ToolSource {
    /// Key used for the source index
    pub fn index_key(&self) -> String {
        match self {
            ToolSource::Agent => "agent".to_string(),
            ToolSource::McpBuiltin => "mcp_builtin".to_string(),
            ToolSource::McpExternal { server_name } => format!("mcp_external:{}", server_name),
        }
    }

    /// Human-readable source label for API responses
    pub fn api_label(&self) -> &str {
        match self {
            ToolSource::Agent => "agent_builtin",
            ToolSource::McpBuiltin => "mcp_builtin",
            ToolSource::McpExternal { .. } => "mcp_external",
        }
    }
}

/// Metadata about a tool's transport and location
#[derive(Debug, Clone, Serialize)]
pub struct ToolMeta {
    pub transport_type: String,
    pub location: String,
    pub server_name: String,
}

impl Default for ToolMeta {
    fn default() -> Self {
        Self {
            transport_type: "local".to_string(),
            location: "local".to_string(),
            server_name: String::new(),
        }
    }
}

/// Filter for selecting tools (used by schemas_for_llm)
#[derive(Debug, Default, Clone)]
pub struct ToolFilter {
    /// When set, only include tools from these namespaces
    pub enabled_namespaces: Option<Vec<String>>,
    /// Whether the current task involves browser automation
    pub is_browser_task: bool,
    /// Whether worker orchestration is enabled
    pub workers_enabled: bool,
    /// Whether code orchestration is enabled
    pub code_orchestration_enabled: bool,
}
