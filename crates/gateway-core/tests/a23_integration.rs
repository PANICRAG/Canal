//! Integration tests for A23 Advanced Orchestration features.
//!
//! These tests verify the following A23 capabilities in an end-to-end fashion:
//!
//! 1. ParallelNode with ContinueOnError strategy
//! 2. DAG diamond execution via DagScheduler + execute_dag
//! 3. MemoryBridge hydrate/flush roundtrip
//! 4. ExecutionBudget tracking across sequential nodes
//! 5. Full orchestration stack (parallel + budget + recording observer + store)
//!
//! # Feature Gate
//!
//! These tests require the `full-orchestration` feature.

#![cfg(feature = "full-orchestration")]

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use gateway_core::graph::builder::GraphConfig;
use gateway_core::graph::execution_store::EventPayload;
use gateway_core::graph::memory_bridge::MemoryBridge;
use gateway_core::graph::{
    BudgetCheckResult, ClosureHandler, ErrorStrategy, ExecutionBudget, ExecutionMode,
    ExecutionStore, GraphExecutor, GraphState, NodeContext, NodeError, ParallelNode,
    RecordingObserver, StateGraphBuilder,
};
use gateway_core::memory::{MemoryCategory, MemoryEntry, UnifiedMemoryStore};

// ============================================================================
// Test State
// ============================================================================

/// A simple state for testing A23 graph execution.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct TestState {
    value: i32,
    steps: Vec<String>,
    done: bool,
}

impl GraphState for TestState {
    fn merge(&mut self, other: Self) {
        self.value += other.value;
        self.steps.extend(other.steps);
    }
}

// ============================================================================
// Test 1: ParallelNode with ContinueOnError
// ============================================================================

/// Verify that a ParallelNode with ErrorStrategy::ContinueOnError collects
/// partial results from successful branches while tolerating a failing branch.
///
/// Graph layout:
///   [parallel] (ContinueOnError)
///     ├── branch_a  (value += 10)
///     ├── branch_b  (returns error)
///     └── branch_c  (value += 30)
///
/// Expected final value: 0 (base) + 10 (branch_a) + 30 (branch_c) = 40
#[tokio::test]
async fn test_parallel_continue_on_error_e2e() {
    let graph = StateGraphBuilder::new()
        .add_node(
            "branch_a",
            ClosureHandler::new(|mut state: TestState, _ctx: &NodeContext| async move {
                state.steps.push("branch_a".into());
                state.value += 10;
                Ok(state)
            }),
        )
        .add_node(
            "branch_b",
            ClosureHandler::new(|_state: TestState, _ctx: &NodeContext| async move {
                Err(NodeError::HandlerError(
                    "branch_b deliberate failure".into(),
                ))
            }),
        )
        .add_node(
            "branch_c",
            ClosureHandler::new(|mut state: TestState, _ctx: &NodeContext| async move {
                state.steps.push("branch_c".into());
                state.value += 30;
                Ok(state)
            }),
        )
        .add_parallel_node(
            ParallelNode::new(
                "parallel",
                "ParallelContinueOnError",
                vec!["branch_a".into(), "branch_b".into(), "branch_c".into()],
            )
            .with_error_strategy(ErrorStrategy::ContinueOnError),
        )
        .set_entry("parallel")
        .set_terminal("parallel")
        .build()
        .expect("Failed to build graph with ContinueOnError parallel node");

    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 0,
        steps: vec![],
        done: false,
    };

    let result = executor
        .execute(initial)
        .await
        .expect("Execution should succeed despite branch_b failure");

    // Only branch_a (10) and branch_c (30) succeed; branch_b error is swallowed.
    assert_eq!(
        result.value, 40,
        "Expected 10 + 30 = 40 from successful branches"
    );
    assert!(result.steps.contains(&"branch_a".to_string()));
    assert!(result.steps.contains(&"branch_c".to_string()));
    // branch_b should NOT appear in steps since it errored out.
    assert!(!result.steps.contains(&"branch_b".to_string()));
}

// ============================================================================
// Test 2: DAG Diamond Execution via execute_dag
// ============================================================================

