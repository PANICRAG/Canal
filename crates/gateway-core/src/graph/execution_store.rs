//! Execution store for recording and querying graph execution events.
//!
//! Provides in-memory storage with optional JSONL persistence, LRU eviction,
//! and SSE-compatible event subscriptions.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};

/// Status of an execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionStatus {
    /// Currently running.
    Running,
    /// Completed successfully.
    Completed,
    /// Failed with an error message.
    Failed(String),
}

/// Execution mode identifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionMode {
    Direct,
    PlanExecute,
    Swarm,
    Expert,
    Dag,
    Graph(String),
}

/// A serializable execution event with sequence number.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionEvent {
    /// Monotonically increasing sequence number.
    pub seq: u64,
    /// Event timestamp.
    pub timestamp: DateTime<Utc>,
    /// Event payload.
    #[serde(flatten)]
    pub payload: EventPayload,
}

/// Event payload types for all observable execution events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventPayload {
    // ── Graph layer ──
    /// Graph execution started.
    GraphStarted,
    /// A node is about to execute.
    NodeEntered { node_id: String },
    /// A node completed successfully.
    NodeCompleted { node_id: String, duration_ms: u64 },
    /// A node failed.
    NodeFailed { node_id: String, error: String },
    /// An edge was traversed.
    EdgeTraversed {
        from: String,
        to: String,
        label: String,
    },
    /// Graph execution completed.
    GraphCompleted { total_duration_ms: u64 },
    /// A checkpoint was saved.
    CheckpointSaved {
        node_id: String,
        checkpoint_id: String,
    },

    // ── Swarm layer ──
    /// A swarm handoff was triggered.
    HandoffTriggered {
        from_agent: String,
        to_agent: String,
        condition: String,
        handoff_count: u32,
    },
    /// A handoff condition was checked.
    HandoffConditionChecked {
        agent: String,
        condition: String,
        matched: bool,
    },
    /// A cycle was detected in swarm execution.
    CycleDetected { agent: String, visit_count: u32 },

    // ── Expert layer ──
    /// A specialist was dispatched.
    SpecialistDispatched {
        specialist: String,
        dispatch_count: u32,
    },
    /// Quality gate result for a specialist.
    QualityGateResult {
        specialist: String,
        score: f32,
        passed: bool,
        feedback: String,
    },
    /// Supervisor selected a specialist.
    SupervisorDecision {
        selected: Option<String>,
        available: Vec<String>,
    },

    // ── LLM layer ──
    /// An LLM request was made.
    LlmRequest { model: String, input_tokens: usize },
    /// An LLM response was received.
    LlmResponse {
        model: String,
        duration_ms: u64,
        output_tokens: usize,
    },
    /// A tool was called.
    ToolCall {
        tool_name: String,
        duration_ms: u64,
        success: bool,
    },

    // ── A23 events ──
    /// Parallel execution completed with partial results.
    ParallelPartialComplete {
        node_id: String,
        succeeded: usize,
        failed: usize,
    },
    /// A parallel branch failed.
    ParallelBranchFailed {
        node_id: String,
        branch_id: String,
        error: String,
    },
    /// A DAG wave started.
    DagWaveStarted {
        wave_index: usize,
        node_ids: Vec<String>,
    },
    /// A DAG wave completed.
    DagWaveCompleted { wave_index: usize, duration_ms: u64 },
    /// Memory was hydrated from store.
    MemoryHydrated { entries_loaded: usize },
    /// Memory was flushed to store.
    MemoryFlushed { entries_persisted: usize },
    /// A template was selected.
    TemplateSelected { template_id: String, reason: String },
    /// Auto mode fell back to a different mode.
    AutoModeFallback {
        from_mode: String,
        to_mode: String,
        reason: String,
    },
    /// Budget warning for a node.
    BudgetWarning {
        node_id: String,
        consumed: u32,
        limit: u32,
        scope: String,
    },
    /// Budget exceeded for a node.
    BudgetExceeded {
        node_id: String,
        consumed: u32,
        limit: u32,
        scope: String,
    },

    // ── A24: Auto-Routing + TaskPlanner events ──
    /// Routing classification completed.
    RoutingClassified {
        /// Classification source: "llm_classifier" | "keyword_fallback" | "explicit".
        source: String,
        /// Classification result: "simple" | "multi_step" | "planning" | "expert".
        category: String,
        /// Final routing mode: "Direct" | "Swarm" | "PlanExecute" | "Expert".
        routed_to: String,
        /// Classification confidence (Phase 3 LLM).
        confidence: Option<f64>,
        /// Classification reasoning.
        reasoning: String,
        /// Classification latency in milliseconds.
        classification_ms: u64,
        /// Whether cache was hit.
        cache_hit: bool,
    },
    /// Plan created by TaskPlanner.
    PlanCreated {
        /// Plan goal.
        goal: String,
        /// Total steps in plan.
        total_steps: usize,
        /// Step previews (e.g., "1. [browser] Open Gmail").
        steps_preview: Vec<String>,
        /// Planner LLM model.
        planner_model: String,
        /// Planning latency in milliseconds.
        planning_ms: u64,
    },
    /// A plan step started executing.
    PlanStepStarted {
        step_id: u32,
        action: String,
        tool_category: String,
        dependency: String,
    },
    /// A plan step completed.
    PlanStepCompleted {
        step_id: u32,
        /// Result summary (first 300 chars).
        result_preview: String,
        duration_ms: u64,
        tokens_used: usize,
    },
    /// A plan step failed.
    PlanStepFailed {
        step_id: u32,
        error: String,
        duration_ms: u64,
    },
    /// Re-planning triggered after step failure.
    ReplanTriggered {
        failed_step_id: u32,
        reason: String,
        replan_count: u32,
        max_replans: u32,
    },
    /// Re-planning completed.
    ReplanCompleted {
        new_steps_count: usize,
        new_steps_preview: Vec<String>,
        replanning_ms: u64,
    },
    /// Plan execution completed (all steps).
    PlanCompleted {
        steps_completed: usize,
        steps_failed: usize,
        steps_skipped: usize,
        total_duration_ms: u64,
        total_tokens: usize,
    },

    // ── Content streaming events (Job SSE → ChatView) ──
    /// LLM thinking/reasoning delta from a graph node.
    ThinkingDelta { node_id: String, content: String },
    /// LLM content delta from a graph node.
    ContentDelta { node_id: String, content: String },
    /// A tool call started within a graph node.
    ToolCallStarted {
        node_id: String,
        tool_id: String,
        tool_name: String,
    },
    /// A tool call completed within a graph node.
    ToolCallCompleted { node_id: String, tool_id: String },

    // ── A36 HITL instruction events ──
    /// A human instruction was received for a running job.
    InstructionReceived { job_id: String, message: String },

    /// Human-in-the-loop input is required before execution can continue.
    HITLInputRequired {
        request_id: String,
        job_id: String,
        prompt: String,
        input_type: String,
        options: Option<Vec<String>>,
        timeout_seconds: Option<u64>,
        context: Option<String>,
    },

    // ── A40 Judge events ──
    /// StepJudge evaluated a step (or final synthesis).
    JudgeEvaluated {
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

    // ── Plan approval events ──
    /// Plan requires user approval before execution begins.
    PlanApprovalRequired {
        execution_id: String,
        request_id: String,
        goal: String,
        steps: serde_json::Value,
        success_criteria: String,
        timeout_seconds: u64,
        risk_level: String,
        revision_round: u32,
        max_revisions: u32,
    },

    // ── Job lifecycle events ──
    /// Job completed with final response text.
    /// Emitted just before `complete_execution()` so SSE subscribers and
    /// replay consumers can access the assembled response.
    JobResultReady {
        response: String,
        total_duration_ms: u64,
    },
}

