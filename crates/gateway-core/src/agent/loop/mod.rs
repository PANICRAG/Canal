//! Agent Loop - Claude Agent SDK Compatible Agentic Loop
//!
//! Implements the core agent loop with tool use, hooks, and permission handling.

pub mod config;
pub mod runner;
pub mod state;

pub use config::{
    AgentConfig, AgentDefinition, AgentModel, CompactionConfig, McpServerConfig,
    SubagentSystemConfig,
};
pub use runner::{AgentRunner, LlmClient, LlmResponse, StopReason, ToolExecutor};
pub use state::AgentState;

use crate::agent::types::{AgentMessage, PermissionMode, ResultMessage, ResultSubtype, Usage};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

/// Agent loop trait - standard interface for agent execution
#[async_trait]
pub trait AgentLoop: Send + Sync {
    /// Run the agent with a prompt, returning a stream of messages.
    ///
    /// The returned stream captures context by move (A41 fix) and does NOT
    /// borrow `self`. Callers may release the agent lock after calling this.
    async fn query(
        &mut self,
        prompt: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<AgentMessage, AgentError>> + Send + 'static>>;

    /// Interrupt the current execution
    async fn interrupt(&mut self) -> Result<(), AgentError>;

    /// Set permission mode
    async fn set_permission_mode(&mut self, mode: PermissionMode) -> Result<(), AgentError>;

    /// Get current session ID
    fn session_id(&self) -> &str;

    /// Get current usage statistics
    fn usage(&self) -> &Usage;

    /// Check if the agent is currently running
    fn is_running(&self) -> bool;
}

/// Agent execution error
#[derive(Debug, Clone)]
pub enum AgentError {
    /// LLM API error
    ApiError(String),
    /// Tool execution error
    ToolError(String),
    /// Permission denied
    PermissionDenied(String),
    /// Timeout
    Timeout(String),
    /// Maximum turns exceeded
    MaxTurnsExceeded(u32),
    /// Maximum budget exceeded
    MaxBudgetExceeded(f64),
    /// Interrupted by user
    Interrupted,
    /// Configuration error
    ConfigError(String),
    /// Session error
    SessionError(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiError(msg) => write!(f, "API error: {}", msg),
            Self::ToolError(msg) => write!(f, "Tool error: {}", msg),
            Self::PermissionDenied(msg) => write!(f, "Permission denied: {}", msg),
            Self::Timeout(msg) => write!(f, "Timeout: {}", msg),
            Self::MaxTurnsExceeded(n) => write!(f, "Maximum turns exceeded: {}", n),
            Self::MaxBudgetExceeded(b) => write!(f, "Maximum budget exceeded: ${:.2}", b),
            Self::Interrupted => write!(f, "Interrupted by user"),
            Self::ConfigError(msg) => write!(f, "Configuration error: {}", msg),
            Self::SessionError(msg) => write!(f, "Session error: {}", msg),
        }
    }
}

impl std::error::Error for AgentError {}

/// Convert AgentError to ResultMessage
impl AgentError {
    pub fn to_result_message(
        &self,
        session_id: &str,
        duration_ms: u64,
        num_turns: u32,
        usage: &Usage,
    ) -> ResultMessage {
        let subtype = match self {
            Self::MaxTurnsExceeded(_) => ResultSubtype::ErrorMaxTurns,
            Self::MaxBudgetExceeded(_) => ResultSubtype::ErrorMaxBudgetUsd,
            Self::Interrupted => ResultSubtype::Interrupted,
            _ => ResultSubtype::ErrorDuringExecution,
        };

        ResultMessage {
            subtype,
            duration_ms,
            duration_api_ms: 0,
            is_error: true,
            num_turns,
            session_id: session_id.to_string(),
            total_cost_usd: None,
            usage: Some(usage.clone()),
            result: Some(self.to_string()),
            structured_output: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_error_display() {
        let err = AgentError::MaxTurnsExceeded(10);
        assert_eq!(err.to_string(), "Maximum turns exceeded: 10");

        let err = AgentError::PermissionDenied("write not allowed".to_string());
        assert_eq!(err.to_string(), "Permission denied: write not allowed");
    }

    #[test]
    fn test_error_to_result_message() {
        let err = AgentError::MaxTurnsExceeded(10);
        let usage = Usage::default();
        let msg = err.to_result_message("session-1", 1000, 10, &usage);

        assert_eq!(msg.subtype, ResultSubtype::ErrorMaxTurns);
        assert!(msg.is_error);
    }
}