/// Verify diamond-shaped DAG execution: two independent nodes run in parallel
/// (wave 1) after a root node (wave 0), then a final node (wave 2) depends on both.
///
/// Graph layout:
///   entry → a
///   entry → b
///   a → end
///   b → end
///
/// Execution waves:
///   Wave 0: [entry]       — value goes 0 → 0 (no change by design)
///   Wave 1: [a, b]        — a adds 10, b adds 20 (parallel, each starts from base state)
///   Wave 2: [end]         — end adds 100
///
/// With merge semantics (value accumulates via +=), final value = base + a + b + end.
#[tokio::test]
async fn test_dag_diamond_execution_e2e() {
    let config = GraphConfig {
        dag_scheduling: true,
        ..Default::default()
    };

    let graph = StateGraphBuilder::new()
        .with_config(config)
        .add_node(
            "entry",
            ClosureHandler::new(|mut state: TestState, _ctx: &NodeContext| async move {
                state.steps.push("entry".into());
                // entry does not add to value — starts the pipeline
                Ok(state)
            }),
        )
        .add_node(
            "a",
            ClosureHandler::new(|mut state: TestState, _ctx: &NodeContext| async move {
                state.steps.push("a".into());
                state.value += 10;
                Ok(state)
            }),
        )
        .add_node(
            "b",
            ClosureHandler::new(|mut state: TestState, _ctx: &NodeContext| async move {
                state.steps.push("b".into());
                state.value += 20;
                Ok(state)
            }),
        )
        .add_node(
            "end",
            ClosureHandler::new(|mut state: TestState, _ctx: &NodeContext| async move {
                state.steps.push("end".into());
                state.value += 100;
                Ok(state)
            }),
        )
        .add_edge("entry", "a")
        .add_edge("entry", "b")
        .add_edge("a", "end")
        .add_edge("b", "end")
        .set_entry("entry")
        .set_terminal("end")
        .build()
        .expect("Failed to build diamond DAG");

    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 0,
        steps: vec![],
        done: false,
    };

    let result = executor
        .execute_dag(initial)
        .await
        .expect("DAG execution failed");

    // DAG merge semantics: base(0) + entry(0) merge + a(10) + b(20) merge + end(100)
    // Wave 0: entry runs, state value stays 0
    // Wave 1: a(0→10) and b(0→20) run in parallel, merge into base: 0 + 10 + 20 = 30
    // Wave 2: end(30→130)
    assert_eq!(
        result.value, 130,
        "Diamond DAG should produce 0 + 10 + 20 + 100 = 130"
    );
    assert!(result.steps.contains(&"entry".to_string()));
    assert!(result.steps.contains(&"a".to_string()));
    assert!(result.steps.contains(&"b".to_string()));
    assert!(result.steps.contains(&"end".to_string()));
}

// ============================================================================
// Test 3: MemoryBridge Hydrate and Flush Roundtrip
// ============================================================================

/// Verify the MemoryBridge hydrate/flush cycle with UnifiedMemoryStore.
///
/// Steps:
/// 1. Create a UnifiedMemoryStore and add a Preference entry.
/// 2. Create a MemoryBridge, hydrate() into an AgentGraphState.
/// 3. Verify working memory was populated.
/// 4. Set a step result, then flush() back to the store.
/// 5. Verify the store has the persisted step result.
#[tokio::test]
async fn test_memory_bridge_hydrate_flush_e2e() {
    use gateway_core::graph::adapters::AgentGraphState;

    let store = Arc::new(UnifiedMemoryStore::new());
    let user_id = Uuid::parse_str("00000000-0000-0000-0000-000000000042").unwrap();

    // 1. Seed the store with a preference entry
    let entry = MemoryEntry::new(
        "test_pref",
        MemoryCategory::Preference,
        "dark mode preferred",
    );
    store
        .store(user_id, entry)
        .await
        .expect("Failed to store preference");

    // 2. Hydrate an AgentGraphState from the store
    let bridge = MemoryBridge::with_defaults(store.clone(), user_id);
    let mut state = AgentGraphState::new("integration test task");
    let loaded = bridge.hydrate(&mut state).await;

    // 3. Verify at least 1 entry was loaded into working memory
    assert!(
        loaded >= 1,
        "Expected at least 1 entry to be hydrated, got {}",
        loaded
    );
    assert!(
        state.working_memory.contains_key("pref:test_pref"),
        "Expected working_memory to contain hydrated preference"
    );

    // 4. Execute a trivial transformation on the state, then flush
    state.set_step_result(
        "analysis",
        serde_json::json!({"score": 95, "status": "pass"}),
    );
    let persisted = bridge.flush(&state).await;
    assert!(
        persisted >= 1,
        "Expected at least 1 step result to be persisted"
    );

    // 5. Verify the store now has the ToolResult entry
    let tool_results = store
        .list_by_category(user_id, MemoryCategory::ToolResult)
        .await;
    assert!(
        !tool_results.is_empty(),
        "Expected ToolResult entries in the store after flush"
    );
    // Find our specific entry
    let found = tool_results.iter().any(|e| e.key.contains("analysis"));
    assert!(
        found,
        "Expected to find 'analysis' step result in persisted ToolResult entries"
    );
}

// ============================================================================
// Test 4: ExecutionBudget Terminates / Tracks Across Sequential Nodes
// ============================================================================

