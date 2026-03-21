//! Experience collection via GraphObserver.
//!
//! The [`ExperienceCollector`] buffers completed experiences and the
//! [`LearningObserver`] implements the `GraphObserver` trait to
//! automatically capture graph execution outcomes.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::agent::types::{AgentMessage, ContentBlock};
use crate::graph::{AgentGraphState, GraphError, GraphObserver, NodeId};

use super::experience::{
    Experience, ExperienceResult, FeedbackSignal, NodeTraceEntry, ToolCallRecord,
};

/// Maximum experience buffer size to prevent OOM.
const MAX_BUFFER_SIZE: usize = 10_000;

/// Collects execution experiences from graph executions.
///
/// Experiences are buffered until drained by the learning engine.
/// The collector is thread-safe and can be shared across observers.
pub struct ExperienceCollector {
    buffer: RwLock<Vec<Experience>>,
    buffer_threshold: usize,
    max_buffer_size: usize,
}

impl ExperienceCollector {
    /// Create a new experience collector.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let collector = ExperienceCollector::new(10);
    /// ```
    pub fn new(buffer_threshold: usize) -> Self {
        Self {
            buffer: RwLock::new(Vec::new()),
            buffer_threshold,
            max_buffer_size: MAX_BUFFER_SIZE,
        }
    }

    /// Record an experience into the buffer.
    ///
    /// Returns silently if the buffer is at capacity to prevent OOM.
    #[tracing::instrument(skip(self, experience), fields(task = %experience.task))]
    pub async fn record(&self, experience: Experience) {
        let mut buf = self.buffer.write().await;
        if buf.len() >= self.max_buffer_size {
            tracing::warn!(
                max_buffer_size = self.max_buffer_size,
                "Experience buffer full, dropping new experience"
            );
            return;
        }
        buf.push(experience);
        let size = buf.len();
        tracing::debug!(
            buffer_size = size,
            threshold = self.buffer_threshold,
            "Experience recorded"
        );
    }

    /// Drain all buffered experiences.
    pub async fn drain_buffer(&self) -> Vec<Experience> {
        let mut buf = self.buffer.write().await;
        std::mem::take(&mut *buf)
    }

    /// Get the number of buffered experiences.
    pub async fn buffer_size(&self) -> usize {
        self.buffer.read().await.len()
    }

    /// Whether the buffer has reached its threshold.
    pub async fn is_threshold_reached(&self) -> bool {
        self.buffer.read().await.len() >= self.buffer_threshold
    }
}

/// GraphObserver implementation that collects experiences.
///
/// Tracks node execution trace during graph execution, then
/// converts the final [`AgentGraphState`] into an [`Experience`].
pub struct LearningObserver {
    collector: Arc<ExperienceCollector>,
    node_trace: RwLock<Vec<NodeTraceEntry>>,
}

impl LearningObserver {
    /// Create a new learning observer.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let collector = Arc::new(ExperienceCollector::new(10));
    /// let observer = LearningObserver::new(collector);
    /// ```
    pub fn new(collector: Arc<ExperienceCollector>) -> Self {
        Self {
            collector,
            node_trace: RwLock::new(Vec::new()),
        }
    }
}

#[async_trait]
impl GraphObserver<AgentGraphState> for LearningObserver {
    async fn on_graph_start(&self, _graph_execution_id: &str, _state: &AgentGraphState) {
        // Reset trace for new execution
        let mut trace = self.node_trace.write().await;
        trace.clear();
    }

    async fn on_node_enter(
        &self,
        _graph_execution_id: &str,
        _node_id: &NodeId,
        _state: &AgentGraphState,
    ) {
        // No-op — timing starts at enter, measured at exit
    }

    async fn on_node_exit(
        &self,
        _graph_execution_id: &str,
        node_id: &NodeId,
        _state: &AgentGraphState,
        duration: Duration,
    ) {
        let mut trace = self.node_trace.write().await;
        trace.push(NodeTraceEntry {
            node_id: node_id.to_string(),
            duration_ms: duration.as_millis() as i64,
            success: true,
        });
    }

    async fn on_node_error(
        &self,
        _graph_execution_id: &str,
        node_id: &NodeId,
        _error: &GraphError,
    ) {
        let mut trace = self.node_trace.write().await;
        trace.push(NodeTraceEntry {
            node_id: node_id.to_string(),
            duration_ms: 0,
            success: false,
        });
    }

