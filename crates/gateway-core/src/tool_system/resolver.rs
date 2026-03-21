//! Tool Resolver
//!
//! Provides schema generation and tool resolution for LLM interactions.

use super::registry::UnifiedToolRegistry;
use super::types::{ToolEntry, ToolFilter, ToolId, ToolSource};

/// Resolves tools from the registry and generates schemas for LLM consumption
pub struct ToolResolver<'a> {
    registry: &'a UnifiedToolRegistry,
}

impl<'a> ToolResolver<'a> {
    /// Create a new resolver referencing a registry
    pub fn new(registry: &'a UnifiedToolRegistry) -> Self {
        Self { registry }
    }

    /// Resolve a tool by its ID
    pub fn resolve(&self, id: &ToolId) -> Option<&ToolEntry> {
        self.registry.get(id)
    }

    /// Resolve a tool by LLM name
    pub fn resolve_llm_name(&self, name: &str) -> Option<&ToolEntry> {
        self.registry.get_by_llm_name(name)
    }

    /// Generate schemas for all tools (no filtering)
    pub fn schemas_all(&self) -> Vec<serde_json::Value> {
        self.registry
            .list()
            .into_iter()
            .map(|e| entry_to_schema(e))
            .collect()
    }

    /// Generate schemas filtered for LLM consumption.
    ///
    /// Applies filtering logic:
    /// - Agent core tools (Read, Write, Edit, Bash, Glob, Grep, Computer) always included
    /// - Browser tools only included when `is_browser_task` is true
    /// - Orchestrate tool only when `workers_enabled`
    /// - CodeOrchestration tool only when `code_orchestration_enabled`
    /// - MCP tools included based on `enabled_namespaces` (None = all)
    pub fn schemas_for_llm(&self, filter: &ToolFilter) -> Vec<serde_json::Value> {
        let core_agent_tools = ["Read", "Write", "Edit", "Bash", "Glob", "Grep", "Computer"];

        self.registry
            .list()
            .into_iter()
            .filter(|entry| {
                match &entry.source {
                    ToolSource::Agent => {
                        let name = entry.id.name.as_str();

                        // Core tools always included
                        if core_agent_tools.contains(&name) {
                            return true;
                        }

                        // Browser/computer use tools only for browser tasks
                        if name.starts_with("computer_")
                            || name.starts_with("browser_")
                            || name == "BrowserTool"
                        {
                            // Exclude computer_click in favor of computer_click_ref
                            if name == "computer_click" {
                                return false;
                            }
                            return filter.is_browser_task;
                        }

                        // Orchestrate tool only when workers enabled
                        if name == "Orchestrate" {
                            return filter.workers_enabled;
                        }

                        // CodeOrchestration tool only when enabled
                        if name == "CodeOrchestration" {
                            return filter.code_orchestration_enabled;
                        }

                        // Other agent tools included by default
                        true
                    }
                    ToolSource::McpBuiltin | ToolSource::McpExternal { .. } => {
                        // Filter by enabled namespaces if set
                        if let Some(ref namespaces) = filter.enabled_namespaces {
                            namespaces.contains(&entry.id.namespace)
                        } else {
                            true
                        }
                    }
                }
            })
            .map(|e| entry_to_schema(e))
            .collect()
    }
}

/// Convert a ToolEntry to the JSON schema format expected by LLMs
fn entry_to_schema(entry: &ToolEntry) -> serde_json::Value {
    serde_json::json!({
        "name": entry.id.llm_name(),
        "description": entry.description,
        "input_schema": entry.input_schema,
    })
}
