//! Streaming observer that sends graph events via an mpsc channel.
//!
//! This observer bridges the `GraphObserver` trait to SSE (Server-Sent Events)
//! by serializing each lifecycle event into a `GraphStreamEvent` and sending it
//! through a `tokio::sync::mpsc` channel. The API layer can then consume these
//! events and forward them to the frontend via SSE.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::error::{GraphError, NodeId};
use super::observer::GraphObserver;
use super::GraphState;

/// A serializable event emitted by the streaming observer.
///
/// These events map 1:1 to frontend store actions:
/// - `GraphStarted` → `setExecution()`
/// - `NodeEntered` → `setCurrentNode()` + `updateNodeStatus("entered")`
/// - `NodeCompleted` → `updateNodeStatus("completed")`
/// - `NodeFailed` → `updateNodeStatus("failed")`
/// - `EdgeTraversed` → `markEdgeTraversed()`
/// - `GraphCompleted` → `completeExecution("completed")`
/// - `GraphFailed` → `completeExecution("failed")`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", content = "data")]
pub enum GraphStreamEvent {
    /// Graph execution has started.
    #[serde(rename = "graph_started")]
    GraphStarted { execution_id: String },

    /// A node is about to execute.
    #[serde(rename = "graph_node_entered")]
    NodeEntered {
        execution_id: String,
        node_id: String,
    },

    /// A node has completed successfully.
    #[serde(rename = "graph_node_completed")]
    NodeCompleted {
        execution_id: String,
        node_id: String,
        duration_ms: u64,
    },

    /// A node execution has failed.
    #[serde(rename = "graph_node_failed")]
    NodeFailed {
        execution_id: String,
        node_id: String,
        error: String,
    },

    /// An edge has been traversed between nodes.
    #[serde(rename = "graph_edge_traversed")]
    EdgeTraversed {
        execution_id: String,
        from: String,
        to: String,
        label: String,
    },

    /// Graph execution has completed successfully.
    #[serde(rename = "graph_completed")]
    GraphCompleted {
        execution_id: String,
        total_duration_ms: u64,
    },

    // ── A23 new events ──
    /// Parallel execution completed with partial results.
    #[serde(rename = "graph_parallel_partial")]
    ParallelPartial {
        execution_id: String,
        node_id: String,
        succeeded: usize,
        failed: usize,
    },

    /// A parallel branch failed.
    #[serde(rename = "graph_parallel_branch_failed")]
    ParallelBranchFailed {
        execution_id: String,
        node_id: String,
        branch_id: String,
        error: String,
    },

    /// A DAG execution wave started.
    #[serde(rename = "graph_dag_wave_started")]
    DagWaveStarted {
        execution_id: String,
        wave_index: usize,
        node_ids: Vec<String>,
    },

    /// A DAG execution wave completed.
    #[serde(rename = "graph_dag_wave_completed")]
    DagWaveCompleted {
        execution_id: String,
        wave_index: usize,
        duration_ms: u64,
    },

    /// Budget warning for a node.
    #[serde(rename = "graph_budget_warning")]
    BudgetWarning {
        execution_id: String,
        node_id: String,
    },

    /// Budget exceeded for a node.
    #[serde(rename = "graph_budget_exceeded")]
    BudgetExceeded {
        execution_id: String,
        node_id: String,
    },

    /// LLM thinking/reasoning content from a node (streamed as it arrives).
    #[serde(rename = "node_thinking")]
    NodeThinking {
        execution_id: String,
        node_id: String,
        content: String,
    },

    /// LLM text content from a node (streamed as it arrives).
    #[serde(rename = "node_text")]
    NodeText {
        execution_id: String,
        node_id: String,
        content: String,
    },

    /// Tool call from a node.
    #[serde(rename = "node_tool_call")]
    NodeToolCall {
        execution_id: String,
        node_id: String,
        tool_id: String,
        tool_name: String,
    },

    /// Tool result from a node.
    #[serde(rename = "node_tool_result")]
    NodeToolResult {
        execution_id: String,
        node_id: String,
        tool_id: String,
    },