/// Summary of an execution for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionSummary {
    /// Execution identifier.
    pub execution_id: String,
    /// Execution mode.
    pub mode: ExecutionMode,
    /// Current status.
    pub status: ExecutionStatus,
    /// Start time.
    pub started_at: DateTime<Utc>,
    /// End time (if completed).
    pub ended_at: Option<DateTime<Utc>>,
    /// Total events recorded.
    pub event_count: u64,
    /// Total duration in milliseconds (if completed).
    pub duration_ms: Option<u64>,
}

/// A complete execution record.
#[derive(Debug, Clone)]
pub struct ExecutionRecord {
    /// Execution identifier.
    pub execution_id: String,
    /// Execution mode.
    pub mode: ExecutionMode,
    /// Current status.
    pub status: ExecutionStatus,
    /// Start time.
    pub started_at: DateTime<Utc>,
    /// End time.
    pub ended_at: Option<DateTime<Utc>>,
    /// Recorded events.
    pub events: Vec<ExecutionEvent>,
    /// Sequence counter.
    seq_counter: Arc<AtomicU64>,
}

impl ExecutionRecord {
    /// Create a new execution record.
    fn new(execution_id: String, mode: ExecutionMode) -> Self {
        Self {
            execution_id,
            mode,
            status: ExecutionStatus::Running,
            started_at: Utc::now(),
            ended_at: None,
            events: Vec::new(),
            seq_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Add an event to the record.
    fn append(&mut self, payload: EventPayload) -> ExecutionEvent {
        let seq = self.seq_counter.fetch_add(1, Ordering::SeqCst);
        let event = ExecutionEvent {
            seq,
            timestamp: Utc::now(),
            payload,
        };
        self.events.push(event.clone());
        event
    }

    /// Get a summary of this record.
    fn summary(&self) -> ExecutionSummary {
        let duration_ms = self
            .ended_at
            .map(|end| (end - self.started_at).num_milliseconds().max(0) as u64);
        ExecutionSummary {
            execution_id: self.execution_id.clone(),
            mode: self.mode.clone(),
            status: self.status.clone(),
            started_at: self.started_at,
            ended_at: self.ended_at,
            event_count: self.seq_counter.load(Ordering::SeqCst),
            duration_ms,
        }
    }
}

/// Global SSE event (summary-level).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum GlobalEvent {
    /// An execution started.
    #[serde(rename = "execution_started")]
    ExecutionStarted { id: String, mode: ExecutionMode },
    /// An execution completed.
    #[serde(rename = "execution_completed")]
    ExecutionCompleted { id: String, duration_ms: u64 },
    /// An execution failed.
    #[serde(rename = "execution_failed")]
    ExecutionFailed { id: String, error: String },
}

/// Central store for execution records.
///
/// Provides:
/// - In-memory DashMap storage with LRU eviction
/// - Per-execution and global event subscriptions
/// - Optional JSONL persistence to disk
pub struct ExecutionStore {
    /// Execution records keyed by ID.
    records: DashMap<String, ExecutionRecord>,
    /// LRU order (oldest first).
    order: RwLock<VecDeque<String>>,
    /// Maximum records to keep in memory.
    max_records: usize,
    /// Per-execution subscribers.
    subscribers: DashMap<String, Vec<mpsc::Sender<ExecutionEvent>>>,
    /// Global event subscribers.
    global_subscribers: RwLock<Vec<mpsc::Sender<GlobalEvent>>>,
    /// Persistence directory (optional).
    persistence_dir: Option<PathBuf>,
}

impl ExecutionStore {
    /// Create a new in-memory execution store.
    pub fn new(max_records: usize) -> Self {
        Self {
            records: DashMap::new(),
            order: RwLock::new(VecDeque::new()),
            max_records,
            subscribers: DashMap::new(),
            global_subscribers: RwLock::new(Vec::new()),
            persistence_dir: None,
        }
    }

