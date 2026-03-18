//! Hook System - Claude Agent SDK Compatible
//!
//! Provides hook execution, matching, and lifecycle management.

pub mod executor;
pub mod iteration;
pub mod matcher;
pub mod shell;

pub use executor::HookExecutor;
pub use iteration::{IterationConfig, IterationHook};
pub use matcher::HookMatcher;
pub use shell::ShellHookRunner;

use crate::agent::types::{HookContext, HookEvent, HookResult};
use async_trait::async_trait;
use std::sync::Arc;

/// Hook callback trait for custom hook implementations
#[async_trait]
pub trait HookCallback: Send + Sync {
    /// Execute the hook callback
    async fn on_event(
        &self,
        event: HookEvent,
        data: serde_json::Value,
        context: &HookContext,
    ) -> HookResult;

    /// Get the name of this hook (for debugging)
    fn name(&self) -> &str {
        "anonymous"
    }

    /// Check if this hook handles a specific event
    fn handles_event(&self, event: HookEvent) -> bool;
}

/// A registered hook with its configuration
#[derive(Clone)]
pub struct RegisteredHook {
    /// The hook callback
    pub callback: Arc<dyn HookCallback>,
    /// Events this hook listens to
    pub events: Vec<HookEvent>,
    /// Tool name filter (glob pattern)
    pub tool_filter: Option<String>,
    /// Priority (higher = runs first)
    pub priority: i32,
    /// Whether this hook is enabled
    pub enabled: bool,
    /// Timeout in milliseconds
    pub timeout_ms: u64,
}

impl RegisteredHook {
    /// Create a new registered hook
    pub fn new(callback: Arc<dyn HookCallback>, events: Vec<HookEvent>) -> Self {
        Self {
            callback,
            events,
            tool_filter: None,
            priority: 0,
            enabled: true,
            timeout_ms: 60000,
        }
    }

    /// Set tool filter
    pub fn with_tool_filter(mut self, filter: impl Into<String>) -> Self {
        self.tool_filter = Some(filter.into());
        self
    }

    /// Set priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }
}

/// Hook execution output
#[derive(Debug, Clone)]
pub struct HookOutput {
    /// The hook name
    pub hook_name: String,
    /// The result
    pub result: HookResult,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
    /// Error message if hook failed
    pub error: Option<String>,
}

impl HookOutput {
    /// Create a successful output
    pub fn success(hook_name: impl Into<String>, result: HookResult, duration_ms: u64) -> Self {
        Self {
            hook_name: hook_name.into(),
            result,
            duration_ms,
            error: None,
        }
    }

    /// Create an error output
    pub fn error(hook_name: impl Into<String>, error: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            hook_name: hook_name.into(),
            result: HookResult::continue_(),
            duration_ms,
            error: Some(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHook {
        name: String,
    }

    #[async_trait]
    impl HookCallback for TestHook {
        async fn on_event(
            &self,
            _event: HookEvent,
            _data: serde_json::Value,
            _context: &HookContext,
        ) -> HookResult {
            HookResult::continue_()
        }

        fn name(&self) -> &str {
            &self.name
        }

        fn handles_event(&self, event: HookEvent) -> bool {
            matches!(event, HookEvent::PreToolUse | HookEvent::PostToolUse)
        }
    }

    #[test]
    fn test_registered_hook_builder() {
        let hook = TestHook {
            name: "test".to_string(),
        };
        let registered = RegisteredHook::new(Arc::new(hook), vec![HookEvent::PreToolUse])
            .with_tool_filter("Bash*")
            .with_priority(10)
            .with_timeout(5000);

        assert_eq!(registered.tool_filter, Some("Bash*".to_string()));
        assert_eq!(registered.priority, 10);
        assert_eq!(registered.timeout_ms, 5000);
    }
}
