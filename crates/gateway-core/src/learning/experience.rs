//! Experience types for the learning system.
//!
//! An [`Experience`] records the complete outcome of an execution —
//! which tools were called, what model was used, whether it succeeded,
//! and how long it took.  These records feed into the pattern miner
//! and knowledge distiller.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single execution experience.
///
/// Captures everything the learning system needs to know about one
/// completed (or failed) graph execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experience {
    /// Unique identifier for this experience.
    pub id: Uuid,
    /// The task or prompt that was executed.
    pub task: String,
    /// Optional plan that was generated before execution.
    pub plan: Option<String>,
    /// Ordered list of tool calls made during execution.
    pub tool_calls: Vec<ToolCallRecord>,
    /// Outcome of the execution.
    pub result: ExperienceResult,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: i64,
    /// Estimated cost in USD.
    pub cost_usd: f64,
    /// Models used during execution.
    pub models_used: Vec<String>,
    /// Trace of graph nodes executed.
    pub node_trace: Vec<NodeTraceEntry>,
    /// Feedback signal (implicit or explicit).
    pub feedback: FeedbackSignal,
    /// When this experience was recorded.
    pub created_at: DateTime<Utc>,
    /// Optional user who triggered the execution.
    pub user_id: Option<Uuid>,
}

impl Experience {
    /// Create a test experience with a successful result.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let exp = Experience::test_success("Summarize the document");
    /// assert!(exp.is_success());
    /// ```
    pub fn test_success(task: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            task: task.into(),
            plan: None,
            tool_calls: vec![],
            result: ExperienceResult::Success {
                response_summary: "OK".into(),
            },
            duration_ms: 1000,
            cost_usd: 0.01,
            models_used: vec!["test-model".into()],
            node_trace: vec![],
            feedback: FeedbackSignal::Implicit {
                success: true,
                retry_count: 0,
            },
            created_at: Utc::now(),
            user_id: None,
        }
    }

    /// Create a test experience with a failure result.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let exp = Experience::test_failure("bad task", "timeout");
    /// assert!(!exp.is_success());
    /// ```
    pub fn test_failure(task: &str, error: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            task: task.into(),
            plan: None,
            tool_calls: vec![],
            result: ExperienceResult::Failure {
                error: error.into(),
            },
            duration_ms: 500,
            cost_usd: 0.005,
            models_used: vec!["test-model".into()],
            node_trace: vec![],
            feedback: FeedbackSignal::Implicit {
                success: false,
                retry_count: 0,
            },
            created_at: Utc::now(),
            user_id: None,
        }
    }

    /// Whether this experience was successful.
    pub fn is_success(&self) -> bool {
        matches!(self.result, ExperienceResult::Success { .. })
    }
}

/// Record of a single tool call within an experience.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    /// Name of the tool that was called.
    pub tool_name: String,
    /// Summary of the input provided to the tool.
    pub input_summary: String,
    /// Whether the tool call succeeded.
    pub success: bool,
    /// Duration of the tool call in milliseconds.
    pub duration_ms: i64,
    /// Error message if the tool call failed.
    pub error: Option<String>,
}

/// A node trace entry recording execution order and timing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeTraceEntry {
    /// Identifier of the graph node.
    pub node_id: String,
    /// Duration of node execution in milliseconds.
    pub duration_ms: i64,
    /// Whether the node executed successfully.
    pub success: bool,
}

/// Result of an execution experience.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExperienceResult {
    /// The execution completed successfully.
    Success {
        /// Truncated summary of the response.
        response_summary: String,
    },
    /// The execution failed completely.
    Failure {
        /// Error description.
        error: String,
    },
    /// The execution produced partial results before failing.
    Partial {
        /// Truncated summary of whatever was produced.
        response_summary: String,
        /// Error that interrupted execution.
        error: String,
    },
}

impl ExperienceResult {
    /// Whether this result is a success.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }
}

/// Feedback signal for an experience.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FeedbackSignal {
    /// Automatically derived from execution outcome.
    Implicit {
        /// Whether the execution was considered successful.
        success: bool,
        /// How many retries occurred before finishing.
        retry_count: u32,
    },
    /// Explicit user feedback.
    Explicit {
        /// User's rating.
        rating: FeedbackRating,
        /// Optional free-form comment.
        comment: Option<String>,
    },
}

/// User feedback rating.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackRating {
    /// Positive feedback.
    ThumbsUp,
    /// Negative feedback.
    ThumbsDown,
    /// Numeric score (0-10).
    Score(u8),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_experience_success() {
        let exp = Experience::test_success("test");
        assert!(exp.is_success());
        assert!(exp.result.is_success());
    }

    #[test]
    fn test_experience_failure() {
        let exp = Experience::test_failure("test", "error");
        assert!(!exp.is_success());
        assert!(!exp.result.is_success());
    }

    #[test]
    fn test_serialize_experience() {
        let exp = Experience::test_success("test task");
        let json = serde_json::to_string(&exp).unwrap();
        let parsed: Experience = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task, "test task");
    }

    #[test]
    fn test_feedback_signal_serialize() {
        let implicit = FeedbackSignal::Implicit {
            success: true,
            retry_count: 0,
        };
        let json = serde_json::to_string(&implicit).unwrap();
        assert!(json.contains("implicit"));

        let explicit = FeedbackSignal::Explicit {
            rating: FeedbackRating::ThumbsUp,
            comment: Some("Great!".into()),
        };
        let json = serde_json::to_string(&explicit).unwrap();
        assert!(json.contains("explicit"));
    }

    #[test]
    fn test_experience_result_variants() {
        let success = ExperienceResult::Success {
            response_summary: "done".into(),
        };
        assert!(success.is_success());

        let failure = ExperienceResult::Failure {
            error: "oops".into(),
        };
        assert!(!failure.is_success());

        let partial = ExperienceResult::Partial {
            response_summary: "half done".into(),
            error: "interrupted".into(),
        };
        assert!(!partial.is_success());
    }

    #[test]
    fn test_tool_call_record_serialize() {
        let record = ToolCallRecord {
            tool_name: "file_read".into(),
            input_summary: "path=/tmp/test.txt".into(),
            success: true,
            duration_ms: 50,
            error: None,
        };
        let json = serde_json::to_string(&record).unwrap();
        let parsed: ToolCallRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tool_name, "file_read");
        assert!(parsed.success);
    }

    #[test]
    fn test_node_trace_entry_serialize() {
        let entry = NodeTraceEntry {
            node_id: "planner".into(),
            duration_ms: 200,
            success: true,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: NodeTraceEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.node_id, "planner");
    }
}
