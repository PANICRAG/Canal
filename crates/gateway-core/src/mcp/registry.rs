//! Tool Registry

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub namespace: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Tool Registry
///
/// Maintains a registry of available tools from all connected MCP servers.
pub struct ToolRegistry {
    tools: HashMap<String, Tool>, // key: "namespace.tool_name"
}

impl ToolRegistry {
    /// Create a new tool registry
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool
    pub fn register(&mut self, tool: Tool) {
        let key = format!("{}.{}", tool.namespace, tool.name);
        tracing::debug!(tool = %key, "Registering tool");
        self.tools.insert(key, tool);
    }

    /// Unregister a tool
    pub fn unregister(&mut self, namespace: &str, name: &str) {
        let key = format!("{}.{}", namespace, name);
        tracing::debug!(tool = %key, "Unregistering tool");
        self.tools.remove(&key);
    }

    /// Clear all tools for a namespace
    pub fn clear_namespace(&mut self, namespace: &str) {
        tracing::debug!(namespace = %namespace, "Clearing namespace tools");
        self.tools.retain(|_, t| t.namespace != namespace);
    }

    /// Get a tool by namespace and name
    pub fn get_tool(&self, namespace: &str, name: &str) -> Option<&Tool> {
        let key = format!("{}.{}", namespace, name);
        self.tools.get(&key)
    }

    /// List all tools
    pub fn list_tools(&self) -> Vec<Tool> {
        self.tools.values().cloned().collect()
    }

    /// List tools by namespace
    pub fn list_by_namespace(&self, namespace: &str) -> Vec<Tool> {
        self.tools
            .values()
            .filter(|t| t.namespace == namespace)
            .cloned()
            .collect()
    }

    /// Get the count of registered tools
    pub fn count(&self) -> usize {
        self.tools.len()
    }

    /// Check if a tool exists
    pub fn contains(&self, namespace: &str, name: &str) -> bool {
        let key = format!("{}.{}", namespace, name);
        self.tools.contains_key(&key)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_tool() {
        let mut registry = ToolRegistry::new();

        let tool = Tool {
            name: "test_tool".to_string(),
            namespace: "test".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({}),
        };

        registry.register(tool);

        assert_eq!(registry.count(), 1);
        assert!(registry.contains("test", "test_tool"));
    }

    #[test]
    fn test_get_tool() {
        let mut registry = ToolRegistry::new();

        let tool = Tool {
            name: "test_tool".to_string(),
            namespace: "test".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({}),
        };

        registry.register(tool);

        let retrieved = registry.get_tool("test", "test_tool");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "test_tool");
    }

    #[test]
    fn test_list_by_namespace() {
        let mut registry = ToolRegistry::new();

        registry.register(Tool {
            name: "tool1".to_string(),
            namespace: "ns1".to_string(),
            description: "Tool 1".to_string(),
            input_schema: serde_json::json!({}),
        });

        registry.register(Tool {
            name: "tool2".to_string(),
            namespace: "ns1".to_string(),
            description: "Tool 2".to_string(),
            input_schema: serde_json::json!({}),
        });

        registry.register(Tool {
            name: "tool3".to_string(),
            namespace: "ns2".to_string(),
            description: "Tool 3".to_string(),
            input_schema: serde_json::json!({}),
        });

        let ns1_tools = registry.list_by_namespace("ns1");
        assert_eq!(ns1_tools.len(), 2);

        let ns2_tools = registry.list_by_namespace("ns2");
        assert_eq!(ns2_tools.len(), 1);
    }

    #[test]
    fn test_clear_namespace() {
        let mut registry = ToolRegistry::new();

        registry.register(Tool {
            name: "tool1".to_string(),
            namespace: "ns1".to_string(),
            description: "Tool 1".to_string(),
            input_schema: serde_json::json!({}),
        });

        registry.register(Tool {
            name: "tool2".to_string(),
            namespace: "ns2".to_string(),
            description: "Tool 2".to_string(),
            input_schema: serde_json::json!({}),
        });

        registry.clear_namespace("ns1");

        assert_eq!(registry.count(), 1);
        assert!(!registry.contains("ns1", "tool1"));
        assert!(registry.contains("ns2", "tool2"));
    }
}
