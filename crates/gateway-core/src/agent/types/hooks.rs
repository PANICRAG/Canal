//! Hook Types - Claude Agent SDK Compatible
//!
//! Defines hook events and handlers for agent lifecycle.

use serde::{Deserialize, Serialize};

/// Hook event types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// User prompt submitted (after message added to state)
    UserPromptSubmit,
    /// Before tool execution
    PreToolUse,
    /// After tool execution
    PostToolUse,
    /// Before sending message to LLM
    PreMessage,
    /// After receiving message from LLM
    PostMessage,
    /// Session started
    SessionStart,
    /// Session ended
    SessionEnd,
    /// Error occurred
    Error,
    /// Tool execution cancelled
    ToolCancelled,
    /// Permission check
    PermissionCheck,
    /// Subagent spawned
    SubagentSpawn,
    /// Subagent completed
    SubagentComplete,
    /// Memory updated
    MemoryUpdate,
    /// Memory loaded at session start
    MemoryLoaded,
    /// Before context compaction
    PreCompact,
    /// After context compaction
    PostCompact,
}

impl HookEvent {
    /// Get all hook events
    pub fn all() -> &'static [HookEvent] {
        &[
            HookEvent::UserPromptSubmit,
            HookEvent::PreToolUse,
            HookEvent::PostToolUse,
            HookEvent::PreMessage,
            HookEvent::PostMessage,
            HookEvent::SessionStart,
            HookEvent::SessionEnd,
            HookEvent::Error,
            HookEvent::ToolCancelled,
            HookEvent::PermissionCheck,
            HookEvent::SubagentSpawn,
            HookEvent::SubagentComplete,
            HookEvent::MemoryUpdate,
            HookEvent::MemoryLoaded,
            HookEvent::PreCompact,
            HookEvent::PostCompact,
        ]
    }

    /// Check if this hook can modify the input
    pub fn can_modify_input(&self) -> bool {
        matches!(
            self,
            Self::PreToolUse | Self::PreMessage | Self::PermissionCheck
        )
    }

    /// Check if this hook can cancel the operation
    pub fn can_cancel(&self) -> bool {
        matches!(
            self,
            Self::PreToolUse | Self::PreMessage | Self::PermissionCheck | Self::PreCompact
        )
    }
}

/// Hook context passed to handlers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookContext {
    /// Session ID
    pub session_id: String,
    /// Current working directory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Environment variables (filtered for safety)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::HashMap<String, String>>,
    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl Default for HookContext {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            cwd: None,
            env: None,
            metadata: None,
        }
    }
}

/// User prompt submit hook data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPromptSubmitHookData {
    /// User prompt content
    pub prompt: String,
    /// Session ID
    pub session_id: String,
    /// Message UUID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_uuid: Option<String>,
    /// Turn number
    pub turn: u32,
}

/// Pre-tool-use hook data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreToolUseHookData {
    /// Tool name
    pub tool_name: String,
    /// Tool input
    pub input: serde_json::Value,
    /// Tool use ID
    pub tool_use_id: String,
}

/// Post-tool-use hook data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostToolUseHookData {
    /// Tool name
    pub tool_name: String,
    /// Tool input
    pub input: serde_json::Value,
    /// Tool use ID
    pub tool_use_id: String,
    /// Tool result
    pub result: serde_json::Value,
    /// Whether the tool execution errored
    pub is_error: bool,
    /// Execution duration in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Pre-message hook data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreMessageHookData {
    /// Messages to send
    pub messages: Vec<serde_json::Value>,
    /// System prompt
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
}

/// Post-message hook data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostMessageHookData {
    /// Response content blocks
    pub content: Vec<serde_json::Value>,
    /// Model used
    pub model: String,
    /// Token usage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<super::messages::Usage>,
}

/// Session start hook data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStartHookData {
    /// Session ID
    pub session_id: String,
    /// Initial prompt
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,
    /// Configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
}

/// Session end hook data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEndHookData {
    /// Session ID
    pub session_id: String,
    /// Total duration in milliseconds
    pub duration_ms: u64,
    /// Total turns
    pub num_turns: u32,
    /// Whether ended with error
    pub is_error: bool,
    /// Final result (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

/// Error hook data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorHookData {
    /// Error type
    pub error_type: String,
    /// Error message
    pub message: String,
    /// Stack trace (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    /// Related tool (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

/// Subagent spawn hook data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSpawnHookData {
    /// Subagent type
    pub agent_type: String,
    /// Task description
    pub description: String,
    /// Prompt given to subagent
    pub prompt: String,
    /// Parent session ID
    pub parent_session_id: String,
    /// Subagent session ID
    pub subagent_session_id: String,
}

