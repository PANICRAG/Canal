//! Unified Tool Registry
//!
//! Single registry for all tool sources (agent, MCP builtin, MCP external).
//! Provides primary HashMap lookup plus secondary indexes by namespace and source.

use std::collections::HashMap;

use super::types::{ToolEntry, ToolId, ToolSource};

/// Unified registry for all tools regardless of source
pub struct UnifiedToolRegistry {
    /// Primary store: "namespace.name" -> ToolEntry
    tools: HashMap<String, ToolEntry>,
    /// Secondary index: namespace -> [registry_keys]
    index_by_namespace: HashMap<String, Vec<String>>,
    /// Secondary index: source_key -> [registry_keys]
    index_by_source: HashMap<String, Vec<String>>,
}

impl UnifiedToolRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            index_by_namespace: HashMap::new(),
            index_by_source: HashMap::new(),
        }
    }

    /// Register a tool. Replaces any existing tool with the same ID.
    pub fn register(&mut self, entry: ToolEntry) {
        let key = entry.id.registry_key();

        // Remove old entry from indexes if replacing
        if self.tools.contains_key(&key) {
            self.remove_from_indexes(&key);
        }

        // Add to secondary indexes
        self.index_by_namespace
            .entry(entry.id.namespace.clone())
            .or_default()
            .push(key.clone());

        self.index_by_source
            .entry(entry.source.index_key())
            .or_default()
            .push(key.clone());

        self.tools.insert(key, entry);
    }

    /// Unregister a tool by its ID
    pub fn unregister(&mut self, id: &ToolId) {
        let key = id.registry_key();
        if self.tools.remove(&key).is_some() {
            self.remove_from_indexes(&key);
        }
    }

    /// Get a tool by its ID
    pub fn get(&self, id: &ToolId) -> Option<&ToolEntry> {
        self.tools.get(&id.registry_key())
    }

    /// Look up a tool by LLM name (e.g. "Read" or "filesystem_read_file").
    ///
    /// Tries agent namespace first, then parses as namespace_name format.
    pub fn get_by_llm_name(&self, name: &str) -> Option<&ToolEntry> {
        // 1. Try as agent tool first (simple name like "Read")
        let agent_key = ToolId::agent(name).registry_key();
        if let Some(entry) = self.tools.get(&agent_key) {
            return Some(entry);
        }

        // 2. Try parsing as namespace_name format
        if let Some(id) = ToolId::from_llm_name(name) {
            return self.tools.get(&id.registry_key());
        }

        None
    }

    /// List all registered tools
    pub fn list(&self) -> Vec<&ToolEntry> {
        self.tools.values().collect()
    }

    /// List tools in a specific namespace
    pub fn list_by_namespace(&self, namespace: &str) -> Vec<&ToolEntry> {
        self.index_by_namespace
            .get(namespace)
            .map(|keys| keys.iter().filter_map(|k| self.tools.get(k)).collect())
            .unwrap_or_default()
    }

    /// List tools from a specific source
    pub fn list_by_source(&self, source: &ToolSource) -> Vec<&ToolEntry> {
        self.index_by_source
            .get(&source.index_key())
            .map(|keys| keys.iter().filter_map(|k| self.tools.get(k)).collect())
            .unwrap_or_default()
    }

    /// List tools filtered by enabled namespaces
    pub fn list_filtered(&self, enabled_namespaces: &[String]) -> Vec<&ToolEntry> {
        self.tools
            .values()
            .filter(|e| enabled_namespaces.contains(&e.id.namespace))
            .collect()
    }

    /// Remove all tools in a namespace
    pub fn clear_namespace(&mut self, namespace: &str) {
        if let Some(keys) = self.index_by_namespace.remove(namespace) {
            for key in &keys {
                if let Some(entry) = self.tools.remove(key) {
                    // Remove from source index
                    let source_key = entry.source.index_key();
                    if let Some(source_keys) = self.index_by_source.get_mut(&source_key) {
                        source_keys.retain(|k| k != key);
                        if source_keys.is_empty() {
                            self.index_by_source.remove(&source_key);
                        }
                    }
                }
            }
        }
    }

    /// Remove all tools from a specific MCP server (by server_name).
    ///
    /// This removes tools whose source is `McpExternal { server_name }`.
    pub fn unregister_by_source(&mut self, server_name: &str) {
        let source_key = ToolSource::McpExternal {
            server_name: server_name.to_string(),
        }
        .index_key();

        if let Some(keys) = self.index_by_source.remove(&source_key) {
            for key in &keys {
                if let Some(entry) = self.tools.remove(key) {
                    // Remove from namespace index
                    if let Some(ns_keys) = self.index_by_namespace.get_mut(&entry.id.namespace) {
                        ns_keys.retain(|k| k != key);
                        if ns_keys.is_empty() {
                            self.index_by_namespace.remove(&entry.id.namespace);
                        }
                    }
                }
            }
        }
    }

    /// Number of registered tools
    pub fn count(&self) -> usize {
        self.tools.len()
    }

    /// Check if a tool is registered
    pub fn contains(&self, id: &ToolId) -> bool {
        self.tools.contains_key(&id.registry_key())
    }

    /// Remove a key from all secondary indexes
    fn remove_from_indexes(&mut self, key: &str) {
        // Remove from namespace index
        for keys in self.index_by_namespace.values_mut() {
            keys.retain(|k| k != key);
        }
        // Clean up empty entries
        self.index_by_namespace.retain(|_, v| !v.is_empty());

        // Remove from source index
        for keys in self.index_by_source.values_mut() {
            keys.retain(|k| k != key);
        }
        self.index_by_source.retain(|_, v| !v.is_empty());
    }
}

impl Default for UnifiedToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