    /// Create a store with JSONL persistence.
    ///
    /// NOTE: JSONL persistence is not yet implemented. The directory is accepted
    /// but data is only stored in memory.
    pub fn with_persistence(mut self, dir: PathBuf) -> Self {
        tracing::warn!(dir = %dir.display(), "JSONL persistence requested but not yet implemented — data is in-memory only");
        self.persistence_dir = Some(dir);
        self
    }

    /// Start a new execution.
    pub async fn start_execution(&self, execution_id: &str, mode: ExecutionMode) {
        let record = ExecutionRecord::new(execution_id.to_string(), mode.clone());
        self.records.insert(execution_id.to_string(), record);

        // Update LRU order
        let mut order = self.order.write().await;
        order.push_back(execution_id.to_string());

        // Evict if over limit
        drop(order);
        self.try_evict().await;

        // Notify global subscribers
        let event = GlobalEvent::ExecutionStarted {
            id: execution_id.to_string(),
            mode,
        };
        self.notify_global(event).await;
    }

    /// Append an event to an execution.
    pub async fn append_event(&self, execution_id: &str, payload: EventPayload) {
        let event = if let Some(mut record) = self.records.get_mut(execution_id) {
            Some(record.append(payload))
        } else {
            tracing::warn!(
                execution_id = %execution_id,
                "Attempted to append event to unknown execution"
            );
            None
        };

        // Notify per-execution subscribers.
        // On buffer full: drop the event but keep the subscriber alive.
        // Only remove subscribers whose channel is actually closed (receiver dropped).
        if let Some(event) = event {
            if let Some(mut subs) = self.subscribers.get_mut(execution_id) {
                subs.retain(|tx| {
                    match tx.try_send(event.clone()) {
                        Ok(()) => true,
                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                            // Buffer full — drop this event but keep the subscriber.
                            // The event is still in the record for replay if needed.
                            tracing::warn!(
                                execution_id = %execution_id,
                                "Subscriber channel full, dropping event (subscriber kept alive)"
                            );
                            true
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                            // Receiver dropped — remove this subscriber
                            false
                        }
                    }
                });
            }
        }
    }