    // ── A40 Judge events ──
    /// StepJudge evaluated a plan step or final synthesis.
    #[serde(rename = "judge_evaluated")]
    JudgeEvaluated {
        execution_id: String,
        /// Step ID being evaluated (None = final judge).
        step_id: Option<String>,
        /// Judge verdict: "pass", "partial_pass", "fail", "stalled".
        verdict: String,
        /// Brief explanation of why this verdict was chosen.
        reasoning: String,
        /// Actionable suggestions for retry or replan.
        suggestions: Vec<String>,
        /// Number of retries so far for this step.
        retry_count: u32,
    },

    /// Plan requires user approval before execution begins.
    #[serde(rename = "plan_approval_required")]
    PlanApprovalRequired {
        execution_id: String,
        /// Frontend uses this to POST back the decision.
        request_id: String,
        /// High-level goal of the plan.
        goal: String,
        /// Enriched steps for review (JSON array of PlanStepReview).
        steps: serde_json::Value,
        /// Success criteria for the overall plan.
        success_criteria: String,
        /// Seconds before auto-rejection.
        timeout_seconds: u64,
        /// Maximum risk across all steps.
        risk_level: String,
        /// Current revision round (0 = initial plan).
        revision_round: u32,
        /// Maximum allowed revision rounds.
        max_revisions: u32,
    },

    // ── A36 HITL instruction events ──
    /// A human instruction was received for a running job.
    ///
    /// Emitted when a user sends an instruction via `POST /api/jobs/:id/instruct`.
    /// The frontend can display this in the SSE stream, and the next graph step
    /// can read it from `AgentGraphState.working_memory["pending_instructions"]`.
    #[serde(rename = "instruction_received")]
    InstructionReceived {
        execution_id: String,
        /// The job this instruction targets.
        job_id: String,
        /// The instruction message from the user.
        message: String,
    },

    /// Human-in-the-loop input is required before execution can continue.
    ///
    /// The frontend should display a prompt to the user and POST the response
    /// to `POST /api/jobs/{job_id}/input` with the `request_id`.
    #[serde(rename = "hitl_input_required")]
    HITLInputRequired {
        /// Graph execution ID.
        execution_id: String,
        /// Unique ID for this input request (used in the POST response).
        request_id: String,
        /// Job ID (for constructing the POST URL).
        job_id: String,
        /// Human-readable prompt describing what input is needed.
        prompt: String,
        /// Type of input expected: "text", "choice", or "confirmation".
        input_type: String,
        /// Available options (only relevant for "choice" input_type).
        options: Option<Vec<String>>,
        /// Seconds before the request auto-expires.
        timeout_seconds: Option<u64>,
        /// Additional context for the user (e.g., what the agent was doing).
        context: Option<String>,
    },

    // ── A43 Research Planner Pipeline events ──
    /// Research agent progress update (streamed during codebase exploration).
    #[serde(rename = "research_progress")]
    ResearchProgress {
        execution_id: String,
        /// Current research phase: "scope_analysis", "codebase_discovery", etc.
        phase: String,
        /// Human-readable progress message.
        message: String,
    },

    /// Complexity assessment result (code-computed, not LLM).
    #[serde(rename = "complexity_assessed")]
    ComplexityAssessed {
        execution_id: String,
        /// "simple", "medium", or "complex".
        complexity: String,
        /// Brief explanation of the scoring.
        reasoning: String,
        /// Whether a PRD will be generated for this task.
        will_generate_prd: bool,
    },

    /// User needs to answer clarifying questions before PRD generation.
    #[serde(rename = "clarification_required")]
    ClarificationRequired {
        execution_id: String,
        /// Frontend uses this to POST back the answers.
        request_id: String,
        /// Questions to present (JSON array of ClarifyingQuestion).
        questions: serde_json::Value,
        /// One-line summary of the task.
        task_summary: String,
        /// Seconds before auto-skip with defaults.
        timeout_seconds: u64,
    },

    /// PRD ready for user review — approve, revise, or reject.
    #[serde(rename = "prd_review_required")]
    PrdReviewRequired {
        execution_id: String,
        /// Frontend uses this to POST back the decision.
        request_id: String,
        /// Full PRD document as JSON.
        prd: serde_json::Value,
        /// Seconds before auto-rejection.
        timeout_seconds: u64,
        /// Current revision round (1-based).
        revision_round: u32,
        /// Maximum allowed revision rounds.
        max_revisions: u32,
    },
}

