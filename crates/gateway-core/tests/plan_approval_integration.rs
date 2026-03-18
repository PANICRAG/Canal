//! Integration tests for plan approval (human-in-the-loop) in PlanExecute mode.
//!
//! These tests verify the complete flow:
//! 1. Graph creates a plan → approval_gate registers pending approval
//! 2. Background task sends approval decision
//! 3. Graph resumes based on decision (approve/reject/revise)
//!
//! # Feature Gate
//!
//! These tests require the `collaboration` feature (implies `graph`).

#![cfg(feature = "collaboration")]

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use gateway_core::collaboration::{PendingPlanApprovals, PlanApprovalDecision};
use gateway_core::graph::{
    ClosureHandler, ClosurePredicate, GraphExecutor, GraphState, NodeContext, StateGraphBuilder,
};

// ============================================================================
// Test State — simulates PlanExecute agent state
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct PlanTestState {
    goal: String,
    plan_steps: Vec<String>,
    plan_decision: String,
    plan_revision_feedback: Option<String>,
    revision_count: u32,
    executed_steps: Vec<String>,
    response: String,
    done: bool,
}

impl Default for PlanTestState {
    fn default() -> Self {
        Self {
            goal: String::new(),
            plan_steps: Vec::new(),
            plan_decision: String::new(),
            plan_revision_feedback: None,
            revision_count: 0,
            executed_steps: Vec::new(),
            response: String::new(),
            done: false,
        }
    }
}

impl GraphState for PlanTestState {
    fn merge(&mut self, other: Self) {
        self.executed_steps.extend(other.executed_steps);
        if !other.response.is_empty() {
            self.response = other.response;
        }
    }
}

// ============================================================================
// Helper: build a PlanExecute-like graph with approval_gate
// ============================================================================

fn build_approval_graph(
    store: Arc<PendingPlanApprovals>,
) -> gateway_core::graph::StateGraph<PlanTestState> {
    let store_clone = store.clone();

    StateGraphBuilder::new()
        // -- Planner node: generates plan (or revises with feedback)
        .add_node(
            "planner",
            ClosureHandler::new(|mut state: PlanTestState, _ctx: &NodeContext| async move {
                if let Some(feedback) = state.plan_revision_feedback.take() {
                    // Revision: modify plan based on feedback
                    state.plan_steps = vec![
                        format!("revised_step_1 (feedback: {})", feedback),
                        "revised_step_2".into(),
                    ];
                    state.revision_count += 1;
                } else {
                    // Initial plan
                    state.plan_steps = vec![
                        "step_1_search".into(),
                        "step_2_analyze".into(),
                        "step_3_summarize".into(),
                    ];
                }
                state.plan_decision.clear();
                Ok(state)
            }),
        )
        // -- Approval gate: pauses graph, waits for user decision
        .add_node(
            "approval_gate",
            ClosureHandler::new(move |mut state: PlanTestState, _ctx: &NodeContext| {
                let store = store_clone.clone();
                async move {
                    let request_id = Uuid::new_v4();
                    let session_id = Uuid::new_v4();

                    let rx = store.register(
                        request_id,
                        session_id,
                        state.goal.clone(),
                        Duration::from_secs(10),
                    );

                    // In a real system, we'd emit a PlanApprovalRequired SSE event here.
                    // For testing, the test code completes the approval from a background task.

                    // Store the request_id in state so the test can find it
                    // (in production, this goes via SSE; here we use a side channel)
                    state.response = format!("approval_request:{}", request_id);

                    // Wait for decision with timeout
                    let decision = match tokio::time::timeout(Duration::from_secs(5), rx).await {
                        Ok(Ok(d)) => d,
                        Ok(Err(_)) => {
                            state.plan_decision = "rejected".into();
                            return Ok(state);
                        }
                        Err(_) => {
                            state.plan_decision = "rejected".into();
                            return Ok(state);
                        }
                    };

                    match decision {
                        PlanApprovalDecision::Approve => {
                            state.plan_decision = "approved".into();
                        }
                        PlanApprovalDecision::ApproveWithEdits { edited_steps } => {
                            // Replace plan with edited steps
                            state.plan_steps = edited_steps
                                .iter()
                                .map(|s| s.action.clone())
                                .collect();
                            state.plan_decision = "approved".into();
                        }
                        PlanApprovalDecision::Revise { feedback } => {
                            state.plan_revision_feedback = Some(feedback);
                            state.plan_decision = "revise".into();
                        }
                        PlanApprovalDecision::Reject { reason: _ } => {
                            state.plan_decision = "rejected".into();
                        }
                    }

                    Ok(state)
                }
            }),
        )
        // -- Executor: runs the approved plan steps
        .add_node(
            "executor",
            ClosureHandler::new(|mut state: PlanTestState, _ctx: &NodeContext| async move {
                for step in &state.plan_steps {
                    state.executed_steps.push(format!("executed:{}", step));
                }
                state.response = format!("Completed {} steps", state.plan_steps.len());
                state.done = true;
                Ok(state)
            }),
        )
        // -- Rejection terminal: sets rejection message
        .add_node(
            "rejection",
            ClosureHandler::new(|mut state: PlanTestState, _ctx: &NodeContext| async move {
                state.response = "Plan rejected by user".into();
                state.done = true;
                Ok(state)
            }),
        )
        // -- Edges
        .add_edge("planner", "approval_gate")
        .add_conditional_edge(
            "approval_gate",
            ClosurePredicate::new(|state: &PlanTestState| {
                match state.plan_decision.as_str() {
                    "approved" => "executor".into(),
                    "revise" => "planner".into(),
                    _ => "rejection".into(),
                }
            }),
            vec![("executor", "executor"), ("planner", "planner"), ("rejection", "rejection")],
        )
        // -- Entry/terminal
        .set_entry("planner")
        .set_terminal("executor")
        .set_terminal("rejection")
        .build()
        .expect("Graph should build successfully")
}