    /// Complete an execution successfully.
    pub async fn complete_execution(&self, execution_id: &str, total_duration_ms: u64) {
        if let Some(mut record) = self.records.get_mut(execution_id) {
            record.status = ExecutionStatus::Completed;
            record.ended_at = Some(Utc::now());
            record.append(EventPayload::GraphCompleted { total_duration_ms });
        }

        // Drop subscriber channels so live SSE streams receive None and close.
        // The final GraphCompleted event is stored in the record for replay.
        self.subscribers.remove(execution_id);

        let event = GlobalEvent::ExecutionCompleted {
            id: execution_id.to_string(),
            duration_ms: total_duration_ms,
        };
        self.notify_global(event).await;
    }

    /// Fail an execution.
    pub async fn fail_execution(&self, execution_id: &str, error: &str) {
        if let Some(mut record) = self.records.get_mut(execution_id) {
            record.status = ExecutionStatus::Failed(error.to_string());
            record.ended_at = Some(Utc::now());
        }

        // Drop subscriber channels so live SSE streams close.
        self.subscribers.remove(execution_id);

        let event = GlobalEvent::ExecutionFailed {
            id: execution_id.to_string(),
            error: error.to_string(),
        };
        self.notify_global(event).await;
    }

    /// Get a summary of a specific execution.
    pub fn get_execution(&self, execution_id: &str) -> Option<ExecutionSummary> {
        self.records.get(execution_id).map(|r| r.summary())
    }

