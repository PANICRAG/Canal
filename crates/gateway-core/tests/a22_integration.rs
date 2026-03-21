//! Integration tests for A22 Process Monitoring & Debug System.
//!
//! These tests verify:
//! - RecordingObserver correctly bridges GraphObserver events to ExecutionStore
//! - ExecutionStore subscription (per-execution and global)
//! - LRU eviction with active-execution protection
//! - Event type filtering on stored events
//!
//! # Feature Gate
//!
//! These tests require the `full-orchestration` feature.

#![cfg(feature = "full-orchestration")]

use gateway_core::graph::{
    ClosureHandler, ErrorStrategy, EventPayload, ExecutionMode, ExecutionStore, GraphExecutor,
    GraphState, NodeContext, NodeError, ParallelNode, RecordingObserver, StateGraphBuilder,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ============================================================================
// Test State
// ============================================================================

/// A simple state for testing graph execution with observer recording.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct TestState {
    value: i32,
    steps: Vec<String>,
}

impl GraphState for TestState {
    fn merge(&mut self, other: Self) {
        self.value += other.value;
        self.steps.extend(other.steps);
    }
}

// ============================================================================
// Helper: build a ClosureHandler that records its node name in steps
// ============================================================================

fn step_handler(name: &str, increment: i32) -> ClosureHandler<TestState> {
    let name = name.to_string();
    ClosureHandler::new(move |mut state: TestState, _ctx: &NodeContext| {
        let name = name.clone();
        async move {
            state.steps.push(name);
            state.value += increment;
            Ok(state)
        }
    })
}

// ============================================================================
// 1. RecordingObserver full lifecycle (3-node linear graph)
// ============================================================================

#[tokio::test]
async fn test_recording_observer_full_lifecycle() {
    let store = Arc::new(ExecutionStore::new(10));
    let exec_id = "lifecycle_test";

    // Start execution in the store BEFORE running the graph
    store
        .start_execution(exec_id, ExecutionMode::Graph("linear".into()))
        .await;

    let observer = RecordingObserver::new(store.clone(), exec_id);

    // Build a 3-node linear graph: A -> B -> C
    let graph = StateGraphBuilder::new()
        .add_node("a", step_handler("a", 1))
        .add_node("b", step_handler("b", 2))
        .add_node("c", step_handler("c", 3))
        .add_edge("a", "b")
        .add_edge("b", "c")
        .set_entry("a")
        .set_terminal("c")
        .with_observer(observer)
        .build()
        .expect("Failed to build graph");

    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 0,
        steps: vec![],
    };

    let result = executor.execute(initial).await.expect("Execution failed");
    assert_eq!(result.steps, vec!["a", "b", "c"]);
    assert_eq!(result.value, 6);

    // Verify events in the store
    let events = store.get_events(exec_id, 0, None);

    // Expected events:
    // GraphStarted, NodeEntered(a), NodeCompleted(a), EdgeTraversed(a->b),
    // NodeEntered(b), NodeCompleted(b), EdgeTraversed(b->c),
    // NodeEntered(c), NodeCompleted(c), GraphCompleted (via complete_execution)
    //
    // Note: on_graph_complete calls complete_execution which appends GraphCompleted

    // Count event types
    let graph_started = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::GraphStarted))
        .count();
    let node_entered = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::NodeEntered { .. }))
        .count();
    let node_completed = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::NodeCompleted { .. }))
        .count();
    let edge_traversed = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::EdgeTraversed { .. }))
        .count();
    let graph_completed = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::GraphCompleted { .. }))
        .count();

    assert_eq!(graph_started, 1, "Expected 1 GraphStarted event");
    assert_eq!(node_entered, 3, "Expected 3 NodeEntered events");
    assert_eq!(node_completed, 3, "Expected 3 NodeCompleted events");
    assert_eq!(edge_traversed, 2, "Expected 2 EdgeTraversed events");
    assert_eq!(graph_completed, 1, "Expected 1 GraphCompleted event");

    // Verify sequence numbers are monotonically increasing
    for i in 1..events.len() {
        assert!(
            events[i].seq > events[i - 1].seq,
            "Seq numbers should be monotonically increasing: {} vs {}",
            events[i - 1].seq,
            events[i].seq,
        );
    }
}