// ============================================================================
// Test 1: Approve flow — plan generated, user approves, execution proceeds
// ============================================================================

#[tokio::test]
async fn test_approve_flow() {
    let store = Arc::new(PendingPlanApprovals::new());
    let graph = build_approval_graph(store.clone());
    let executor = GraphExecutor::new(graph);

    let initial_state = PlanTestState {
        goal: "Research quantum computing".into(),
        ..Default::default()
    };

    // Run graph in background — it will block at approval_gate
    let store_bg = store.clone();
    let handle = tokio::spawn(async move { executor.execute(initial_state).await });

    // Wait for approval to be registered
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(store_bg.pending_count(), 1);

    // Find the request_id (in production this comes from SSE)
    // The approval_gate stores it in state.response, but we can't read state mid-execution.
    // Instead, iterate the store's entries indirectly — we know there's exactly 1.
    // Use complete with the known pattern: iterate pending approvals.
    // Since PendingPlanApprovals doesn't expose keys directly, we'll get the ID
    // from the pending count + register pattern.
    // For integration test, we register a known ID before the graph runs.

    // Actually, we need a different approach. Let's use a shared request_id.
    // This is the integration test limitation — in production, the SSE event carries the ID.
    // Instead, let's use a broadcast approach where the test pre-populates a known ID.
    drop(handle);

    // Better approach: use a shared Arc<Mutex<Option<Uuid>>> as a side channel
    test_approve_flow_with_side_channel().await;
}