/// A `GraphObserver` implementation that sends events via an mpsc channel.
///
/// # Example
///
/// ```ignore
/// let (tx, mut rx) = tokio::sync::mpsc::channel(64);
/// let observer = StreamingObserver::new(tx);
///
/// // Use observer with graph executor
/// let graph = StateGraphBuilder::new()
///     .observer(observer)
///     .build()?;
///
/// // Consume events in another task
/// tokio::spawn(async move {
///     while let Some(event) = rx.recv().await {
///         // Forward to SSE stream
///     }
/// });
/// ```
pub struct StreamingObserver {
    tx: mpsc::Sender<GraphStreamEvent>,
}

impl StreamingObserver {
    /// Create a new streaming observer with the given channel sender.
    pub fn new(tx: mpsc::Sender<GraphStreamEvent>) -> Self {
        Self { tx }
    }

    /// Create a new streaming observer and return both the observer and receiver.
    pub fn channel(buffer: usize) -> (Self, mpsc::Receiver<GraphStreamEvent>) {
        let (tx, rx) = mpsc::channel(buffer);
        (Self::new(tx), rx)
    }

    /// Get a clone of the underlying sender for forwarding content events from nodes.
    pub fn sender(&self) -> mpsc::Sender<GraphStreamEvent> {
        self.tx.clone()
    }

    async fn send(&self, event: GraphStreamEvent) {
        if let Err(e) = self.tx.send(event).await {
            tracing::warn!("Graph stream receiver dropped: {}", e);
        }
    }
}

#[async_trait]
impl<S: GraphState> GraphObserver<S> for StreamingObserver {
    async fn on_graph_start(&self, graph_execution_id: &str, _state: &S) {
        self.send(GraphStreamEvent::GraphStarted {
            execution_id: graph_execution_id.to_string(),
        })
        .await;
    }

    async fn on_node_enter(&self, graph_execution_id: &str, node_id: &NodeId, _state: &S) {
        self.send(GraphStreamEvent::NodeEntered {
            execution_id: graph_execution_id.to_string(),
            node_id: node_id.clone(),
        })
        .await;
    }

    async fn on_node_exit(
        &self,
        graph_execution_id: &str,
        node_id: &NodeId,
        _state: &S,
        duration: Duration,
    ) {
        self.send(GraphStreamEvent::NodeCompleted {
            execution_id: graph_execution_id.to_string(),
            node_id: node_id.clone(),
            duration_ms: duration.as_millis() as u64,
        })
        .await;
    }

    async fn on_node_error(&self, graph_execution_id: &str, node_id: &NodeId, error: &GraphError) {
        self.send(GraphStreamEvent::NodeFailed {
            execution_id: graph_execution_id.to_string(),
            node_id: node_id.clone(),
            error: error.to_string(),
        })
        .await;
    }

    async fn on_edge_traverse(
        &self,
        graph_execution_id: &str,
        from: &NodeId,
        to: &NodeId,
        label: &str,
    ) {
        self.send(GraphStreamEvent::EdgeTraversed {
            execution_id: graph_execution_id.to_string(),
            from: from.clone(),
            to: to.clone(),
            label: label.to_string(),
        })
        .await;
    }

    async fn on_graph_complete(
        &self,
        graph_execution_id: &str,
        _state: &S,
        total_duration: Duration,
    ) {
        self.send(GraphStreamEvent::GraphCompleted {
            execution_id: graph_execution_id.to_string(),
            total_duration_ms: total_duration.as_millis() as u64,
        })
        .await;
    }

    async fn on_parallel_partial(
        &self,
        exec_id: &str,
        node_id: &super::error::NodeId,
        succeeded: usize,
        failed: usize,
    ) {
        self.send(GraphStreamEvent::ParallelPartial {
            execution_id: exec_id.to_string(),
            node_id: node_id.clone(),
            succeeded,
            failed,
        })
        .await;
    }

    async fn on_parallel_branch_failed(
        &self,
        exec_id: &str,
        node_id: &super::error::NodeId,
        branch_id: &super::error::NodeId,
        error: &str,
    ) {
        self.send(GraphStreamEvent::ParallelBranchFailed {
            execution_id: exec_id.to_string(),
            node_id: node_id.clone(),
            branch_id: branch_id.clone(),
            error: error.to_string(),
        })
        .await;
    }

