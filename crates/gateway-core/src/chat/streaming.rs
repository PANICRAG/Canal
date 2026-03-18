//! Streaming event types for real-time chat responses

use serde::Serialize;
use uuid::Uuid;

use super::message::Artifact;

/// R3-M: Typed severity for constraint violations instead of raw String
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConstraintSeverity {
    /// Blocking violation — request should be rejected
    Error,
    /// Informational — request proceeds with warning
    Warning,
}

/// Stream events for real-time chat updates
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum StreamEvent {
    /// Conversation started
    Start {
        conversation_id: Uuid,
        message_id: Uuid,
        /// Backend agent session_id for trace file lookup
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },

    /// Text chunk from LLM response
    Text { chunk: String },

    /// Agent thinking/reasoning (optional display)
    Thinking { message: String },

    /// Tool execution started
    ToolStart { tool: String, description: String },

    /// Tool execution progress
    ToolProgress {
        tool: String,
        progress: f32,
        message: Option<String>,
    },

    /// Tool execution completed (legacy format)
    ToolResult {
        tool: String,
        success: bool,
        summary: String,
        output: Option<serde_json::Value>,
    },

    /// LLM tool call request (Claude API tool_use format)
    #[serde(rename = "tool_call")]
    ToolCall {
        /// Tool call ID (matches tool_use.id from Claude API)
        id: String,
        /// Tool name (namespace_toolname format)
        name: String,
        /// Tool arguments
        arguments: serde_json::Value,
    },

    /// Tool call result (Claude API tool_result format)
    #[serde(rename = "tool_response")]
    ToolResponse {
        /// Tool call ID (matches the tool_call.id)
        id: String,
        /// Tool name
        name: String,
        /// Tool result (JSON)
        result: serde_json::Value,
    },

    /// Artifact generated
    Artifact { artifact: Artifact },

    /// User confirmation needed
    NeedConfirm {
        task_id: Uuid,
        description: String,
        actions: Vec<ConfirmAction>,
    },

    /// Permission request requiring user approval
    PermissionRequest {
        /// Unique request ID
        request_id: String,
        /// Tool name requesting permission
        tool_name: String,
        /// Tool input parameters
        tool_input: serde_json::Value,
        /// Question to display to the user
        question: String,
        /// Session ID
        session_id: String,
        /// Available options
        options: Vec<PermissionOption>,
    },

    /// Worker progress update (Orchestrator-Worker pattern)
    WorkerProgress {
        /// Worker name
        worker_name: String,
        /// Worker status
        status: WorkerProgressStatus,
        /// Optional progress message
        message: Option<String>,
    },

    /// Code orchestration progress update
    CodeOrchestrationProgress {
        /// Current phase
        phase: CodeOrchestrationPhase,
        /// Optional progress message
        message: Option<String>,
        /// Number of tool calls made so far
        tool_calls_count: Option<usize>,
    },

    /// Constraint violation detected during pre-flight or post-flight validation
    ConstraintViolation {
        /// Type of violation (e.g., "blocked_command", "role_drift", "output_format")
        violation_type: String,
        /// Human-readable violation message
        message: String,
        /// Severity: error (blocking) or warning (informational)
        severity: ConstraintSeverity,
        /// Suggested fix or alternative
        suggestion: Option<String>,
        /// Validation phase: "preflight", "postflight", or "repaired"
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
    },

    /// Custom event for extensibility
    Custom {
        /// Event type identifier
        event_type: String,
        /// Event data
        data: serde_json::Value,
    },

    /// Error occurred
    Error { message: String, recoverable: bool },

    /// Message complete
    Done {
        message_id: Uuid,
        artifacts: Vec<Artifact>,
        usage: Option<TokenUsage>,
    },

    /// Heartbeat to keep connection alive
    Heartbeat,
}

/// Confirmation action
#[derive(Debug, Clone, Serialize)]
pub struct ConfirmAction {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub is_destructive: bool,
}

/// Token usage statistics
#[derive(Debug, Clone, Serialize)]
pub struct TokenUsage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

/// Worker progress status for streaming events
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerProgressStatus {
    /// Worker has been queued
    Queued,
    /// Worker is currently running
    Running,
    /// Worker completed successfully
    Completed,
    /// Worker failed
    Failed,
    /// Worker timed out
    TimedOut,
}

/// Code orchestration phase for streaming events
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeOrchestrationPhase {
    /// Starting the tool proxy bridge
    StartingProxy,
    /// Generating SDK preamble
    GeneratingPreamble,
    /// Executing code in sandbox
    Executing,
    /// Collecting results
    CollectingResults,
    /// Code execution completed
    Completed,
    /// Code execution failed
    Failed,
}

