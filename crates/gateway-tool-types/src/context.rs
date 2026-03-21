//! Tool execution context — portable base type without gateway-core dependencies.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Permission mode for the agent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// Default mode — ask for permission on potentially dangerous operations
    Default,
    /// Accept all file edits automatically
    AcceptEdits,
    /// Plan mode — read-only, no modifications
    Plan,
    /// Auto-approve all tool calls for uninterrupted agent execution
    #[default]
    BypassPermissions,
}

impl PermissionMode {
    /// Check if this mode allows automatic tool execution
    pub fn auto_approve(&self) -> bool {
        matches!(self, Self::BypassPermissions)
    }

    /// Check if this mode allows file edits
    pub fn allows_edits(&self) -> bool {
        !matches!(self, Self::Plan)
    }

    /// Check if this mode should auto-approve edits
    pub fn auto_approve_edits(&self) -> bool {
        matches!(self, Self::AcceptEdits | Self::BypassPermissions)
    }
}

/// Context passed to tool execution.
///
/// This is the portable base context that can be used by both gateway-core
/// and gateway-tools without circular dependencies. Gateway-core wraps this
/// in `AgentToolContext` which adds `HookExecutor`.
#[derive(Clone)]
pub struct ToolContext {
    /// Session ID
    pub session_id: String,
    /// Current working directory
    pub cwd: PathBuf,
    /// Permission mode
    pub permission_mode: PermissionMode,
    /// Allowed directories for file operations
    pub allowed_directories: Vec<PathBuf>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// User ID (if authenticated)
    pub user_id: Option<String>,
    /// Maximum execution timeout in seconds
    pub timeout_secs: u64,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            cwd: PathBuf::from("."),
            permission_mode: PermissionMode::BypassPermissions,
            allowed_directories: Vec::new(),
            env: HashMap::new(),
            user_id: None,
            timeout_secs: 120,
            metadata: HashMap::new(),
        }
    }
}

impl ToolContext {
    /// Create a new tool context
    pub fn new(session_id: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            session_id: session_id.into(),
            cwd: cwd.into(),
            ..Default::default()
        }
    }

    /// Set permission mode
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        self
    }

    /// Add allowed directory
    pub fn with_allowed_directory(mut self, dir: impl Into<PathBuf>) -> Self {
        self.allowed_directories.push(dir.into());
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Set user ID
    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Check if a path is within allowed directories
    pub fn is_path_allowed(&self, path: &std::path::Path) -> bool {
        if self.allowed_directories.is_empty() {
            return path.starts_with(&self.cwd);
        }
        self.allowed_directories
            .iter()
            .any(|allowed| path.starts_with(allowed))
    }

    /// Resolve a path relative to cwd
    pub fn resolve_path(&self, path: &str) -> PathBuf {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            path
        } else {
            self.cwd.join(path)
        }
    }

    /// Check if this context allows mutations
    pub fn allows_mutations(&self) -> bool {
        self.permission_mode.allows_edits()
    }

    /// Check if this context auto-approves edits
    pub fn auto_approves_edits(&self) -> bool {
        self.permission_mode.auto_approve_edits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_default() {
        let ctx = ToolContext::default();
        assert!(ctx.session_id.is_empty());
        assert_eq!(ctx.permission_mode, PermissionMode::BypassPermissions);
    }

    #[test]
    fn test_context_builder() {
        let ctx = ToolContext::new("session-1", "/tmp/workspace")
            .with_permission_mode(PermissionMode::AcceptEdits)
            .with_timeout(60)
            .with_user_id("user-1")
            .with_allowed_directory("/tmp");

        assert_eq!(ctx.session_id, "session-1");
        assert_eq!(ctx.cwd, PathBuf::from("/tmp/workspace"));
        assert_eq!(ctx.permission_mode, PermissionMode::AcceptEdits);
        assert_eq!(ctx.timeout_secs, 60);
        assert_eq!(ctx.user_id, Some("user-1".to_string()));
    }

    #[test]
    fn test_path_allowed() {
        let ctx = ToolContext::new("s1", "/home/user/project")
            .with_allowed_directory("/home/user/project")
            .with_allowed_directory("/tmp");

        assert!(ctx.is_path_allowed(std::path::Path::new("/home/user/project/src/main.rs")));
        assert!(ctx.is_path_allowed(std::path::Path::new("/tmp/test.txt")));
        assert!(!ctx.is_path_allowed(std::path::Path::new("/etc/passwd")));
    }

    #[test]
    fn test_resolve_path() {
        let ctx = ToolContext::new("s1", "/home/user/project");

        assert_eq!(
            ctx.resolve_path("src/main.rs"),
            PathBuf::from("/home/user/project/src/main.rs")
        );
        assert_eq!(
            ctx.resolve_path("/absolute/path"),
            PathBuf::from("/absolute/path")
        );
    }

    #[test]
    fn test_permission_checks() {
        let ctx_default =
            ToolContext::new("s1", "/tmp").with_permission_mode(PermissionMode::Default);
        assert!(ctx_default.allows_mutations());
        assert!(!ctx_default.auto_approves_edits());

        let ctx_plan = ToolContext::new("s1", "/tmp").with_permission_mode(PermissionMode::Plan);
        assert!(!ctx_plan.allows_mutations());

        let ctx_accept =
            ToolContext::new("s1", "/tmp").with_permission_mode(PermissionMode::AcceptEdits);
        assert!(ctx_accept.allows_mutations());
        assert!(ctx_accept.auto_approves_edits());
    }

    #[test]
    fn test_permission_mode_serde_roundtrip() {
        let mode = PermissionMode::AcceptEdits;
        let json = serde_json::to_string(&mode).unwrap();
        let deserialized: PermissionMode = serde_json::from_str(&json).unwrap();
        assert_eq!(mode, deserialized);
    }
}
