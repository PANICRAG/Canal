//! Cross-integration tests for A22 (Debug System) + A23 (Advanced Orchestration).
//!
//! These tests verify that:
//! - A23 execution events (ParallelPartialComplete, ParallelBranchFailed) are
//!   recorded by the A22 RecordingObserver into the ExecutionStore.
//! - The full debug pipeline (store -> record -> query -> subscribe) works end-to-end.
//! - Event ordering guarantees (monotonic sequence numbers) hold across complex
//!   multi-node graph executions.
//!
//! # Feature Gate
//!
//! These tests require the `full-orchestration` feature.

#![cfg(feature = "full-orchestration")]

use gateway_core::graph::{
    ClosureHandler, ErrorStrategy, EventPayload, ExecutionMode, ExecutionStatus, ExecutionStore,
    GraphExecutor, GraphState, NodeContext, NodeError, ParallelNode, RecordingObserver,
    StateGraphBuilder,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ============================================================================
// Test State
// ============================================================================

/// A simple state for testing graph execution with debug recording.
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
// Test 1: A23 events recorded by A22
// ============================================================================

/// Verify that A23-specific events (ParallelPartialComplete, ParallelBranchFailed)
/// are captured by RecordingObserver and stored in ExecutionStore.
///
/// This test creates a graph with:
/// - A ParallelNode (ContinueOnError) containing 3 branches (2 succeed, 1 fails)
/// - A sequential node after the parallel node
///
/// After execution, the ExecutionStore should contain:
/// - GraphStarted
/// - NodeEntered / NodeCompleted for the parallel node
/// - ParallelBranchFailed for the failing branch
/// - ParallelPartialComplete (succeeded=2, failed=1)
/// - NodeEntered / NodeCompleted for the sequential tail node
/// - GraphCompleted
#[tokio::test]
async fn test_a23_events_recorded_by_a22() {
    let store = Arc::new(ExecutionStore::new(10));
    let execution_id = "a23_events_test";

    // Start the execution in the store BEFORE graph runs
    store
        .start_execution(execution_id, ExecutionMode::Graph("parallel_test".into()))
        .await;

    // Build graph: parallel(ok_a, ok_b, fail_c) → finish
    let observer = RecordingObserver::new(store.clone(), execution_id);

    let graph = StateGraphBuilder::new()
        .add_node(
            "ok_a",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("ok_a".into());
                state.value += 10;
                Ok(state)
            }),
        )
        .add_node(
            "ok_b",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("ok_b".into());
                state.value += 20;
                Ok(state)
            }),
        )
        .add_node(
            "fail_c",
            ClosureHandler::new(|_state: TestState, _: &NodeContext| async move {
                Err(NodeError::HandlerError("deliberate branch failure".into()))
            }),
        )
        .add_parallel_node(
            ParallelNode::new(
                "parallel",
                "Parallel",
                vec!["ok_a".into(), "fail_c".into(), "ok_b".into()],
            )
            .with_error_strategy(ErrorStrategy::ContinueOnError),
        )
        .add_node(
            "finish",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("finish".into());
                state.done = true;
                Ok(state)
            }),
        )
        .add_edge("parallel", "finish")
        .set_entry("parallel")
        .set_terminal("finish")
        .with_observer(observer)
        .build()
        .expect("Failed to build graph");

    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 0,
        steps: vec![],
        done: false,
    };

    let result = executor.execute(initial).await.expect("Execution failed");

    // Verify graph result: 2 successful branches merged (10 + 20 = 30), finish adds nothing
    assert_eq!(result.value, 30);
    assert!(result.steps.contains(&"ok_a".to_string()));
    assert!(result.steps.contains(&"ok_b".to_string()));
    assert!(result.steps.contains(&"finish".to_string()));
    assert!(result.done);

    // Query events from the store
    let events = store.get_events(execution_id, 0, None);
    assert!(
        !events.is_empty(),
        "ExecutionStore should have recorded events"
    );

    // Verify GraphStarted is present
    let has_graph_started = events
        .iter()
        .any(|e| matches!(e.payload, EventPayload::GraphStarted));
    assert!(has_graph_started, "GraphStarted event should be present");

    // Verify NodeEntered events are present
    let node_entered_count = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::NodeEntered { .. }))
        .count();
    assert!(
        node_entered_count >= 2,
        "At least 2 NodeEntered events expected (parallel + finish), got {}",
        node_entered_count
    );

    // Verify NodeCompleted events are present
    let node_completed_count = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::NodeCompleted { .. }))
        .count();
    assert!(
        node_completed_count >= 2,
        "At least 2 NodeCompleted events expected, got {}",
        node_completed_count
    );

    // Verify A23-specific: ParallelBranchFailed
    let branch_failed_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::ParallelBranchFailed { .. }))
        .collect();
    assert!(
        !branch_failed_events.is_empty(),
        "ParallelBranchFailed event should be present for fail_c"
    );
    // Verify the branch_id in the failure event
    if let EventPayload::ParallelBranchFailed {
        ref branch_id,
        ref error,
        ..
    } = branch_failed_events[0].payload
    {
        assert_eq!(branch_id, "fail_c");
        assert!(error.contains("deliberate branch failure"));
    }

    // Verify A23-specific: ParallelPartialComplete
    let partial_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::ParallelPartialComplete { .. }))
        .collect();
    assert!(
        !partial_events.is_empty(),
        "ParallelPartialComplete event should be present"
    );
    if let EventPayload::ParallelPartialComplete {
        succeeded, failed, ..
    } = partial_events[0].payload
    {
        assert_eq!(succeeded, 2, "2 branches should have succeeded");
        assert_eq!(failed, 1, "1 branch should have failed");
    }

    // Verify GraphCompleted is present (added by RecordingObserver::on_graph_complete
    // which calls store.complete_execution())
    let summary = store.get_execution(execution_id).unwrap();
    assert_eq!(
        summary.status,
        ExecutionStatus::Completed,
        "Execution should be marked completed"
    );
}