/// Verify that ExecutionBudget correctly tracks token consumption across
/// multiple sequential node executions, and that `is_exceeded()` returns
/// true when the global budget is surpassed.
///
/// Setup:
/// - Budget max: 100 tokens
/// - Node1 records 90 tokens
/// - Node2 records 20 tokens (total = 110 > 100)
///
/// After both records, `budget.consumed()` should be 110 and the second
/// record's BudgetCheckResult should be `Exceeded`.
#[tokio::test]
async fn test_budget_terminates_execution_e2e() {
    let budget = ExecutionBudget::new(100);

    // Simulate node1 recording 90 tokens
    let result1 = budget.record("node1", 90);
    assert!(
        !result1.is_exceeded(),
        "90 tokens should be within 100 budget"
    );
    assert_eq!(budget.consumed(), 90);
    assert_eq!(budget.remaining(), 10);

    // Simulate node2 recording 20 more tokens (total 110 > 100)
    let result2 = budget.record("node2", 20);
    assert!(result2.is_exceeded(), "110 tokens should exceed 100 budget");
    assert_eq!(budget.consumed(), 110);
    assert_eq!(budget.remaining(), 0); // saturating_sub

    // Also verify the exceeded scope is "global"
    if let BudgetCheckResult::Exceeded {
        consumed,
        limit,
        scope,
    } = result2
    {
        assert_eq!(consumed, 110);
        assert_eq!(limit, 100);
        assert_eq!(scope, "global");
    } else {
        panic!("Expected BudgetCheckResult::Exceeded");
    }

    // Verify the budget can still be attached to an executor (wiring check)
    let graph = StateGraphBuilder::new()
        .add_node(
            "final",
            ClosureHandler::new(|mut state: TestState, _ctx: &NodeContext| async move {
                state.steps.push("final".into());
                state.value = 999;
                Ok(state)
            }),
        )
        .set_entry("final")
        .set_terminal("final")
        .build()
        .expect("Failed to build simple graph");

    let budget2 = ExecutionBudget::new(100);
    let executor = GraphExecutor::new(graph).with_budget(budget2);
    let result = executor
        .execute(TestState {
            value: 0,
            steps: vec![],
            done: false,
        })
        .await
        .expect("Execution should succeed");
    assert_eq!(result.value, 999);
}

// ============================================================================
// Test 5: Full Orchestration Stack
// ============================================================================

/// Combine multiple A23 features: ParallelNode with ContinueOnError,
/// ExecutionBudget, RecordingObserver, and ExecutionStore.
///
/// Verifies:
/// - Events are recorded in the ExecutionStore
/// - Budget tracks consumption
/// - Parallel results are merged despite one branch failing
#[tokio::test]
async fn test_full_orchestration_stack() {
    // Set up ExecutionStore and start an execution
    let store = Arc::new(ExecutionStore::new(10));
    store
        .start_execution("full_stack_test", ExecutionMode::Graph("a23_test".into()))
        .await;

    // Create a RecordingObserver that writes to the store
    let observer = RecordingObserver::new(store.clone(), "full_stack_test");

    // Create a budget
    let budget = ExecutionBudget::new(5000);

    // Build a graph with parallel ContinueOnError + downstream node
    let graph = StateGraphBuilder::new()
        .add_node(
            "branch_ok",
            ClosureHandler::new(|mut state: TestState, _ctx: &NodeContext| async move {
                state.steps.push("branch_ok".into());
                state.value += 50;
                Ok(state)
            }),
        )
        .add_node(
            "branch_fail",
            ClosureHandler::new(|_state: TestState, _ctx: &NodeContext| async move {
                Err(NodeError::HandlerError("intentional failure".into()))
            }),
        )
        .add_parallel_node(
            ParallelNode::new(
                "par",
                "ParallelTest",
                vec!["branch_ok".into(), "branch_fail".into()],
            )
            .with_error_strategy(ErrorStrategy::ContinueOnError),
        )
        .add_node(
            "finalize",
            ClosureHandler::new(|mut state: TestState, _ctx: &NodeContext| async move {
                state.steps.push("finalize".into());
                state.value += 7;
                state.done = true;
                Ok(state)
            }),
        )
        .add_edge("par", "finalize")
        .set_entry("par")
        .set_terminal("finalize")
        .with_observer(observer)
        .build()
        .expect("Failed to build full orchestration graph");

    let executor = GraphExecutor::new(graph).with_budget(budget);
    let initial = TestState {
        value: 0,
        steps: vec![],
        done: false,
    };

    let result = executor
        .execute(initial)
        .await
        .expect("Full orchestration execution failed");

    // Verify parallel results merged (branch_ok succeeded, branch_fail skipped)
    assert_eq!(
        result.value, 57,
        "Expected 50 (branch_ok) + 7 (finalize) = 57"
    );
    assert!(result.steps.contains(&"branch_ok".to_string()));
    assert!(result.steps.contains(&"finalize".to_string()));
    assert!(result.done);

    // Verify events were recorded in the store
    let events = store.get_events("full_stack_test", 0, None);
    assert!(
        !events.is_empty(),
        "Expected events to be recorded in ExecutionStore"
    );

    // Check for a GraphStarted event
    let has_graph_started = events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::GraphStarted));
    assert!(
        has_graph_started,
        "Expected a GraphStarted event in the store"
    );

    // Check for at least one ParallelBranchFailed or ParallelPartialComplete event
    let has_parallel_event = events.iter().any(|e| {
        matches!(
            e.payload,
            EventPayload::ParallelBranchFailed { .. }
                | EventPayload::ParallelPartialComplete { .. }
        )
    });
    assert!(
        has_parallel_event,
        "Expected parallel failure/partial events in the store"
    );

    // Verify the execution was completed
    let summary = store.get_execution("full_stack_test");
    assert!(summary.is_some(), "Expected execution summary to exist");
}