/// Helper that uses a side channel to pass request_id from approval_gate to test.
async fn test_approve_flow_with_side_channel() {
    let store = Arc::new(PendingPlanApprovals::new());
    let request_id = Arc::new(tokio::sync::Mutex::new(None::<Uuid>));

    let store_clone = store.clone();
    let rid_clone = request_id.clone();

    let graph = StateGraphBuilder::new()
        .add_node(
            "planner",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                state.plan_steps = vec!["search".into(), "analyze".into(), "summarize".into()];
                Ok(state)
            }),
        )
        .add_node(
            "approval_gate",
            ClosureHandler::new(move |mut state: PlanTestState, _: &NodeContext| {
                let store = store_clone.clone();
                let rid = rid_clone.clone();
                async move {
                    let req_id = Uuid::new_v4();
                    *rid.lock().await = Some(req_id);

                    let rx = store.register(
                        req_id,
                        Uuid::new_v4(),
                        state.goal.clone(),
                        Duration::from_secs(10),
                    );

                    match tokio::time::timeout(Duration::from_secs(5), rx).await {
                        Ok(Ok(PlanApprovalDecision::Approve)) => {
                            state.plan_decision = "approved".into();
                        }
                        _ => {
                            state.plan_decision = "rejected".into();
                        }
                    }
                    Ok(state)
                }
            }),
        )
        .add_node(
            "executor",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                for step in &state.plan_steps {
                    state.executed_steps.push(format!("executed:{}", step));
                }
                state.response = "done".into();
                state.done = true;
                Ok(state)
            }),
        )
        .add_node(
            "rejection",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                state.response = "rejected".into();
                state.done = true;
                Ok(state)
            }),
        )
        .add_edge("planner", "approval_gate")
        .add_conditional_edge(
            "approval_gate",
            ClosurePredicate::new(|state: &PlanTestState| {
                if state.plan_decision == "approved" {
                    "executor".into()
                } else {
                    "rejection".into()
                }
            }),
            vec![("executor", "executor"), ("rejection", "rejection")],
        )
        .set_entry("planner")
        .set_terminal("executor")
        .set_terminal("rejection")
        .build()
        .expect("Graph should build");

    let executor = GraphExecutor::new(graph);
    let initial = PlanTestState {
        goal: "test approval".into(),
        ..Default::default()
    };

    let store_bg = store.clone();
    let rid_bg = request_id.clone();

    // Run graph in background
    let handle = tokio::spawn(async move { executor.execute(initial).await });

    // Wait for approval_gate to register, then approve
    tokio::time::sleep(Duration::from_millis(200)).await;
    let req_id = rid_bg.lock().await.expect("Request ID should be set");
    store_bg
        .complete(&req_id, PlanApprovalDecision::Approve)
        .unwrap();

    let result = handle.await.unwrap().expect("Graph should complete");
    assert_eq!(result.plan_decision, "approved");
    assert_eq!(result.executed_steps.len(), 3);
    assert!(result.executed_steps[0].contains("search"));
    assert_eq!(result.response, "done");
    assert!(result.done);
}

// ============================================================================
// Test 2: Reject flow — plan generated, user rejects, graph terminates
// ============================================================================

#[tokio::test]
async fn test_reject_flow() {
    let store = Arc::new(PendingPlanApprovals::new());
    let request_id = Arc::new(tokio::sync::Mutex::new(None::<Uuid>));

    let store_clone = store.clone();
    let rid_clone = request_id.clone();

    let graph = StateGraphBuilder::new()
        .add_node(
            "planner",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                state.plan_steps = vec!["step_1".into(), "step_2".into()];
                Ok(state)
            }),
        )
        .add_node(
            "approval_gate",
            ClosureHandler::new(move |mut state: PlanTestState, _: &NodeContext| {
                let store = store_clone.clone();
                let rid = rid_clone.clone();
                async move {
                    let req_id = Uuid::new_v4();
                    *rid.lock().await = Some(req_id);

                    let rx = store.register(
                        req_id,
                        Uuid::new_v4(),
                        state.goal.clone(),
                        Duration::from_secs(10),
                    );

                    match tokio::time::timeout(Duration::from_secs(5), rx).await {
                        Ok(Ok(PlanApprovalDecision::Reject { .. })) => {
                            state.plan_decision = "rejected".into();
                        }
                        _ => {
                            state.plan_decision = "rejected".into();
                        }
                    }
                    Ok(state)
                }
            }),
        )
        .add_node(
            "executor",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                state.executed_steps.push("should_not_run".into());
                state.done = true;
                Ok(state)
            }),
        )
        .add_node(
            "rejection",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                state.response = "Plan rejected".into();
                state.done = true;
                Ok(state)
            }),
        )
        .add_edge("planner", "approval_gate")
        .add_conditional_edge(
            "approval_gate",
            ClosurePredicate::new(|state: &PlanTestState| {
                if state.plan_decision == "approved" {
                    "executor".into()
                } else {
                    "rejection".into()
                }
            }),
            vec![("executor", "executor"), ("rejection", "rejection")],
        )
        .set_entry("planner")
        .set_terminal("executor")
        .set_terminal("rejection")
        .build()
        .expect("Graph should build");

    let executor = GraphExecutor::new(graph);
    let initial = PlanTestState {
        goal: "test rejection".into(),
        ..Default::default()
    };

    let store_bg = store.clone();
    let rid_bg = request_id.clone();

    let handle = tokio::spawn(async move { executor.execute(initial).await });

    // Wait, then reject
    tokio::time::sleep(Duration::from_millis(200)).await;
    let req_id = rid_bg.lock().await.expect("Request ID should be set");
    store_bg
        .complete(
            &req_id,
            PlanApprovalDecision::Reject {
                reason: Some("not needed".into()),
            },
        )
        .unwrap();

    let result = handle.await.unwrap().expect("Graph should complete");
    assert_eq!(result.plan_decision, "rejected");
    assert!(
        result.executed_steps.is_empty(),
        "Executor should not have run"
    );
    assert_eq!(result.response, "Plan rejected");
}

