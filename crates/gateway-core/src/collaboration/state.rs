//! Typed state overlay for PlanExecute mode (A44).
//!
//! Replaces the 50+ magic string keys in `AgentGraphState.working_memory`
//! with compile-time typed access via `PlanExecuteState`. The state is stored
//! as a single serialized blob under the `__plan_execute_state__` key in
//! working_memory, keeping backward compatibility with the untyped HashMap.
//!
//! # Usage
//!
//! ```ignore
//! // Read
//! let ps = state.plan_state();
//! let idx = ps.current_step_index;
//!
//! // Write
//! state.update_plan_state(|ps| {
//!     ps.current_step_index += 1;
//! });
//! ```

use serde::{Deserialize, Serialize};

use super::planner::PlanStep;

/// The reserved key in `AgentGraphState.working_memory` where the typed state is stored.
pub const PLAN_STATE_KEY: &str = "__plan_execute_state__";

/// Pipeline phase for the PlanExecute graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelinePhase {
    /// Research phase — gathering information about the task.
    Research,
    /// Clarification phase — asking the user for missing details.
    Clarification,
    /// PRD assembly — building a Product Requirements Document.
    PrdAssembly,
    /// PRD approval — waiting for human approval of the PRD.
    PrdApproval,
    /// PRD distillation — extracting actionable context from the PRD.
    PrdDistill,
    /// Planning — creating an execution plan from the distilled PRD.
    Planning,
    /// Plan approval — waiting for human approval of the plan.
    PlanApproval,
    /// Execution — running plan steps one by one.
    Execution,
    /// Judging — evaluating the output of a step (A39).
    Judging,
    /// Replanning — adjusting the plan after a failed step.
    Replanning,
    /// Synthesis — producing the final result.
    Synthesis,
    /// Done — execution complete.
    Done,
}

impl Default for PipelinePhase {
    fn default() -> Self {
        Self::Research
    }
}

/// Approval decision for PRD or Plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Pending,
    Approved,
    Rejected,
}

/// Status of a single plan step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Done,
    Error,
    Skipped,
    Retrying,
    Replanning,
}

impl Default for StepStatus {
    fn default() -> Self {
        Self::Pending
    }
}

/// Per-step execution state. Replaces the `step_{idx}_*` magic keys.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepExecutionState {
    /// Step output text.
    pub result: Option<String>,
    /// Current status.
    pub status: StepStatus,
    /// Error message if any.
    pub error: Option<String>,
    /// Number of retries attempted.
    pub retry_count: u32,
    /// Previous output (for stall detection).
    pub prev_output: Option<String>,
    /// Per-step replan count.
    pub replan_count: u32,
    /// Reflection from judge node (A39).
    pub reflection: Option<serde_json::Value>,
    /// Retry suggestions from judge.
    pub retry_suggestions: Option<String>,
    /// Replan request (new steps from replanner).
    pub replan_request: Option<serde_json::Value>,
}

/// Typed state for the PlanExecute pipeline.
///
/// This replaces 50+ magic string keys in `AgentGraphState.working_memory`
/// with compile-time checked fields. All fields have sensible defaults,
/// and the struct is `Serialize + Deserialize` for checkpointing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanExecuteState {
    // ── Identity ──
    /// Graph execution ID.
    pub execution_id: String,
    /// Job ID (for async execution).
    pub job_id: Option<String>,
    /// Current pipeline phase.
    pub phase: PipelinePhase,

    // ── Research → PRD pipeline ──
    /// Research output (serialized ResearchOutput).
    pub research_output: Option<serde_json::Value>,
    /// Task complexity assessment.
    pub task_complexity: Option<String>,
    /// Clarification questions for the user.
    pub clarification_questions: Option<serde_json::Value>,
    /// User's answers to clarification questions.
    pub clarification_answers: Option<serde_json::Value>,

    // ── PRD assembly ──
    /// The assembled PRD document.
    pub prd_document: Option<serde_json::Value>,
    /// Distilled PRD context for the planner.
    pub prd_context: Option<String>,
    /// Distilled PRD (after distillation phase).
    pub distilled_prd: Option<serde_json::Value>,
    /// Chosen approach index from the PRD.
    pub chosen_approach: Option<usize>,
    /// PRD approval decision.
    pub prd_decision: Option<ApprovalDecision>,
    /// Feedback for PRD revision.
    pub prd_revision_feedback: Option<String>,
    /// PRD revision round counter.
    pub prd_revision_round: u32,

    // ── Planning ──
    /// The plan steps.
    pub plan_steps: Vec<PlanStep>,
    /// High-level goal of the plan.
    pub plan_goal: String,
    /// Success criteria for the plan.
    pub success_criteria: String,
    /// Index of the step currently being executed.
    pub current_step_index: usize,
    /// Total number of steps.
    pub total_steps: usize,
    /// Number of replans performed.
    pub replan_count: u32,
    /// Plan approval decision.
    pub plan_decision: Option<ApprovalDecision>,
    /// Feedback for plan revision.
    pub plan_revision_feedback: Option<String>,
    /// Plan revision round counter.
    pub revision_round: u32,
    /// Verified plans offered to the user.
    pub offered_verified_plan: Option<serde_json::Value>,

    // ── Execution control ──
    /// Whether a replan is needed.
    pub needs_replan: bool,
    /// Pending instruction from user (HITL).
    pub pending_instruction: Option<String>,
    /// Total skipped steps (for global skip limit).
    pub total_skipped_steps: u32,
    /// Rejection reason (from approval gates).
    pub rejection_reason: Option<String>,

    // ── Per-step results ──
    /// Per-step execution state. Indexed by step index.
    pub steps: Vec<StepExecutionState>,

    // ── Judge / verdict (A39) ──
    /// Latest judge verdict.
    pub judge_verdict: Option<String>,
    /// Guidance for replanner.
    pub replan_guidance: Option<String>,

    // ── Verified plans (A39) ──
    /// Candidate for verified plan registration.
    pub verified_plan_candidate: Option<serde_json::Value>,

    // ── Metrics ──
    /// Execution metrics (serialized).
    pub execution_metrics: Option<serde_json::Value>,
}