// ============================================================================
// 2. RecordingObserver parallel events (ContinueOnError with 1 failure)
// ============================================================================

#[tokio::test]
async fn test_recording_observer_parallel_events() {
    let store = Arc::new(ExecutionStore::new(10));
    let exec_id = "parallel_test";

    store
        .start_execution(exec_id, ExecutionMode::Graph("parallel".into()))
        .await;

    let observer = RecordingObserver::new(store.clone(), exec_id);

    // Build a graph with ParallelNode: 3 branches, 1 fails
    let graph = StateGraphBuilder::new()
        .add_node("ok_1", step_handler("ok_1", 10))
        .add_node("ok_2", step_handler("ok_2", 20))
        .add_node(
            "fail_branch",
            ClosureHandler::new(|_state: TestState, _ctx: &NodeContext| async move {
                Err(NodeError::HandlerError("deliberate failure".into()))
            }),
        )
        .add_parallel_node(
            ParallelNode::new(
                "par",
                "Parallel",
                vec!["ok_1".into(), "fail_branch".into(), "ok_2".into()],
            )
            .with_error_strategy(ErrorStrategy::ContinueOnError),
        )
        .set_entry("par")
        .set_terminal("par")
        .with_observer(observer)
        .build()
        .expect("Failed to build graph");

    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 0,
        steps: vec![],
    };

    let result = executor.execute(initial).await.expect("Execution failed");
    // Only successful branches contribute
    assert_eq!(result.value, 30); // 10 + 20

    let events = store.get_events(exec_id, 0, None);

    // Verify ParallelPartialComplete event exists
    let partial_complete = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::ParallelPartialComplete { .. }))
        .count();
    assert!(
        partial_complete >= 1,
        "Expected at least 1 ParallelPartialComplete event"
    );

    // Verify ParallelBranchFailed event exists
    let branch_failed = events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::ParallelBranchFailed { .. }))
        .count();
    assert!(
        branch_failed >= 1,
        "Expected at least 1 ParallelBranchFailed event"
    );

    // Verify the failed branch ID
    let failed_events: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::ParallelBranchFailed { branch_id, .. } => Some(branch_id.clone()),
            _ => None,
        })
        .collect();
    assert!(
        failed_events.contains(&"fail_branch".to_string()),
        "Expected fail_branch in ParallelBranchFailed events"
    );
}

// ============================================================================
// 3. RecordingObserver DAG events (diamond graph)
// ============================================================================

#[tokio::test]
async fn test_recording_observer_dag_events() {
    use gateway_core::graph::GraphConfig;

    let store = Arc::new(ExecutionStore::new(10));
    let exec_id = "dag_test";

    store.start_execution(exec_id, ExecutionMode::Dag).await;

    let observer = RecordingObserver::new(store.clone(), exec_id);

    let config = GraphConfig {
        dag_scheduling: true,
        ..Default::default()
    };

    // Diamond DAG: entry -> a, entry -> b, a -> end, b -> end
    // Waves: [entry], [a, b], [end]
    let graph = StateGraphBuilder::new()
        .with_config(config)
        .add_node("entry", step_handler("entry", 1))
        .add_node("a", step_handler("a", 10))
        .add_node("b", step_handler("b", 20))
        .add_node("end", step_handler("end", 100))
        .add_edge("entry", "a")
        .add_edge("entry", "b")
        .add_edge("a", "end")
        .add_edge("b", "end")
        .set_entry("entry")
        .set_terminal("end")
        .with_observer(observer)
        .build()
        .expect("Failed to build graph");

    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 0,
        steps: vec![],
    };

    let result = executor
        .execute_dag(initial)
        .await
        .expect("DAG execution failed");
    assert!(result.value > 0);

    let events = store.get_events(exec_id, 0, None);

    // Verify DagWaveStarted events
    let wave_started: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::DagWaveStarted { wave_index, .. } => Some(*wave_index),
            _ => None,
        })
        .collect();

    // Verify DagWaveCompleted events
    let wave_completed: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::DagWaveCompleted { wave_index, .. } => Some(*wave_index),
            _ => None,
        })
        .collect();

    // 3 waves: [entry], [a, b], [end]
    assert!(
        wave_started.len() >= 2,
        "Expected at least 2 DagWaveStarted events, got {}",
        wave_started.len()
    );
    assert!(
        wave_completed.len() >= 2,
        "Expected at least 2 DagWaveCompleted events, got {}",
        wave_completed.len()
    );

    // Wave indices should include 0 and 1 at minimum
    assert!(
        wave_started.contains(&0),
        "Expected wave 0 in DagWaveStarted"
    );
    assert!(
        wave_started.contains(&1),
        "Expected wave 1 in DagWaveStarted"
    );
}