/// Subagent complete hook data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentCompleteHookData {
    /// Subagent type
    pub agent_type: String,
    /// Parent session ID
    pub parent_session_id: String,
    /// Subagent session ID
    pub subagent_session_id: String,
    /// Result
    pub result: serde_json::Value,
    /// Whether completed with error
    pub is_error: bool,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Memory update hook data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryUpdateHookData {
    /// User ID
    pub user_id: String,
    /// Memory key
    pub key: String,
    /// Previous value (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<serde_json::Value>,
    /// New value
    pub new_value: serde_json::Value,
    /// Source of the update
    pub source: String,
    /// Session ID where update occurred
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Memory loaded hook data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLoadedHookData {
    /// User ID
    pub user_id: String,
    /// Session ID
    pub session_id: String,
    /// Number of memories loaded
    pub memory_count: usize,
    /// Memory keys that were loaded
    pub keys: Vec<String>,
}

/// Hook result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum HookResult {
    /// Continue with original/modified data
    #[serde(rename = "continue")]
    Continue {
        /// Modified data (if any)
        #[serde(skip_serializing_if = "Option::is_none")]
        modified_data: Option<serde_json::Value>,
    },

    /// Cancel the operation
    #[serde(rename = "cancel")]
    Cancel {
        /// Reason for cancellation
        reason: String,
    },

    /// Skip to next iteration (for loops)
    #[serde(rename = "skip")]
    Skip {
        /// Reason for skipping
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    /// Retry the operation
    #[serde(rename = "retry")]
    Retry {
        /// Modified data for retry
        #[serde(skip_serializing_if = "Option::is_none")]
        modified_data: Option<serde_json::Value>,
        /// Delay before retry in milliseconds
        #[serde(skip_serializing_if = "Option::is_none")]
        delay_ms: Option<u64>,
    },
}

impl HookResult {
    /// Create a continue result
    pub fn continue_() -> Self {
        Self::Continue {
            modified_data: None,
        }
    }

    /// Create a continue result with modified data
    pub fn continue_with(data: serde_json::Value) -> Self {
        Self::Continue {
            modified_data: Some(data),
        }
    }

    /// Create a cancel result
    pub fn cancel(reason: impl Into<String>) -> Self {
        Self::Cancel {
            reason: reason.into(),
        }
    }

    /// Create a skip result
    pub fn skip() -> Self {
        Self::Skip { reason: None }
    }

    /// Create a retry result
    pub fn retry() -> Self {
        Self::Retry {
            modified_data: None,
            delay_ms: None,
        }
    }

    /// Check if this is a continue result
    pub fn is_continue(&self) -> bool {
        matches!(self, Self::Continue { .. })
    }

    /// Check if this is a cancel result
    pub fn is_cancel(&self) -> bool {
        matches!(self, Self::Cancel { .. })
    }
}

/// Hook definition for configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    /// Hook event to trigger on
    pub event: HookEvent,
    /// Shell command to execute (for shell hooks)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Timeout in milliseconds
    #[serde(default = "default_hook_timeout")]
    pub timeout_ms: u64,
    /// Whether this hook is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Tool name filter (for tool hooks)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_filter: Option<String>,
    /// Description of what this hook does
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn default_hook_timeout() -> u64 {
    60000 // 60 seconds
}

fn default_true() -> bool {
    true
}

impl HookDefinition {
    /// Create a new hook definition
    pub fn new(event: HookEvent) -> Self {
        Self {
            event,
            command: None,
            timeout_ms: default_hook_timeout(),
            enabled: true,
            tool_filter: None,
            description: None,
        }
    }

    /// Set the shell command
    pub fn command(mut self, cmd: impl Into<String>) -> Self {
        self.command = Some(cmd.into());
        self
    }

    /// Set the timeout
    pub fn timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Set the tool filter
    pub fn tool_filter(mut self, filter: impl Into<String>) -> Self {
        self.tool_filter = Some(filter.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_event_all() {
        let all = HookEvent::all();
        assert!(all.contains(&HookEvent::PreToolUse));
        assert!(all.contains(&HookEvent::PostToolUse));
        assert!(all.contains(&HookEvent::SessionStart));
    }

    #[test]
    fn test_hook_event_can_modify() {
        assert!(HookEvent::PreToolUse.can_modify_input());
        assert!(HookEvent::PreMessage.can_modify_input());
        assert!(!HookEvent::PostToolUse.can_modify_input());
        assert!(!HookEvent::SessionEnd.can_modify_input());
    }

    #[test]
    fn test_hook_result_continue() {
        let result = HookResult::continue_();
        assert!(result.is_continue());
        assert!(!result.is_cancel());
    }

    #[test]
    fn test_hook_result_cancel() {
        let result = HookResult::cancel("Not allowed");
        assert!(result.is_cancel());
        assert!(!result.is_continue());
    }

    #[test]
    fn test_hook_definition_builder() {
        let hook = HookDefinition::new(HookEvent::PreToolUse)
            .command("echo 'pre-tool'")
            .timeout(5000)
            .tool_filter("Bash*");

        assert_eq!(hook.event, HookEvent::PreToolUse);
        assert_eq!(hook.command, Some("echo 'pre-tool'".to_string()));
        assert_eq!(hook.timeout_ms, 5000);
        assert_eq!(hook.tool_filter, Some("Bash*".to_string()));
    }

    #[test]
    fn test_hook_result_serialization() {
        let result = HookResult::continue_();
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"action\":\"continue\""));

        let result = HookResult::cancel("Test reason");
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"action\":\"cancel\""));
        assert!(json.contains("\"reason\":\"Test reason\""));
    }
}