    async fn on_dag_wave_start(
        &self,
        exec_id: &str,
        wave_index: usize,
        node_ids: &[super::error::NodeId],
    ) {
        self.send(GraphStreamEvent::DagWaveStarted {
            execution_id: exec_id.to_string(),
            wave_index,
            node_ids: node_ids.to_vec(),
        })
        .await;
    }

    async fn on_dag_wave_complete(&self, exec_id: &str, wave_index: usize, duration: Duration) {
        self.send(GraphStreamEvent::DagWaveCompleted {
            execution_id: exec_id.to_string(),
            wave_index,
            duration_ms: duration.as_millis() as u64,
        })
        .await;
    }

    async fn on_budget_warning(&self, exec_id: &str, node_id: &super::error::NodeId) {
        self.send(GraphStreamEvent::BudgetWarning {
            execution_id: exec_id.to_string(),
            node_id: node_id.clone(),
        })
        .await;
    }

    async fn on_budget_exceeded(&self, exec_id: &str, node_id: &super::error::NodeId) {
        self.send(GraphStreamEvent::BudgetExceeded {
            execution_id: exec_id.to_string(),
            node_id: node_id.clone(),
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct TestState {
        value: i32,
    }

    impl super::super::GraphState for TestState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
        }
    }

    #[tokio::test]
    async fn test_streaming_observer_sends_events() {
        let (observer, mut rx) = StreamingObserver::channel(16);
        let state = TestState { value: 42 };

        // Explicit type annotation to disambiguate GraphObserver<TestState>
        let obs: &dyn GraphObserver<TestState> = &observer;

        obs.on_graph_start("exec_1", &state).await;
        obs.on_node_enter("exec_1", &"node_a".to_string(), &state)
            .await;
        obs.on_node_exit(
            "exec_1",
            &"node_a".to_string(),
            &state,
            Duration::from_millis(150),
        )
        .await;
        obs.on_edge_traverse(
            "exec_1",
            &"node_a".to_string(),
            &"node_b".to_string(),
            "next",
        )
        .await;
        obs.on_graph_complete("exec_1", &state, Duration::from_secs(2))
            .await;

        // Verify events received
        let e1 = rx.recv().await.unwrap();
        assert!(matches!(e1, GraphStreamEvent::GraphStarted { .. }));

        let e2 = rx.recv().await.unwrap();
        assert!(matches!(e2, GraphStreamEvent::NodeEntered { .. }));

        let e3 = rx.recv().await.unwrap();
        if let GraphStreamEvent::NodeCompleted { duration_ms, .. } = e3 {
            assert_eq!(duration_ms, 150);
        } else {
            panic!("Expected NodeCompleted");
        }

        let e4 = rx.recv().await.unwrap();
        if let GraphStreamEvent::EdgeTraversed { label, .. } = e4 {
            assert_eq!(label, "next");
        } else {
            panic!("Expected EdgeTraversed");
        }

        let e5 = rx.recv().await.unwrap();
        if let GraphStreamEvent::GraphCompleted {
            total_duration_ms, ..
        } = e5
        {
            assert_eq!(total_duration_ms, 2000);
        } else {
            panic!("Expected GraphCompleted");
        }
    }

    #[tokio::test]
    async fn test_streaming_observer_handles_dropped_receiver() {
        let (tx, rx) = mpsc::channel(1);
        let observer = StreamingObserver::new(tx);
        let state = TestState { value: 1 };

        // Drop the receiver
        drop(rx);

        // Should not panic — explicit type annotation
        let obs: &dyn GraphObserver<TestState> = &observer;
        obs.on_graph_start("exec_1", &state).await;
        obs.on_node_enter("exec_1", &"n1".to_string(), &state).await;
    }

    #[test]
    fn test_graph_stream_event_serialization() {
        let event = GraphStreamEvent::NodeCompleted {
            execution_id: "exec_1".to_string(),
            node_id: "node_a".to_string(),
            duration_ms: 150,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("graph_node_completed"));
        assert!(json.contains("150"));

        let deserialized: GraphStreamEvent = serde_json::from_str(&json).unwrap();
        if let GraphStreamEvent::NodeCompleted { duration_ms, .. } = deserialized {
            assert_eq!(duration_ms, 150);
        } else {
            panic!("Deserialization failed");
        }
    }
}