impl PlanExecuteState {
    /// Create a new state with the given execution ID.
    pub fn new(execution_id: impl Into<String>) -> Self {
        Self {
            execution_id: execution_id.into(),
            ..Default::default()
        }
    }

    /// Get the step execution state for a given index, creating it if needed.
    pub fn step_mut(&mut self, index: usize) -> &mut StepExecutionState {
        // Grow the vec if needed
        while self.steps.len() <= index {
            self.steps.push(StepExecutionState::default());
        }
        &mut self.steps[index]
    }

    /// Get the step execution state for a given index (read-only).
    pub fn step(&self, index: usize) -> Option<&StepExecutionState> {
        self.steps.get(index)
    }

    /// Get the current step's execution state (mutable).
    pub fn current_step_mut(&mut self) -> &mut StepExecutionState {
        let idx = self.current_step_index;
        self.step_mut(idx)
    }

    /// Get the current step's execution state (read-only).
    pub fn current_step(&self) -> Option<&StepExecutionState> {
        self.step(self.current_step_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_execute_state_serialize_roundtrip() {
        let mut state = PlanExecuteState::new("exec-1");
        state.phase = PipelinePhase::Execution;
        state.plan_goal = "Build a feature".into();
        state.current_step_index = 2;
        state.total_steps = 5;
        state.plan_steps = vec![PlanStep {
            id: 1,
            action: "Do something".into(),
            tool_category: crate::collaboration::planner::ToolCategory::Code,
            dependency: crate::collaboration::planner::StepDependency::Sequential,
            expected_output: None,
            executor_agent: None,
            executor_model: None,
            requires_visual_verification: None,
            prd_content: None,
            executor_type: None,
        }];
        state.step_mut(0).status = StepStatus::Done;
        state.step_mut(0).result = Some("OK".into());
        state.step_mut(1).status = StepStatus::Error;
        state.step_mut(1).error = Some("fail".into());

        let json = serde_json::to_string(&state).unwrap();
        let restored: PlanExecuteState = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.execution_id, "exec-1");
        assert_eq!(restored.phase, PipelinePhase::Execution);
        assert_eq!(restored.current_step_index, 2);
        assert_eq!(restored.total_steps, 5);
        assert_eq!(restored.steps.len(), 2);
        assert_eq!(restored.steps[0].status, StepStatus::Done);
        assert_eq!(restored.steps[1].error.as_deref(), Some("fail"));
    }

    #[test]
    fn test_plan_execute_state_default() {
        let state = PlanExecuteState::default();
        assert_eq!(state.execution_id, "");
        assert_eq!(state.phase, PipelinePhase::Research);
        assert_eq!(state.current_step_index, 0);
        assert_eq!(state.total_steps, 0);
        assert!(!state.needs_replan);
        assert!(state.steps.is_empty());
        assert!(state.plan_steps.is_empty());
    }

    #[test]
    fn test_pipeline_phase_transitions() {
        let phases = vec![
            PipelinePhase::Research,
            PipelinePhase::Clarification,
            PipelinePhase::PrdAssembly,
            PipelinePhase::PrdApproval,
            PipelinePhase::PrdDistill,
            PipelinePhase::Planning,
            PipelinePhase::PlanApproval,
            PipelinePhase::Execution,
            PipelinePhase::Judging,
            PipelinePhase::Replanning,
            PipelinePhase::Synthesis,
            PipelinePhase::Done,
        ];
        // All phases should serialize/deserialize correctly
        for phase in &phases {
            let json = serde_json::to_string(phase).unwrap();
            let restored: PipelinePhase = serde_json::from_str(&json).unwrap();
            assert_eq!(*phase, restored);
        }
        assert_eq!(phases.len(), 12);
    }

    #[test]
    fn test_step_result_status_variants() {
        let statuses = vec![
            StepStatus::Pending,
            StepStatus::Done,
            StepStatus::Error,
            StepStatus::Skipped,
            StepStatus::Retrying,
            StepStatus::Replanning,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).unwrap();
            let restored: StepStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(*status, restored);
        }
    }

    #[test]
    fn test_step_mut_auto_grows() {
        let mut state = PlanExecuteState::new("test");
        assert!(state.steps.is_empty());

        state.step_mut(5).status = StepStatus::Done;
        assert_eq!(state.steps.len(), 6);
        assert_eq!(state.steps[5].status, StepStatus::Done);
        // Intermediate steps should be default
        assert_eq!(state.steps[0].status, StepStatus::Pending);
    }

    #[test]
    fn test_current_step_accessor() {
        let mut state = PlanExecuteState::new("test");
        state.current_step_index = 2;
        state.step_mut(2).result = Some("hello".into());

        let current = state.current_step().unwrap();
        assert_eq!(current.result.as_deref(), Some("hello"));
    }

    #[test]
    fn test_approval_decision_serde() {
        let decisions = vec![
            ApprovalDecision::Pending,
            ApprovalDecision::Approved,
            ApprovalDecision::Rejected,
        ];
        for d in &decisions {
            let json = serde_json::to_string(d).unwrap();
            let restored: ApprovalDecision = serde_json::from_str(&json).unwrap();
            assert_eq!(*d, restored);
        }
    }
}