// ============================================================================
// Test 3: ApproveWithEdits — user modifies steps before approving
// ============================================================================

#[tokio::test]
async fn test_approve_with_edits_flow() {
    use gateway_core::collaboration::planner::{PlanStep, StepDependency, ToolCategory};

    let store = Arc::new(PendingPlanApprovals::new());
    let request_id = Arc::new(tokio::sync::Mutex::new(None::<Uuid>));

    let store_clone = store.clone();
    let rid_clone = request_id.clone();

    let graph = StateGraphBuilder::new()
        .add_node(
            "planner",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                state.plan_steps = vec!["original_step_1".into(), "original_step_2".into()];
                Ok(state)
            }),
        )
        .add_node(
            "approval_gate",
            ClosureHandler::new(move |mut state: PlanTestState, _: &NodeContext| {
                let store = store_clone.clone();
                let rid = rid_clone.clone();
                async move {
                    let req_id = Uuid::new_v4();
                    *rid.lock().await = Some(req_id);

                    let rx = store.register(
                        req_id,
                        Uuid::new_v4(),
                        state.goal.clone(),
                        Duration::from_secs(10),
                    );

                    match tokio::time::timeout(Duration::from_secs(5), rx).await {
                        Ok(Ok(PlanApprovalDecision::ApproveWithEdits { edited_steps })) => {
                            state.plan_steps =
                                edited_steps.iter().map(|s| s.action.clone()).collect();
                            state.plan_decision = "approved".into();
                        }
                        Ok(Ok(PlanApprovalDecision::Approve)) => {
                            state.plan_decision = "approved".into();
                        }
                        _ => {
                            state.plan_decision = "rejected".into();
                        }
                    }
                    Ok(state)
                }
            }),
        )
        .add_node(
            "executor",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                for step in &state.plan_steps {
                    state.executed_steps.push(format!("executed:{}", step));
                }
                state.response = "done".into();
                state.done = true;
                Ok(state)
            }),
        )
        .add_node(
            "rejection",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                state.response = "rejected".into();
                state.done = true;
                Ok(state)
            }),
        )
        .add_edge("planner", "approval_gate")
        .add_conditional_edge(
            "approval_gate",
            ClosurePredicate::new(|state: &PlanTestState| {
                if state.plan_decision == "approved" {
                    "executor".into()
                } else {
                    "rejection".into()
                }
            }),
            vec![("executor", "executor"), ("rejection", "rejection")],
        )
        .set_entry("planner")
        .set_terminal("executor")
        .set_terminal("rejection")
        .build()
        .expect("Graph should build");

    let executor = GraphExecutor::new(graph);
    let initial = PlanTestState {
        goal: "test edits".into(),
        ..Default::default()
    };

    let store_bg = store.clone();
    let rid_bg = request_id.clone();

    let handle = tokio::spawn(async move { executor.execute(initial).await });

    // Wait, then approve with edits
    tokio::time::sleep(Duration::from_millis(200)).await;
    let req_id = rid_bg.lock().await.expect("Request ID should be set");

    let edited = vec![
        PlanStep {
            id: 1,
            action: "edited_search".into(),
            tool_category: ToolCategory::Search,
            dependency: StepDependency::None,
            expected_output: None,
            executor_agent: None,
            executor_model: None,
            requires_visual_verification: None,
            prd_content: None,
            executor_type: None,
        },
        PlanStep {
            id: 2,
            action: "edited_browser".into(),
            tool_category: ToolCategory::Browser,
            dependency: StepDependency::Sequential,
            expected_output: Some("Page content".into()),
            executor_agent: None,
            executor_model: None,
            requires_visual_verification: None,
            prd_content: None,
            executor_type: None,
        },
    ];

    store_bg
        .complete(
            &req_id,
            PlanApprovalDecision::ApproveWithEdits {
                edited_steps: edited,
            },
        )
        .unwrap();

    let result = handle.await.unwrap().expect("Graph should complete");
    assert_eq!(result.plan_decision, "approved");
    // Steps should be the edited ones, not the originals
    assert_eq!(result.plan_steps, vec!["edited_search", "edited_browser"]);
    assert_eq!(result.executed_steps.len(), 2);
    assert!(result.executed_steps[0].contains("edited_search"));
    assert!(result.executed_steps[1].contains("edited_browser"));
}