// ============================================================================
// Test 2: Full debug pipeline end-to-end
// ============================================================================

/// Verify the complete debug pipeline:
/// 1. Create ExecutionStore and start an execution
/// 2. Build graph with RecordingObserver attached
/// 3. Execute graph
/// 4. Query events via get_events()
/// 5. Subscribe to the execution and verify replay events
/// 6. Verify execution shows as completed via get_execution()
#[tokio::test]
async fn test_debug_pipeline_end_to_end() {
    let store = Arc::new(ExecutionStore::new(10));
    let execution_id = "debug_pipeline_test";

    // Step 1: Start execution in store
    store
        .start_execution(execution_id, ExecutionMode::Direct)
        .await;

    // Verify it starts as Running
    let summary = store.get_execution(execution_id).unwrap();
    assert_eq!(summary.status, ExecutionStatus::Running);

    // Step 2: Build a 3-node linear graph with RecordingObserver
    let observer = RecordingObserver::new(store.clone(), execution_id);

    let graph = StateGraphBuilder::new()
        .add_node(
            "step_1",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("step_1".into());
                state.value += 1;
                Ok(state)
            }),
        )
        .add_node(
            "step_2",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("step_2".into());
                state.value += 2;
                Ok(state)
            }),
        )
        .add_node(
            "step_3",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("step_3".into());
                state.value += 3;
                state.done = true;
                Ok(state)
            }),
        )
        .add_edge("step_1", "step_2")
        .add_edge("step_2", "step_3")
        .set_entry("step_1")
        .set_terminal("step_3")
        .with_observer(observer)
        .build()
        .expect("Failed to build graph");

    // Step 3: Execute graph
    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 0,
        steps: vec![],
        done: false,
    };
    let result = executor.execute(initial).await.expect("Execution failed");
    assert_eq!(result.value, 6);
    assert_eq!(result.steps, vec!["step_1", "step_2", "step_3"]);

    // Step 4: Query events via get_events()
    let all_events = store.get_events(execution_id, 0, None);
    assert!(!all_events.is_empty(), "Events should have been recorded");

    // Verify event types: GraphStarted, 3x(NodeEntered, NodeCompleted), 2x EdgeTraversed, GraphCompleted
    let graph_started = all_events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::GraphStarted))
        .count();
    assert_eq!(graph_started, 1, "Exactly 1 GraphStarted event");

    let node_entered = all_events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::NodeEntered { .. }))
        .count();
    assert_eq!(node_entered, 3, "3 NodeEntered events (one per node)");

    let node_completed = all_events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::NodeCompleted { .. }))
        .count();
    assert_eq!(node_completed, 3, "3 NodeCompleted events (one per node)");

    let edge_traversed = all_events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::EdgeTraversed { .. }))
        .count();
    assert_eq!(
        edge_traversed, 2,
        "2 EdgeTraversed events (step_1->step_2, step_2->step_3)"
    );

    // Verify get_events with offset and limit
    let first_3 = store.get_events(execution_id, 0, Some(3));
    assert_eq!(first_3.len(), 3);

    let from_offset_2 = store.get_events(execution_id, 2, Some(2));
    assert_eq!(from_offset_2.len(), 2);
    assert_eq!(from_offset_2[0].seq, all_events[2].seq);

    // Step 5: Subscribe and verify replay
    // subscribe() returns (Receiver, Vec<replay_events>)
    let (_rx, replay) = store.subscribe(execution_id, 100);
    assert_eq!(
        replay.len(),
        all_events.len(),
        "Replay with large count should return all events"
    );
    // Verify replay events match stored events
    for (i, (replay_event, stored_event)) in replay.iter().zip(all_events.iter()).enumerate() {
        assert_eq!(
            replay_event.seq, stored_event.seq,
            "Replay event seq mismatch at index {}",
            i
        );
    }

    // Step 6: Verify execution is marked completed
    let final_summary = store.get_execution(execution_id).unwrap();
    assert_eq!(
        final_summary.status,
        ExecutionStatus::Completed,
        "Execution should be completed after graph finishes"
    );
    assert!(final_summary.event_count > 0, "Event count should be > 0");
    assert!(
        final_summary.duration_ms.is_some(),
        "Duration should be set after completion"
    );
}