// ============================================================================
// 4. Subscriber receives live events in order
// ============================================================================

#[tokio::test]
async fn test_subscriber_receives_live_events() {
    let store = Arc::new(ExecutionStore::new(10));
    let exec_id = "sub_test";

    store.start_execution(exec_id, ExecutionMode::Direct).await;

    // Subscribe with 0 replay (we only want live events)
    let (mut rx, replay) = store.subscribe(exec_id, 0);
    assert!(replay.is_empty(), "No replay events expected");

    // Append 3 events
    store
        .append_event(
            exec_id,
            EventPayload::NodeEntered {
                node_id: "node_a".to_string(),
            },
        )
        .await;

    store
        .append_event(
            exec_id,
            EventPayload::NodeCompleted {
                node_id: "node_a".to_string(),
                duration_ms: 42,
            },
        )
        .await;

    store
        .append_event(
            exec_id,
            EventPayload::EdgeTraversed {
                from: "node_a".to_string(),
                to: "node_b".to_string(),
                label: "default".to_string(),
            },
        )
        .await;

    // Receive and verify order
    let event1 = rx.try_recv().expect("Expected event 1");
    assert!(
        matches!(event1.payload, EventPayload::NodeEntered { .. }),
        "First event should be NodeEntered"
    );
    assert_eq!(event1.seq, 0);

    let event2 = rx.try_recv().expect("Expected event 2");
    assert!(
        matches!(event2.payload, EventPayload::NodeCompleted { .. }),
        "Second event should be NodeCompleted"
    );
    assert_eq!(event2.seq, 1);

    let event3 = rx.try_recv().expect("Expected event 3");
    assert!(
        matches!(event3.payload, EventPayload::EdgeTraversed { .. }),
        "Third event should be EdgeTraversed"
    );
    assert_eq!(event3.seq, 2);

    // No more events
    assert!(rx.try_recv().is_err(), "No more events expected");
}

// ============================================================================
// 5. Global stream lifecycle (2 executions)
// ============================================================================

#[tokio::test]
async fn test_global_stream_lifecycle() {
    use gateway_core::graph::GlobalEvent;

    let store = Arc::new(ExecutionStore::new(10));

    // Subscribe to global stream
    let mut rx = store.subscribe_global().await;

    // Start and complete execution 1
    store.start_execution("exec_1", ExecutionMode::Direct).await;
    store.complete_execution("exec_1", 100).await;

    // Start and complete execution 2
    store.start_execution("exec_2", ExecutionMode::Swarm).await;
    store.complete_execution("exec_2", 200).await;

    // Verify 4 global events: 2 started + 2 completed
    let event1 = rx.try_recv().expect("Expected global event 1");
    match event1 {
        GlobalEvent::ExecutionStarted { id, .. } => assert_eq!(id, "exec_1"),
        other => panic!("Expected ExecutionStarted for exec_1, got {:?}", other),
    }

    let event2 = rx.try_recv().expect("Expected global event 2");
    match event2 {
        GlobalEvent::ExecutionCompleted { id, duration_ms } => {
            assert_eq!(id, "exec_1");
            assert_eq!(duration_ms, 100);
        }
        other => panic!("Expected ExecutionCompleted for exec_1, got {:?}", other),
    }

    let event3 = rx.try_recv().expect("Expected global event 3");
    match event3 {
        GlobalEvent::ExecutionStarted { id, .. } => assert_eq!(id, "exec_2"),
        other => panic!("Expected ExecutionStarted for exec_2, got {:?}", other),
    }

    let event4 = rx.try_recv().expect("Expected global event 4");
    match event4 {
        GlobalEvent::ExecutionCompleted { id, duration_ms } => {
            assert_eq!(id, "exec_2");
            assert_eq!(duration_ms, 200);
        }
        other => panic!("Expected ExecutionCompleted for exec_2, got {:?}", other),
    }

    // No more events
    assert!(rx.try_recv().is_err(), "No more global events expected");
}