// ============================================================================
// Test 4: Revise flow — user sends feedback, planner regenerates, re-approve
// ============================================================================

#[tokio::test]
async fn test_revise_flow() {
    let store = Arc::new(PendingPlanApprovals::new());
    // Track which round we're on so we can respond differently
    let round = Arc::new(std::sync::atomic::AtomicU32::new(0));
    // Collect all request IDs
    let request_ids = Arc::new(tokio::sync::Mutex::new(Vec::<Uuid>::new()));

    let store_clone = store.clone();
    let rids_clone = request_ids.clone();

    let graph = StateGraphBuilder::new()
        .add_node(
            "planner",
            ClosureHandler::new({
                let round = round.clone();
                move |mut state: PlanTestState, _: &NodeContext| {
                    let r = round.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    async move {
                        if let Some(feedback) = state.plan_revision_feedback.take() {
                            state.plan_steps = vec![format!("revised_{}_based_on:{}", r, feedback)];
                            state.revision_count += 1;
                        } else {
                            state.plan_steps = vec!["initial_step".into()];
                        }
                        state.plan_decision.clear();
                        Ok(state)
                    }
                }
            }),
        )
        .add_node(
            "approval_gate",
            ClosureHandler::new(move |mut state: PlanTestState, _: &NodeContext| {
                let store = store_clone.clone();
                let rids = rids_clone.clone();
                async move {
                    let req_id = Uuid::new_v4();
                    rids.lock().await.push(req_id);

                    let rx = store.register(
                        req_id,
                        Uuid::new_v4(),
                        state.goal.clone(),
                        Duration::from_secs(10),
                    );

                    match tokio::time::timeout(Duration::from_secs(5), rx).await {
                        Ok(Ok(decision)) => match decision {
                            PlanApprovalDecision::Approve => {
                                state.plan_decision = "approved".into();
                            }
                            PlanApprovalDecision::Revise { feedback } => {
                                state.plan_revision_feedback = Some(feedback);
                                state.plan_decision = "revise".into();
                            }
                            PlanApprovalDecision::Reject { .. } => {
                                state.plan_decision = "rejected".into();
                            }
                            PlanApprovalDecision::ApproveWithEdits { edited_steps } => {
                                state.plan_steps =
                                    edited_steps.iter().map(|s| s.action.clone()).collect();
                                state.plan_decision = "approved".into();
                            }
                        },
                        _ => {
                            state.plan_decision = "rejected".into();
                        }
                    }
                    Ok(state)
                }
            }),
        )
        .add_node(
            "executor",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                for step in &state.plan_steps {
                    state.executed_steps.push(format!("executed:{}", step));
                }
                state.response = "done".into();
                state.done = true;
                Ok(state)
            }),
        )
        .add_node(
            "rejection",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                state.response = "rejected".into();
                state.done = true;
                Ok(state)
            }),
        )
        .add_edge("planner", "approval_gate")
        .add_conditional_edge(
            "approval_gate",
            ClosurePredicate::new(|state: &PlanTestState| match state.plan_decision.as_str() {
                "approved" => "executor".into(),
                "revise" => "planner".into(),
                _ => "rejection".into(),
            }),
            vec![
                ("executor", "executor"),
                ("planner", "planner"),
                ("rejection", "rejection"),
            ],
        )
        .set_entry("planner")
        .set_terminal("executor")
        .set_terminal("rejection")
        .build()
        .expect("Graph should build");

    let executor = GraphExecutor::new(graph);
    let initial = PlanTestState {
        goal: "test revision".into(),
        ..Default::default()
    };

    let store_bg = store.clone();
    let rids_bg = request_ids.clone();

    let handle = tokio::spawn(async move { executor.execute(initial).await });

    // Round 1: Revise
    tokio::time::sleep(Duration::from_millis(200)).await;
    {
        let rids = rids_bg.lock().await;
        assert_eq!(rids.len(), 1, "First approval should be registered");
        store_bg
            .complete(
                &rids[0],
                PlanApprovalDecision::Revise {
                    feedback: "use browser".into(),
                },
            )
            .unwrap();
    }

    // Round 2: Approve the revised plan
    tokio::time::sleep(Duration::from_millis(300)).await;
    {
        let rids = rids_bg.lock().await;
        assert_eq!(
            rids.len(),
            2,
            "Second approval should be registered after revision"
        );
        store_bg
            .complete(&rids[1], PlanApprovalDecision::Approve)
            .unwrap();
    }

    let result = handle.await.unwrap().expect("Graph should complete");
    assert_eq!(result.plan_decision, "approved");
    assert_eq!(result.revision_count, 1);
    // The plan should contain the revised step
    assert!(result.plan_steps[0].contains("revised"));
    assert!(result.plan_steps[0].contains("use browser"));
    assert_eq!(result.executed_steps.len(), 1);
    assert!(result.executed_steps[0].contains("revised"));
}