/// Permission option for stream events
#[derive(Debug, Clone, Serialize)]
pub struct PermissionOption {
    /// Display label
    pub label: String,
    /// Option value
    pub value: String,
    /// Whether this is the default option
    pub is_default: bool,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl StreamEvent {
    /// Create a start event
    pub fn start(conversation_id: Uuid, message_id: Uuid) -> Self {
        Self::Start {
            conversation_id,
            message_id,
            session_id: None,
        }
    }

    /// Create a start event with a backend session_id for trace lookup
    pub fn start_with_session(conversation_id: Uuid, message_id: Uuid, session_id: String) -> Self {
        Self::Start {
            conversation_id,
            message_id,
            session_id: Some(session_id),
        }
    }

    /// Create a text chunk event
    pub fn text(chunk: impl Into<String>) -> Self {
        Self::Text {
            chunk: chunk.into(),
        }
    }

    /// Create a thinking event
    pub fn thinking(message: impl Into<String>) -> Self {
        Self::Thinking {
            message: message.into(),
        }
    }

    /// Create a tool start event
    pub fn tool_start(tool: impl Into<String>, description: impl Into<String>) -> Self {
        Self::ToolStart {
            tool: tool.into(),
            description: description.into(),
        }
    }

    /// Create a legacy tool result event
    pub fn tool_result_legacy(
        tool: impl Into<String>,
        success: bool,
        summary: impl Into<String>,
    ) -> Self {
        Self::ToolResult {
            tool: tool.into(),
            success,
            summary: summary.into(),
            output: None,
        }
    }

    /// Create a tool call event (LLM requesting tool execution)
    pub fn tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: &serde_json::Value,
    ) -> Self {
        Self::ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: arguments.clone(),
        }
    }

    /// Create a tool result event (tool execution completed)
    pub fn tool_result(
        id: impl Into<String>,
        name: impl Into<String>,
        result: &serde_json::Value,
    ) -> Self {
        Self::ToolResponse {
            id: id.into(),
            name: name.into(),
            result: result.clone(),
        }
    }

    /// Create an artifact event
    pub fn artifact(artifact: Artifact) -> Self {
        Self::Artifact { artifact }
    }

    /// Create an error event
    pub fn error(message: impl Into<String>, recoverable: bool) -> Self {
        Self::Error {
            message: message.into(),
            recoverable,
        }
    }

    /// Create a done event
    pub fn done(message_id: Uuid, artifacts: Vec<Artifact>) -> Self {
        Self::Done {
            message_id,
            artifacts,
            usage: None,
        }
    }

    /// Create a done event with usage
    pub fn done_with_usage(message_id: Uuid, artifacts: Vec<Artifact>, usage: TokenUsage) -> Self {
        Self::Done {
            message_id,
            artifacts,
            usage: Some(usage),
        }
    }

    /// Create a worker progress event
    pub fn worker_progress(
        worker_name: impl Into<String>,
        status: WorkerProgressStatus,
        message: Option<String>,
    ) -> Self {
        Self::WorkerProgress {
            worker_name: worker_name.into(),
            status,
            message,
        }
    }

    /// Create a code orchestration progress event
    pub fn code_orchestration_progress(
        phase: CodeOrchestrationPhase,
        message: Option<String>,
        tool_calls_count: Option<usize>,
    ) -> Self {
        Self::CodeOrchestrationProgress {
            phase,
            message,
            tool_calls_count,
        }
    }

    /// Create a constraint violation event
    pub fn constraint_violation(
        violation_type: impl Into<String>,
        message: impl Into<String>,
        severity: ConstraintSeverity,
        suggestion: Option<String>,
    ) -> Self {
        Self::ConstraintViolation {
            violation_type: violation_type.into(),
            message: message.into(),
            severity,
            suggestion,
            phase: None,
        }
    }

    /// Create a constraint violation event with a phase
    pub fn constraint_violation_with_phase(
        violation_type: impl Into<String>,
        message: impl Into<String>,
        severity: ConstraintSeverity,
        suggestion: Option<String>,
        phase: impl Into<String>,
    ) -> Self {
        Self::ConstraintViolation {
            violation_type: violation_type.into(),
            message: message.into(),
            severity,
            suggestion,
            phase: Some(phase.into()),
        }
    }

    /// Create a need confirmation event
    pub fn need_confirm(task_id: Uuid, description: impl Into<String>) -> Self {
        Self::NeedConfirm {
            task_id,
            description: description.into(),
            actions: vec![
                ConfirmAction {
                    id: "confirm".into(),
                    label: "Confirm".into(),
                    description: None,
                    is_destructive: false,
                },
                ConfirmAction {
                    id: "cancel".into(),
                    label: "Cancel".into(),
                    description: None,
                    is_destructive: false,
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_event_serialization() {
        let event = StreamEvent::text("Hello");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("Text"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_done_event() {
        let event = StreamEvent::done(Uuid::new_v4(), vec![]);
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("Done"));
    }
}
