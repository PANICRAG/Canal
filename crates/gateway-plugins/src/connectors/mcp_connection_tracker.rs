//! MCP connection status tracker — tracks per-server connection lifecycle.
//!
//! Provides a thread-safe view of which MCP servers are connected,
//! connecting, failed, or disconnected. Used by the bundle API to
//! report server status to the frontend.

use dashmap::DashMap;
use serde::Serialize;

/// Connection status for an MCP server.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", content = "error")]
pub enum McpConnectionStatus {
    /// Config registered, connect not started.
    Pending,
    /// `connect_server()` in progress.
    Connecting,
    /// Successfully connected, tools available.
    Connected,
    /// Connection attempt failed.
    Failed(String),
    /// Intentionally disconnected (deactivation).
    Disconnected,
}

impl McpConnectionStatus {
    /// Returns the status as a simple string label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Failed(_) => "failed",
            Self::Disconnected => "disconnected",
        }
    }

    /// Returns the error message if status is Failed.
    pub fn error_message(&self) -> Option<&str> {
        match self {
            Self::Failed(msg) => Some(msg),
            _ => None,
        }
    }
}

/// Thread-safe tracker for MCP server connection statuses.
pub struct McpConnectionTracker {
    statuses: DashMap<String, McpConnectionStatus>,
}

impl McpConnectionTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self {
            statuses: DashMap::new(),
        }
    }

    /// Set the status for a server.
    pub fn set_status(&self, server: &str, status: McpConnectionStatus) {
        self.statuses.insert(server.to_string(), status);
    }

    /// Get the status for a server (defaults to Disconnected if not tracked).
    pub fn get_status(&self, server: &str) -> McpConnectionStatus {
        self.statuses
            .get(server)
            .map(|e| e.value().clone())
            .unwrap_or(McpConnectionStatus::Disconnected)
    }

    /// Remove a server from tracking.
    pub fn remove(&self, server: &str) {
        self.statuses.remove(server);
    }

    /// Get all tracked server statuses.
    pub fn all_statuses(&self) -> Vec<(String, McpConnectionStatus)> {
        self.statuses
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect()
    }
}

impl Default for McpConnectionTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get_status() {
        let tracker = McpConnectionTracker::new();

        tracker.set_status("slack", McpConnectionStatus::Connecting);
        assert_eq!(tracker.get_status("slack").label(), "connecting");

        tracker.set_status("slack", McpConnectionStatus::Connected);
        assert_eq!(tracker.get_status("slack").label(), "connected");
    }

    #[test]
    fn test_remove_status() {
        let tracker = McpConnectionTracker::new();

        tracker.set_status("slack", McpConnectionStatus::Connected);
        tracker.remove("slack");
        assert_eq!(tracker.get_status("slack").label(), "disconnected");
    }

    #[test]
    fn test_all_statuses() {
        let tracker = McpConnectionTracker::new();

        tracker.set_status("slack", McpConnectionStatus::Connected);
        tracker.set_status("notion", McpConnectionStatus::Failed("timeout".to_string()));

        let all = tracker.all_statuses();
        assert_eq!(all.len(), 2);
    }
}