    /// Get all events for an execution, optionally filtered.
    pub fn get_events(
        &self,
        execution_id: &str,
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<ExecutionEvent> {
        if let Some(record) = self.records.get(execution_id) {
            let events = &record.events;
            let start = offset.min(events.len());
            let end = limit
                .map(|l| (start + l).min(events.len()))
                .unwrap_or(events.len());
            events[start..end].to_vec()
        } else {
            Vec::new()
        }
    }

    /// List recent executions.
    pub async fn list_recent(&self, limit: usize) -> Vec<ExecutionSummary> {
        let order = self.order.read().await;
        order
            .iter()
            .rev()
            .take(limit)
            .filter_map(|id| self.records.get(id).map(|r| r.summary()))
            .collect()
    }

    /// List currently running executions.
    pub fn list_active(&self) -> Vec<ExecutionSummary> {
        self.records
            .iter()
            .filter(|r| r.status == ExecutionStatus::Running)
            .map(|r| r.summary())
            .collect()
    }

    /// Subscribe to events for a specific execution.
    ///
    /// Returns a receiver that will receive new events.
    /// Optionally replays recent events before returning.
    ///
    /// **Important**: The subscriber is registered BEFORE reading replay events
    /// to close a TOCTOU race where events arriving between reading the replay
    /// and registering the subscriber would be lost. This means some events may
    /// appear in both the replay vec and the live channel (duplicates), but the
    /// SSE layer assigns sequential IDs so the client can handle this safely.
    pub fn subscribe(
        &self,
        execution_id: &str,
        replay_count: usize,
    ) -> (mpsc::Receiver<ExecutionEvent>, Vec<ExecutionEvent>) {
        let (tx, rx) = mpsc::channel(512);

        // Register subscriber FIRST to close the TOCTOU gap.
        // Any events appended after this point will be delivered via `tx`.
        self.subscribers
            .entry(execution_id.to_string())
            .or_default()
            .push(tx);

        // THEN get replay events. Events that arrive during this read are
        // also delivered via the channel above (possible duplicates, which
        // is preferable to missing events).
        let replay = if let Some(record) = self.records.get(execution_id) {
            let events = &record.events;
            let start = events.len().saturating_sub(replay_count);
            events[start..].to_vec()
        } else {
            Vec::new()
        };

        (rx, replay)
    }

    /// Subscribe to global execution events.
    pub async fn subscribe_global(&self) -> mpsc::Receiver<GlobalEvent> {
        let (tx, rx) = mpsc::channel(64);
        let mut subs = self.global_subscribers.write().await;
        subs.push(tx);
        rx
    }

    /// Get the total number of tracked executions.
    pub fn count(&self) -> usize {
        self.records.len()
    }

    /// Try to evict the oldest non-protected record.
    async fn try_evict(&self) {
        if self.records.len() <= self.max_records {
            return;
        }

        let mut order = self.order.write().await;
        let mut to_remove = Vec::new();

        for id in order.iter() {
            if self.records.len() - to_remove.len() <= self.max_records {
                break;
            }

            // Protect running executions
            if let Some(record) = self.records.get(id) {
                if record.status == ExecutionStatus::Running {
                    continue;
                }
            }

            // Protect executions with subscribers
            if let Some(subs) = self.subscribers.get(id) {
                if !subs.is_empty() {
                    continue;
                }
            }

            to_remove.push(id.clone());
        }

        for id in &to_remove {
            self.records.remove(id);
            self.subscribers.remove(id);
        }

        order.retain(|id| !to_remove.contains(id));
    }

    /// Notify all global subscribers.
    async fn notify_global(&self, event: GlobalEvent) {
        let mut subs = self.global_subscribers.write().await;
        subs.retain(|tx| tx.try_send(event.clone()).is_ok());
    }
}

impl Default for ExecutionStore {
    fn default() -> Self {
        Self::new(100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execution_lifecycle() {
        let store = ExecutionStore::new(10);

        store.start_execution("exec_1", ExecutionMode::Direct).await;

        store
            .append_event(
                "exec_1",
                EventPayload::NodeEntered {
                    node_id: "router".to_string(),
                },
            )
            .await;

        store
            .append_event(
                "exec_1",
                EventPayload::NodeCompleted {
                    node_id: "router".to_string(),
                    duration_ms: 50,
                },
            )
            .await;

        store.complete_execution("exec_1", 100).await;

        let summary = store.get_execution("exec_1").unwrap();
        assert_eq!(summary.status, ExecutionStatus::Completed);
        assert_eq!(summary.event_count, 3); // 2 appended + 1 from complete
    }

    #[tokio::test]
    async fn test_event_retrieval() {
        let store = ExecutionStore::new(10);
        store.start_execution("exec_1", ExecutionMode::Swarm).await;

        for i in 0..5 {
            store
                .append_event(
                    "exec_1",
                    EventPayload::NodeEntered {
                        node_id: format!("node_{}", i),
                    },
                )
                .await;
        }

        // Get all events
        let all = store.get_events("exec_1", 0, None);
        assert_eq!(all.len(), 5);

        // Get with offset
        let from_2 = store.get_events("exec_1", 2, None);
        assert_eq!(from_2.len(), 3);

        // Get with limit
        let first_2 = store.get_events("exec_1", 0, Some(2));
        assert_eq!(first_2.len(), 2);
    }

    #[tokio::test]
    async fn test_list_recent() {
        let store = ExecutionStore::new(10);

        store.start_execution("exec_1", ExecutionMode::Direct).await;
        store.start_execution("exec_2", ExecutionMode::Swarm).await;
        store.start_execution("exec_3", ExecutionMode::Expert).await;

        let recent = store.list_recent(2).await;
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].execution_id, "exec_3"); // Most recent first
    }