    async fn on_edge_traverse(
        &self,
        _graph_execution_id: &str,
        _from: &NodeId,
        _to: &NodeId,
        _label: &str,
    ) {
        // No-op
    }

    async fn on_graph_complete(
        &self,
        _graph_execution_id: &str,
        state: &AgentGraphState,
        _total_duration: Duration,
    ) {
        let trace = {
            let mut t = self.node_trace.write().await;
            std::mem::take(&mut *t)
        };

        let experience = Experience::from_graph_state(state, trace);
        self.collector.record(experience).await;
    }

    async fn on_checkpoint(
        &self,
        _graph_execution_id: &str,
        _node_id: &NodeId,
        _checkpoint_id: &str,
    ) {
        // No-op
    }
}

impl Experience {
    /// Extract an experience from a completed graph state.
    ///
    /// The result type is determined by the presence of `state.error`:
    /// - No error and non-empty response -> `Success`
    /// - Error with non-empty response -> `Partial`
    /// - Error with empty response -> `Failure`
    pub fn from_graph_state(state: &AgentGraphState, node_trace: Vec<NodeTraceEntry>) -> Self {
        let result = match &state.error {
            None => ExperienceResult::Success {
                response_summary: truncate(&state.response, 500),
            },
            Some(err) if !state.response.is_empty() => ExperienceResult::Partial {
                response_summary: truncate(&state.response, 500),
                error: err.clone(),
            },
            Some(err) => ExperienceResult::Failure { error: err.clone() },
        };

        let success = state.error.is_none();

        let tool_calls = Self::extract_tool_calls(&state.messages, &node_trace);

        Self {
            id: uuid::Uuid::new_v4(),
            task: state.task.clone(),
            plan: state.plan.clone(),
            tool_calls,
            result,
            duration_ms: state.metadata.duration_ms().unwrap_or(0),
            cost_usd: state.metadata.total_cost_usd,
            models_used: state.metadata.models_used.clone(),
            node_trace,
            feedback: FeedbackSignal::Implicit {
                success,
                retry_count: 0,
            },
            created_at: chrono::Utc::now(),
            user_id: None,
        }
    }

    /// Extract tool call records from agent messages.
    ///
    /// Scans assistant messages for `ToolUse` content blocks to build
    /// [`ToolCallRecord`] entries. If a matching `ToolResult` is found
    /// in subsequent user messages, the success/error status is extracted
    /// from it. If no messages contain tool_use blocks, falls back to
    /// extracting tool names from the node_trace entries.
    fn extract_tool_calls(
        messages: &[AgentMessage],
        node_trace: &[NodeTraceEntry],
    ) -> Vec<ToolCallRecord> {
        // Collect tool_use blocks from assistant messages along with their IDs
        let mut tool_uses: Vec<(String, String, String)> = Vec::new(); // (id, name, input_summary)
        for msg in messages {
            if let AgentMessage::Assistant(assistant) = msg {
                for block in &assistant.content {
                    if let ContentBlock::ToolUse { id, name, input } = block {
                        let input_summary = truncate(&input.to_string(), 200);
                        tool_uses.push((id.clone(), name.clone(), input_summary));
                    }
                }
            }
        }

        if !tool_uses.is_empty() {
            // Build a map of tool_use_id -> (is_error)
            let mut result_map: std::collections::HashMap<String, bool> =
                std::collections::HashMap::new();
            for msg in messages {
                if let AgentMessage::User(user_msg) = msg {
                    if let Some(ref tool_use_id) = user_msg.parent_tool_use_id {
                        // Check tool_use_result for error indication
                        let is_error = user_msg
                            .tool_use_result
                            .as_ref()
                            .and_then(|v| v.get("is_error"))
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        result_map.insert(tool_use_id.clone(), is_error);
                    }
                    // Also check content blocks for ToolResult
                    if let crate::agent::types::MessageContent::Blocks(blocks) = &user_msg.content {
                        for block in blocks {
                            if let ContentBlock::ToolResult {
                                tool_use_id,
                                is_error,
                                ..
                            } = block
                            {
                                result_map.insert(tool_use_id.clone(), is_error.unwrap_or(false));
                            }
                        }
                    }
                }
            }

            return tool_uses
                .into_iter()
                .map(|(id, name, input_summary)| {
                    let is_error = result_map.get(&id).copied().unwrap_or(false);
                    ToolCallRecord {
                        tool_name: name,
                        input_summary,
                        success: !is_error,
                        duration_ms: 0, // Duration not available from messages
                        error: if is_error {
                            Some("tool call failed".into())
                        } else {
                            None
                        },
                    }
                })
                .collect();
        }

        // Fallback: extract tool names from node_trace
        if !node_trace.is_empty() {
            return node_trace
                .iter()
                .map(|entry| ToolCallRecord {
                    tool_name: entry.node_id.clone(),
                    input_summary: String::new(),
                    success: entry.success,
                    duration_ms: entry.duration_ms,
                    error: if entry.success {
                        None
                    } else {
                        Some("node failed".into())
                    },
                })
                .collect();
        }

        vec![]
    }
}

