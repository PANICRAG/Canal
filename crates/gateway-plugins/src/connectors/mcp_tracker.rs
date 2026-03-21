//! MCP reference tracker — ref-counts MCP servers shared across bundles.
//!
//! When multiple bundles reference the same MCP server (e.g., "slack"),
//! the tracker ensures the server is only connected once and only
//! disconnected when the last bundle referencing it is deactivated.

use dashmap::DashMap;
use std::collections::HashSet;

/// Thread-safe reference counter for shared MCP servers across bundles.
///
/// Uses `DashMap` for lock-free concurrent access. Each server name maps
/// to the set of bundle names that reference it.
pub struct McpRefTracker {
    /// server_name → set of bundle_names referencing it
    server_refs: DashMap<String, HashSet<String>>,
}

impl McpRefTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self {
            server_refs: DashMap::new(),
        }
    }

    /// Add a bundle's reference to a server.
    ///
    /// Returns `true` if this is the **first** reference (server needs connecting).
    pub fn add_reference(&self, server: &str, bundle: &str) -> bool {
        let mut entry = self.server_refs.entry(server.to_string()).or_default();
        let is_first = entry.is_empty();
        entry.insert(bundle.to_string());
        is_first
    }

    /// Remove all references from a bundle.
    ///
    /// Returns the list of orphaned servers (ref_count dropped to 0).
    pub fn remove_bundle(&self, bundle: &str) -> Vec<String> {
        let mut orphaned = Vec::new();

        // Collect keys first to avoid holding refs during mutation
        let keys: Vec<String> = self
            .server_refs
            .iter()
            .filter(|entry| entry.value().contains(bundle))
            .map(|entry| entry.key().clone())
            .collect();

        for key in keys {
            if let Some(mut entry) = self.server_refs.get_mut(&key) {
                entry.remove(bundle);
                if entry.is_empty() {
                    orphaned.push(key.clone());
                }
            }
        }

        // Remove empty entries
        for server in &orphaned {
            self.server_refs.remove(server);
        }

        orphaned
    }

    /// Rebuild from active bundle data (for startup reconnect).
    ///
    /// Takes a list of `(bundle_name, Vec<McpServerDef>)` pairs and
    /// populates the tracker accordingly.
    pub fn rebuild(&self, bundles: &[(String, Vec<String>)]) {
        self.server_refs.clear();
        for (bundle_name, server_names) in bundles {
            for server_name in server_names {
                self.server_refs
                    .entry(server_name.clone())
                    .or_default()
                    .insert(bundle_name.clone());
            }
        }
    }

    /// Get all servers referenced by any bundle.
    pub fn all_servers(&self) -> Vec<String> {
        self.server_refs.iter().map(|e| e.key().clone()).collect()
    }

    /// Get reference count for a server.
    pub fn ref_count(&self, server: &str) -> usize {
        self.server_refs.get(server).map(|e| e.len()).unwrap_or(0)
    }

    /// Get all servers for a specific bundle.
    pub fn servers_for_bundle(&self, bundle: &str) -> Vec<String> {
        self.server_refs
            .iter()
            .filter(|entry| entry.value().contains(bundle))
            .map(|entry| entry.key().clone())
            .collect()
    }
}

impl Default for McpRefTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_reference_first_returns_true() {
        let tracker = McpRefTracker::new();
        assert!(tracker.add_reference("slack", "productivity"));
        assert_eq!(tracker.ref_count("slack"), 1);
    }

    #[test]
    fn test_add_reference_subsequent_returns_false() {
        let tracker = McpRefTracker::new();
        assert!(tracker.add_reference("slack", "productivity"));
        assert!(!tracker.add_reference("slack", "sales"));
        assert_eq!(tracker.ref_count("slack"), 2);
    }

    #[test]
    fn test_remove_bundle_orphans_unique_servers() {
        let tracker = McpRefTracker::new();
        tracker.add_reference("slack", "productivity");
        tracker.add_reference("notion", "productivity");
        tracker.add_reference("slack", "sales");

        let orphaned = tracker.remove_bundle("productivity");

        // slack still referenced by sales, notion is orphaned
        assert_eq!(orphaned, vec!["notion".to_string()]);
        assert_eq!(tracker.ref_count("slack"), 1);
        assert_eq!(tracker.ref_count("notion"), 0);
    }

    #[test]
    fn test_shared_server_survives_single_deactivation() {
        let tracker = McpRefTracker::new();
        tracker.add_reference("slack", "productivity");
        tracker.add_reference("slack", "sales");

        let orphaned = tracker.remove_bundle("productivity");
        assert!(orphaned.is_empty());
        assert_eq!(tracker.ref_count("slack"), 1);

        let orphaned = tracker.remove_bundle("sales");
        assert_eq!(orphaned, vec!["slack".to_string()]);
        assert_eq!(tracker.ref_count("slack"), 0);
    }

    #[test]
    fn test_rebuild_from_active_bundles() {
        let tracker = McpRefTracker::new();

        tracker.rebuild(&[
            (
                "productivity".to_string(),
                vec!["slack".to_string(), "notion".to_string()],
            ),
            (
                "sales".to_string(),
                vec!["slack".to_string(), "hubspot".to_string()],
            ),
        ]);

        assert_eq!(tracker.ref_count("slack"), 2);
        assert_eq!(tracker.ref_count("notion"), 1);
        assert_eq!(tracker.ref_count("hubspot"), 1);
        assert_eq!(tracker.all_servers().len(), 3);
    }

    #[test]
    fn test_servers_for_bundle() {
        let tracker = McpRefTracker::new();
        tracker.add_reference("slack", "productivity");
        tracker.add_reference("notion", "productivity");
        tracker.add_reference("hubspot", "sales");

        let mut servers = tracker.servers_for_bundle("productivity");
        servers.sort();
        assert_eq!(servers, vec!["notion", "slack"]);
    }
}