// ============================================================================
// Test 3: Recording observer event ordering
// ============================================================================

/// Execute a multi-node graph (4+ nodes, mix of sequential and parallel) and verify:
/// 1. All recorded events have strictly monotonically increasing sequence numbers.
/// 2. Event count matches expected (GraphStarted + N*NodeEntered + N*NodeCompleted
///    + edges + GraphCompleted).
#[tokio::test]
async fn test_recording_observer_event_ordering() {
    let store = Arc::new(ExecutionStore::new(10));
    let execution_id = "ordering_test";

    store
        .start_execution(execution_id, ExecutionMode::Graph("ordering".into()))
        .await;

    let observer = RecordingObserver::new(store.clone(), execution_id);

    // Build a graph: start → parallel(branch_a, branch_b) → merge → finish
    // This gives us: 5 "logical" nodes (start, parallel, branch_a, branch_b, merge, finish)
    // but the parallel node is a single node containing branch_a and branch_b.
    // Actually: start, branch_a, branch_b (inside parallel), merge, finish = 5 node IDs,
    // plus 1 parallel node ID.
    //
    // Execution path:
    //   start → parallel(branch_a, branch_b) → merge → finish
    //
    // Expected observer events:
    //   GraphStarted
    //   NodeEntered(start), NodeCompleted(start)
    //   EdgeTraversed(start → parallel)
    //   NodeEntered(parallel), NodeCompleted(parallel)
    //   EdgeTraversed(parallel → merge)
    //   NodeEntered(merge), NodeCompleted(merge)
    //   EdgeTraversed(merge → finish)
    //   NodeEntered(finish), NodeCompleted(finish)
    //   GraphCompleted (via store.complete_execution from RecordingObserver)

    let graph = StateGraphBuilder::new()
        .add_node(
            "start",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("start".into());
                state.value += 1;
                Ok(state)
            }),
        )
        .add_node(
            "branch_a",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("branch_a".into());
                state.value += 10;
                Ok(state)
            }),
        )
        .add_node(
            "branch_b",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("branch_b".into());
                state.value += 20;
                Ok(state)
            }),
        )
        .add_parallel_node(ParallelNode::new(
            "parallel",
            "Parallel",
            vec!["branch_a".into(), "branch_b".into()],
        ))
        .add_node(
            "merge",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("merge".into());
                state.value += 100;
                Ok(state)
            }),
        )
        .add_node(
            "finish",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("finish".into());
                state.done = true;
                Ok(state)
            }),
        )
        .add_edge("start", "parallel")
        .add_edge("parallel", "merge")
        .add_edge("merge", "finish")
        .set_entry("start")
        .set_terminal("finish")
        .with_observer(observer)
        .build()
        .expect("Failed to build graph");

    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 0,
        steps: vec![],
        done: false,
    };

    let result = executor.execute(initial).await.expect("Execution failed");
    assert!(result.done);

    // Retrieve all events
    let events = store.get_events(execution_id, 0, None);
    assert!(
        events.len() >= 5,
        "Should have at least 5 events, got {}",
        events.len()
    );

    // Verify strictly monotonically increasing sequence numbers
    for i in 1..events.len() {
        assert!(
            events[i].seq > events[i - 1].seq,
            "Events must have strictly increasing seq numbers: event[{}].seq={} should be > event[{}].seq={}",
            i, events[i].seq, i - 1, events[i - 1].seq
        );
    }

    // Verify first seq starts at 0
    assert_eq!(events[0].seq, 0, "First event should have seq=0");

    // Verify consecutive: each seq = previous + 1 (no gaps)
    for i in 1..events.len() {
        assert_eq!(
            events[i].seq,
            events[i - 1].seq + 1,
            "Sequence numbers should be consecutive: event[{}].seq={} should be event[{}].seq+1={}",
            i,
            events[i].seq,
            i - 1,
            events[i - 1].seq + 1,
        );
    }

    // Count event types
    let graph_started_count = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::GraphStarted))
        .count();
    let node_entered_count = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::NodeEntered { .. }))
        .count();
    let node_completed_count = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::NodeCompleted { .. }))
        .count();
    let edge_traversed_count = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::EdgeTraversed { .. }))
        .count();
    let graph_completed_count = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::GraphCompleted { .. }))
        .count();

    // The execution path visits 4 nodes: start, parallel, merge, finish
    // Expected events:
    //   1 GraphStarted
    //   4 NodeEntered (start, parallel, merge, finish)
    //   4 NodeCompleted (start, parallel, merge, finish)
    //   3 EdgeTraversed (start→parallel, parallel→merge, merge→finish)
    //   1 GraphCompleted
    // Total = 1 + 4 + 4 + 3 + 1 = 13

    assert_eq!(graph_started_count, 1, "Exactly 1 GraphStarted");
    assert_eq!(
        node_entered_count, 4,
        "4 NodeEntered (start, parallel, merge, finish)"
    );
    assert_eq!(
        node_completed_count, 4,
        "4 NodeCompleted (start, parallel, merge, finish)"
    );
    assert_eq!(
        edge_traversed_count, 3,
        "3 EdgeTraversed (start->parallel, parallel->merge, merge->finish)"
    );
    assert_eq!(graph_completed_count, 1, "Exactly 1 GraphCompleted");

    // Total event count
    let expected_total = 1 + 4 + 4 + 3 + 1; // 13
    assert_eq!(
        events.len(),
        expected_total,
        "Total event count should be {}, got {}",
        expected_total,
        events.len()
    );

    // Verify the first event is GraphStarted and the last is GraphCompleted
    assert!(
        matches!(events[0].payload, EventPayload::GraphStarted),
        "First event should be GraphStarted"
    );
    assert!(
        matches!(
            events[events.len() - 1].payload,
            EventPayload::GraphCompleted { .. }
        ),
        "Last event should be GraphCompleted"
    );

    // Verify that for each node, NodeEntered comes before NodeCompleted
    let sequential_nodes = ["start", "parallel", "merge", "finish"];
    for node_name in &sequential_nodes {
        let entered_seq = events
            .iter()
            .find(|e| {
                matches!(&e.payload, EventPayload::NodeEntered { node_id } if node_id == node_name)
            })
            .map(|e| e.seq);
        let completed_seq = events
            .iter()
            .find(|e| {
                matches!(&e.payload, EventPayload::NodeCompleted { node_id, .. } if node_id == node_name)
            })
            .map(|e| e.seq);

        assert!(
            entered_seq.is_some(),
            "NodeEntered for {} should exist",
            node_name
        );
        assert!(
            completed_seq.is_some(),
            "NodeCompleted for {} should exist",
            node_name
        );
        assert!(
            entered_seq.unwrap() < completed_seq.unwrap(),
            "NodeEntered for {} (seq={}) should come before NodeCompleted (seq={})",
            node_name,
            entered_seq.unwrap(),
            completed_seq.unwrap()
        );
    }
}