/// Truncate a string to a maximum length, appending "..." if truncated.
/// Uses char boundaries to avoid panicking on multi-byte UTF-8.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(max_len);
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collector_record_and_drain() {
        let collector = ExperienceCollector::new(10);
        assert_eq!(collector.buffer_size().await, 0);

        collector.record(Experience::test_success("task1")).await;
        collector.record(Experience::test_success("task2")).await;
        assert_eq!(collector.buffer_size().await, 2);

        let drained = collector.drain_buffer().await;
        assert_eq!(drained.len(), 2);
        assert_eq!(collector.buffer_size().await, 0);
    }

    #[tokio::test]
    async fn test_collector_threshold() {
        let collector = ExperienceCollector::new(2);
        assert!(!collector.is_threshold_reached().await);

        collector.record(Experience::test_success("t1")).await;
        assert!(!collector.is_threshold_reached().await);

        collector.record(Experience::test_success("t2")).await;
        assert!(collector.is_threshold_reached().await);
    }

    #[tokio::test]
    async fn test_collector_multiple_drains() {
        let collector = ExperienceCollector::new(10);
        collector.record(Experience::test_success("t1")).await;

        let first = collector.drain_buffer().await;
        assert_eq!(first.len(), 1);

        let second = collector.drain_buffer().await;
        assert!(second.is_empty());
    }

    #[test]
    fn test_from_graph_state_success() {
        let mut state = AgentGraphState::new("test task");
        state.response = "Hello world".into();
        state.metadata.total_cost_usd = 0.05;
        state.metadata.models_used = vec!["claude".into()];

        let exp = Experience::from_graph_state(&state, vec![]);
        assert!(exp.is_success());
        assert_eq!(exp.task, "test task");
        assert_eq!(exp.cost_usd, 0.05);
    }

    #[test]
    fn test_from_graph_state_failure() {
        let mut state = AgentGraphState::new("fail task");
        state.error = Some("timeout".into());

        let exp = Experience::from_graph_state(&state, vec![]);
        assert!(!exp.is_success());
        assert!(matches!(exp.result, ExperienceResult::Failure { .. }));
    }

    #[test]
    fn test_from_graph_state_partial() {
        let mut state = AgentGraphState::new("partial task");
        state.response = "Some output".into();
        state.error = Some("incomplete".into());

        let exp = Experience::from_graph_state(&state, vec![]);
        assert!(!exp.is_success());
        assert!(matches!(exp.result, ExperienceResult::Partial { .. }));
    }

    #[test]
    fn test_from_graph_state_with_trace() {
        let state = AgentGraphState::new("traced task");
        let trace = vec![
            NodeTraceEntry {
                node_id: "planner".into(),
                duration_ms: 100,
                success: true,
            },
            NodeTraceEntry {
                node_id: "executor".into(),
                duration_ms: 500,
                success: true,
            },
        ];

        let exp = Experience::from_graph_state(&state, trace);
        assert_eq!(exp.node_trace.len(), 2);
        assert_eq!(exp.node_trace[0].node_id, "planner");
        assert_eq!(exp.node_trace[1].node_id, "executor");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("a longer string here", 10), "a longer s...");
        assert_eq!(truncate("", 10), "");
        assert_eq!(truncate("exact", 5), "exact");
    }

    #[test]
    fn test_extract_tool_calls_from_messages() {
        use crate::agent::types::{AssistantMessage, MessageContent, UserMessage};

        let mut state = AgentGraphState::new("tool call task");
        // Add assistant message with tool_use
        state
            .messages
            .push(AgentMessage::Assistant(AssistantMessage {
                content: vec![ContentBlock::ToolUse {
                    id: "tu_1".into(),
                    name: "screenshot".into(),
                    input: serde_json::json!({}),
                }],
                model: "test-model".into(),
                parent_tool_use_id: None,
                error: None,
            }));
        // Add user message with tool result (success)
        state.messages.push(AgentMessage::User(UserMessage {
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "tu_1".into(),
                content: Some(crate::agent::types::ToolResultContent::Text("ok".into())),
                is_error: Some(false),
            }]),
            uuid: None,
            parent_tool_use_id: Some("tu_1".into()),
            tool_use_result: None,
        }));

        let exp = Experience::from_graph_state(&state, vec![]);
        assert_eq!(exp.tool_calls.len(), 1);
        assert_eq!(exp.tool_calls[0].tool_name, "screenshot");
        assert!(exp.tool_calls[0].success);
        assert!(exp.tool_calls[0].error.is_none());
    }

    #[test]
    fn test_extract_tool_calls_with_error() {
        use crate::agent::types::{AssistantMessage, MessageContent, UserMessage};

        let mut state = AgentGraphState::new("failing tool task");
        state
            .messages
            .push(AgentMessage::Assistant(AssistantMessage {
                content: vec![ContentBlock::ToolUse {
                    id: "tu_err".into(),
                    name: "click".into(),
                    input: serde_json::json!({"x": 100, "y": 200}),
                }],
                model: "test-model".into(),
                parent_tool_use_id: None,
                error: None,
            }));
        // Tool result with error
        state.messages.push(AgentMessage::User(UserMessage {
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "tu_err".into(),
                content: None,
                is_error: Some(true),
            }]),
            uuid: None,
            parent_tool_use_id: Some("tu_err".into()),
            tool_use_result: None,
        }));

        let exp = Experience::from_graph_state(&state, vec![]);
        assert_eq!(exp.tool_calls.len(), 1);
        assert_eq!(exp.tool_calls[0].tool_name, "click");
        assert!(!exp.tool_calls[0].success);
        assert!(exp.tool_calls[0].error.is_some());
    }

    #[test]
    fn test_extract_tool_calls_fallback_to_node_trace() {
        let state = AgentGraphState::new("trace task");
        let trace = vec![
            NodeTraceEntry {
                node_id: "planner".into(),
                duration_ms: 100,
                success: true,
            },
            NodeTraceEntry {
                node_id: "executor".into(),
                duration_ms: 500,
                success: false,
            },
        ];

        let exp = Experience::from_graph_state(&state, trace);
        assert_eq!(exp.tool_calls.len(), 2);
        assert_eq!(exp.tool_calls[0].tool_name, "planner");
        assert!(exp.tool_calls[0].success);
        assert_eq!(exp.tool_calls[1].tool_name, "executor");
        assert!(!exp.tool_calls[1].success);
        assert!(exp.tool_calls[1].error.is_some());
    }

    #[test]
    fn test_extract_tool_calls_empty_when_no_messages_or_trace() {
        let state = AgentGraphState::new("empty task");
        let exp = Experience::from_graph_state(&state, vec![]);
        assert!(exp.tool_calls.is_empty());
    }

    #[test]
    fn test_extract_tool_calls_multiple_tools() {
        use crate::agent::types::AssistantMessage;

        let mut state = AgentGraphState::new("multi tool task");
        state
            .messages
            .push(AgentMessage::Assistant(AssistantMessage {
                content: vec![
                    ContentBlock::ToolUse {
                        id: "tu_a".into(),
                        name: "screenshot".into(),
                        input: serde_json::json!({}),
                    },
                    ContentBlock::Text {
                        text: "Let me take a screenshot and then click.".into(),
                    },
                    ContentBlock::ToolUse {
                        id: "tu_b".into(),
                        name: "click".into(),
                        input: serde_json::json!({"x": 50}),
                    },
                ],
                model: "test-model".into(),
                parent_tool_use_id: None,
                error: None,
            }));

        let exp = Experience::from_graph_state(&state, vec![]);
        assert_eq!(exp.tool_calls.len(), 2);
        assert_eq!(exp.tool_calls[0].tool_name, "screenshot");
        assert_eq!(exp.tool_calls[1].tool_name, "click");
        // Without matching results, assumed success
        assert!(exp.tool_calls[0].success);
        assert!(exp.tool_calls[1].success);
    }
}