// ============================================================================
// Test 5: Timeout flow — no decision within timeout → auto-reject
// ============================================================================

#[tokio::test]
async fn test_timeout_auto_reject() {
    let store = Arc::new(PendingPlanApprovals::new());
    let store_clone = store.clone();

    let graph = StateGraphBuilder::new()
        .add_node(
            "planner",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                state.plan_steps = vec!["step_1".into()];
                Ok(state)
            }),
        )
        .add_node(
            "approval_gate",
            ClosureHandler::new(move |mut state: PlanTestState, _: &NodeContext| {
                let store = store_clone.clone();
                async move {
                    let req_id = Uuid::new_v4();
                    let rx = store.register(
                        req_id,
                        Uuid::new_v4(),
                        state.goal.clone(),
                        Duration::from_millis(100), // Very short timeout for test
                    );

                    // Wait with short timeout — will expire
                    match tokio::time::timeout(Duration::from_millis(200), rx).await {
                        Ok(Ok(PlanApprovalDecision::Approve)) => {
                            state.plan_decision = "approved".into();
                        }
                        _ => {
                            state.plan_decision = "rejected".into();
                        }
                    }
                    Ok(state)
                }
            }),
        )
        .add_node(
            "executor",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                state.executed_steps.push("should_not_run".into());
                state.done = true;
                Ok(state)
            }),
        )
        .add_node(
            "rejection",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                state.response = "Timed out".into();
                state.done = true;
                Ok(state)
            }),
        )
        .add_edge("planner", "approval_gate")
        .add_conditional_edge(
            "approval_gate",
            ClosurePredicate::new(|state: &PlanTestState| {
                if state.plan_decision == "approved" {
                    "executor".into()
                } else {
                    "rejection".into()
                }
            }),
            vec![("executor", "executor"), ("rejection", "rejection")],
        )
        .set_entry("planner")
        .set_terminal("executor")
        .set_terminal("rejection")
        .build()
        .expect("Graph should build");

    let executor = GraphExecutor::new(graph);
    let initial = PlanTestState {
        goal: "test timeout".into(),
        ..Default::default()
    };

    // No one approves — graph should auto-reject
    let result = executor
        .execute(initial)
        .await
        .expect("Graph should complete");
    assert_eq!(result.plan_decision, "rejected");
    assert!(result.executed_steps.is_empty());
    assert_eq!(result.response, "Timed out");
}

// ============================================================================
// Test 6: Concurrent approvals — multiple plans pending simultaneously
// ============================================================================