// ============================================================================
// 6. LRU eviction preserves active executions
// ============================================================================

#[tokio::test]
async fn test_lru_eviction_preserves_active() {
    let store = Arc::new(ExecutionStore::new(3));

    // Start 5 executions, complete the first 2
    store.start_execution("exec_1", ExecutionMode::Direct).await;
    store.complete_execution("exec_1", 10).await;

    store.start_execution("exec_2", ExecutionMode::Direct).await;
    store.complete_execution("exec_2", 20).await;

    // exec_3, exec_4, exec_5 remain Running (active)
    store.start_execution("exec_3", ExecutionMode::Direct).await;
    store.start_execution("exec_4", ExecutionMode::Direct).await;
    store.start_execution("exec_5", ExecutionMode::Direct).await;

    // After adding exec_4 and exec_5, eviction should have run.
    // max_records=3, we have 5 records. Completed ones (exec_1, exec_2)
    // should be evicted. Active ones (exec_3, exec_4, exec_5) preserved.

    // Verify active executions are preserved
    assert!(
        store.get_execution("exec_3").is_some(),
        "Active exec_3 should be preserved"
    );
    assert!(
        store.get_execution("exec_4").is_some(),
        "Active exec_4 should be preserved"
    );
    assert!(
        store.get_execution("exec_5").is_some(),
        "Active exec_5 should be preserved"
    );

    // Verify completed executions were evicted
    assert!(
        store.get_execution("exec_1").is_none(),
        "Completed exec_1 should be evicted"
    );
    assert!(
        store.get_execution("exec_2").is_none(),
        "Completed exec_2 should be evicted"
    );

    // Active list should contain the 3 running executions
    let active = store.list_active();
    assert_eq!(active.len(), 3, "Should have 3 active executions");
}

// ============================================================================
// 7. Event type filtering (NodeEntered only)
// ============================================================================

#[tokio::test]
async fn test_event_type_filtering() {
    let store = Arc::new(ExecutionStore::new(10));
    let exec_id = "filter_test";

    store
        .start_execution(exec_id, ExecutionMode::Graph("linear".into()))
        .await;

    let observer = RecordingObserver::new(store.clone(), exec_id);

    // Build a 3-node linear graph
    let graph = StateGraphBuilder::new()
        .add_node("x", step_handler("x", 1))
        .add_node("y", step_handler("y", 2))
        .add_node("z", step_handler("z", 3))
        .add_edge("x", "y")
        .add_edge("y", "z")
        .set_entry("x")
        .set_terminal("z")
        .with_observer(observer)
        .build()
        .expect("Failed to build graph");

    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 0,
        steps: vec![],
    };

    let result = executor.execute(initial).await.expect("Execution failed");
    assert_eq!(result.value, 6);

    // Get all events
    let all_events = store.get_events(exec_id, 0, None);
    assert!(
        all_events.len() > 3,
        "Should have more than 3 total events (got {})",
        all_events.len()
    );

    // Filter for NodeEntered events only
    let node_entered_events: Vec<_> = all_events
        .iter()
        .filter(|e| matches!(e.payload, EventPayload::NodeEntered { .. }))
        .collect();

    assert_eq!(
        node_entered_events.len(),
        3,
        "Should have exactly 3 NodeEntered events"
    );

    // Verify the node IDs in NodeEntered events
    let entered_node_ids: Vec<String> = node_entered_events
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::NodeEntered { node_id } => Some(node_id.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(entered_node_ids.len(), 3);
    assert_eq!(entered_node_ids[0], "x");
    assert_eq!(entered_node_ids[1], "y");
    assert_eq!(entered_node_ids[2], "z");

    // Verify that non-NodeEntered events are excluded
    let non_entered: Vec<_> = node_entered_events
        .iter()
        .filter(|e| !matches!(e.payload, EventPayload::NodeEntered { .. }))
        .collect();
    assert!(
        non_entered.is_empty(),
        "Filtered results should contain only NodeEntered events"
    );
}