    #[tokio::test]
    async fn test_list_active() {
        let store = ExecutionStore::new(10);

        store.start_execution("exec_1", ExecutionMode::Direct).await;
        store.start_execution("exec_2", ExecutionMode::Swarm).await;
        store.complete_execution("exec_1", 100).await;

        let active = store.list_active();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].execution_id, "exec_2");
    }

    #[tokio::test]
    async fn test_lru_eviction() {
        let store = ExecutionStore::new(3);

        store.start_execution("exec_1", ExecutionMode::Direct).await;
        store.complete_execution("exec_1", 10).await;

        store.start_execution("exec_2", ExecutionMode::Direct).await;
        store.complete_execution("exec_2", 10).await;

        store.start_execution("exec_3", ExecutionMode::Direct).await;
        store.complete_execution("exec_3", 10).await;

        // Adding 4th should evict exec_1
        store.start_execution("exec_4", ExecutionMode::Direct).await;

        assert!(store.get_execution("exec_1").is_none());
        assert!(store.get_execution("exec_2").is_some());
        assert!(store.get_execution("exec_4").is_some());
    }

    #[tokio::test]
    async fn test_lru_protects_running() {
        let store = ExecutionStore::new(2);

        store.start_execution("exec_1", ExecutionMode::Direct).await;
        // exec_1 is still Running

        store.start_execution("exec_2", ExecutionMode::Direct).await;
        store.complete_execution("exec_2", 10).await;

        store.start_execution("exec_3", ExecutionMode::Direct).await;

        // exec_1 (Running) should be protected, exec_2 (Completed) should be evicted
        assert!(store.get_execution("exec_1").is_some());
        assert!(store.get_execution("exec_3").is_some());
    }

    #[tokio::test]
    async fn test_subscribe_with_replay() {
        let store = ExecutionStore::new(10);
        store.start_execution("exec_1", ExecutionMode::Direct).await;

        for i in 0..10 {
            store
                .append_event(
                    "exec_1",
                    EventPayload::NodeEntered {
                        node_id: format!("node_{}", i),
                    },
                )
                .await;
        }

        // Subscribe with replay of 5
        let (mut rx, replay) = store.subscribe("exec_1", 5);
        assert_eq!(replay.len(), 5);
        assert_eq!(replay[0].seq, 5); // Last 5 events: seq 5-9

        // New events should be received
        store
            .append_event(
                "exec_1",
                EventPayload::NodeEntered {
                    node_id: "new_node".to_string(),
                },
            )
            .await;

        let event = rx.try_recv().unwrap();
        assert_eq!(event.seq, 10);
    }

    #[tokio::test]
    async fn test_global_subscribe() {
        let store = ExecutionStore::new(10);
        let mut rx = store.subscribe_global().await;

        store.start_execution("exec_1", ExecutionMode::Dag).await;

        let event = rx.try_recv().unwrap();
        if let GlobalEvent::ExecutionStarted { id, .. } = event {
            assert_eq!(id, "exec_1");
        } else {
            panic!("Expected ExecutionStarted");
        }

        store.complete_execution("exec_1", 500).await;

        let event = rx.try_recv().unwrap();
        if let GlobalEvent::ExecutionCompleted { id, duration_ms } = event {
            assert_eq!(id, "exec_1");
            assert_eq!(duration_ms, 500);
        } else {
            panic!("Expected ExecutionCompleted");
        }
    }

    #[tokio::test]
    async fn test_fail_execution() {
        let store = ExecutionStore::new(10);
        store.start_execution("exec_1", ExecutionMode::Direct).await;
        store.fail_execution("exec_1", "something broke").await;

        let summary = store.get_execution("exec_1").unwrap();
        assert_eq!(
            summary.status,
            ExecutionStatus::Failed("something broke".to_string())
        );
    }

    #[tokio::test]
    async fn test_event_sequence_numbers() {
        let store = ExecutionStore::new(10);
        store.start_execution("exec_1", ExecutionMode::Direct).await;

        for _ in 0..5 {
            store
                .append_event("exec_1", EventPayload::GraphStarted)
                .await;
        }

        let events = store.get_events("exec_1", 0, None);
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.seq, i as u64);
        }
    }
}