#[tokio::test]
async fn test_concurrent_approvals() {
    let store = Arc::new(PendingPlanApprovals::new());

    // Register 3 approvals
    let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
    let mut receivers = Vec::new();

    for (i, id) in ids.iter().enumerate() {
        let rx = store.register(
            *id,
            Uuid::new_v4(),
            format!("plan_{}", i),
            Duration::from_secs(10),
        );
        receivers.push(rx);
    }

    assert_eq!(store.pending_count(), 3);

    // Complete them in reverse order
    store
        .complete(&ids[2], PlanApprovalDecision::Approve)
        .unwrap();
    store
        .complete(&ids[0], PlanApprovalDecision::Reject { reason: None })
        .unwrap();
    store
        .complete(
            &ids[1],
            PlanApprovalDecision::Revise {
                feedback: "change".into(),
            },
        )
        .unwrap();

    assert_eq!(store.pending_count(), 0);

    // Verify each receiver got the correct decision
    let d0 = receivers.remove(0).await.unwrap();
    assert!(matches!(d0, PlanApprovalDecision::Reject { .. }));

    let d1 = receivers.remove(0).await.unwrap();
    assert!(matches!(d1, PlanApprovalDecision::Revise { .. }));

    let d2 = receivers.remove(0).await.unwrap();
    assert!(matches!(d2, PlanApprovalDecision::Approve));
}

// ============================================================================
// Test 7: Backward compatibility — no approval store → graph runs without pause
// ============================================================================

#[tokio::test]
async fn test_backward_compat_no_approval_store() {
    // Graph without approval_gate — simulates the old PlanExecute flow
    let graph = StateGraphBuilder::new()
        .add_node(
            "planner",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                state.plan_steps = vec!["auto_step_1".into(), "auto_step_2".into()];
                state.plan_decision = "approved".into(); // auto-approve
                Ok(state)
            }),
        )
        .add_node(
            "executor",
            ClosureHandler::new(|mut state: PlanTestState, _: &NodeContext| async move {
                for step in &state.plan_steps {
                    state.executed_steps.push(format!("executed:{}", step));
                }
                state.response = "auto-done".into();
                state.done = true;
                Ok(state)
            }),
        )
        .add_edge("planner", "executor")
        .set_entry("planner")
        .set_terminal("executor")
        .build()
        .expect("Graph should build");

    let executor = GraphExecutor::new(graph);
    let initial = PlanTestState {
        goal: "backward compat".into(),
        ..Default::default()
    };

    let result = executor
        .execute(initial)
        .await
        .expect("Graph should complete");
    assert_eq!(result.plan_decision, "approved");
    assert_eq!(result.executed_steps.len(), 2);
    assert_eq!(result.response, "auto-done");
}

// ============================================================================
// Test 8: PlanStepReview enrichment
// ============================================================================

#[test]
fn test_plan_step_review_enrichment() {
    use gateway_core::collaboration::approval::{max_risk_level, PlanStepReview};
    use gateway_core::collaboration::planner::{PlanStep, StepDependency, ToolCategory};

    let steps = vec![
        PlanStep {
            id: 1,
            action: "Search for papers".into(),
            tool_category: ToolCategory::Search,
            dependency: StepDependency::None,
            expected_output: Some("List of papers".into()),
            executor_agent: None,
            executor_model: None,
            requires_visual_verification: None,
            prd_content: None,
            executor_type: None,
        },
        PlanStep {
            id: 2,
            action: "Open browser to read".into(),
            tool_category: ToolCategory::Browser,
            dependency: StepDependency::Sequential,
            expected_output: Some("Page content".into()),
            executor_agent: None,
            executor_model: None,
            requires_visual_verification: None,
            prd_content: None,
            executor_type: None,
        },
        PlanStep {
            id: 3,
            action: "Summarize findings".into(),
            tool_category: ToolCategory::Llm,
            dependency: StepDependency::Sequential,
            expected_output: None,
            executor_agent: None,
            executor_model: None,
            requires_visual_verification: None,
            prd_content: None,
            executor_type: None,
        },
    ];

    // Verify risk classification
    assert_eq!(max_risk_level(&steps), "high"); // Browser step makes it high

    // Verify step reviews
    let reviews: Vec<PlanStepReview> = steps
        .iter()
        .map(|s| PlanStepReview::from_plan_step(s, Some("qwen-turbo".into())))
        .collect();

    assert_eq!(reviews[0].risk_level, "low"); // Search
    assert_eq!(reviews[1].risk_level, "high"); // Browser
    assert_eq!(reviews[2].risk_level, "low"); // LLM
    assert_eq!(reviews[0].estimated_model, Some("qwen-turbo".into()));
}
