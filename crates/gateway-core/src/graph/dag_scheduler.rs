//! DAG-level automatic parallel scheduling.
//!
//! Analyzes a `StateGraph` to identify independent nodes that can be
//! executed in parallel. Uses Kahn's algorithm to compute topological
//! "waves" where each wave contains independent nodes.
//!
//! # Example
//!
//! ```text
//! Diamond graph: entry→A, entry→B, A→C, B→C
//!
//! Waves: [[entry], [A, B], [C]]
//!         ↑ sequential  ↑ parallel ↑ sequential
//! ```

use std::collections::{HashMap, HashSet, VecDeque};

use super::builder::StateGraph;
use super::edge::EdgeType;
use super::error::NodeId;
use super::GraphState;

/// A single execution wave containing nodes that can run in parallel.
#[derive(Debug, Clone)]
pub struct ExecutionWave {
    /// Nodes in this wave (can all execute concurrently).
    pub nodes: Vec<NodeId>,
    /// Zero-based wave index.
    pub wave_index: usize,
}

/// A segment of a graph that can be DAG-scheduled.
///
/// Conditional edges break DAG scheduling — each contiguous
/// region of direct edges forms a separate `DagSegment`.
#[derive(Debug, Clone)]
pub struct DagSegment {
    /// Parallel waves within this segment.
    pub waves: Vec<ExecutionWave>,
    /// Whether this segment ends with a conditional edge.
    pub ends_with_conditional: bool,
    /// Target node of the conditional edge (if any).
    pub conditional_target: Option<NodeId>,
}

/// DAG scheduler for computing parallel execution waves.
pub struct DagScheduler;

impl DagScheduler {
    /// Compute full DAG waves for a graph that contains only direct edges.
    ///
    /// Returns `None` if:
    /// - The graph contains conditional edges
    /// - The graph is purely linear (no parallelism benefit)
    /// - The graph has cycles
    pub fn compute_waves<S: GraphState>(graph: &StateGraph<S>) -> Option<Vec<ExecutionWave>> {
        // Check all edges are direct
        for edges in graph.edges.values() {
            for edge in edges {
                if matches!(edge, EdgeType::Conditional(_)) {
                    return None;
                }
            }
        }

        // Build adjacency and in-degree maps
        let node_ids: HashSet<&NodeId> = graph.nodes.keys().collect();
        let mut in_degree: HashMap<&NodeId, usize> = HashMap::new();
        let mut adjacency: HashMap<&NodeId, Vec<&NodeId>> = HashMap::new();

        for id in &node_ids {
            in_degree.insert(id, 0);
            adjacency.insert(id, Vec::new());
        }

        for edges in graph.edges.values() {
            for edge in edges {
                if let EdgeType::Direct(ref de) = edge {
                    if let Some(adj) = adjacency.get_mut(&&de.from) {
                        adj.push(&de.to);
                    }
                    if let Some(deg) = in_degree.get_mut(&&de.to) {
                        *deg += 1;
                    }
                }
            }
        }

        // Kahn's algorithm to compute waves
        let mut queue: VecDeque<&NodeId> = VecDeque::new();
        for (id, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(id);
            }
        }

        let mut waves: Vec<ExecutionWave> = Vec::new();
        let mut processed = 0;

        while !queue.is_empty() {
            let wave_nodes: Vec<NodeId> = queue.drain(..).map(|id| id.clone()).collect();
            let wave_index = waves.len();

            // For each node in the wave, reduce in-degree of successors
            let mut next_queue: VecDeque<&NodeId> = VecDeque::new();
            for node_id in &wave_nodes {
                processed += 1;
                if let Some(neighbors) = adjacency.get(&node_id) {
                    for neighbor in neighbors {
                        if let Some(deg) = in_degree.get_mut(neighbor) {
                            *deg -= 1;
                            if *deg == 0 {
                                next_queue.push_back(neighbor);
                            }
                        }
                    }
                }
            }

            waves.push(ExecutionWave {
                nodes: wave_nodes,
                wave_index,
            });
            queue = next_queue;
        }

        // Check for cycles (not all nodes processed)
        if processed != node_ids.len() {
            return None;
        }

        // Only return if there's actual parallelism (at least one wave with 2+ nodes)
        if waves.iter().any(|w| w.nodes.len() > 1) {
            Some(waves)
        } else {
            None
        }
    }

    /// Check if a graph can benefit from DAG scheduling.
    pub fn is_dag_schedulable<S: GraphState>(graph: &StateGraph<S>) -> bool {
        Self::compute_waves(graph).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::builder::StateGraphBuilder;
    use crate::graph::node::ClosureHandler;
    use crate::graph::GraphState;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Serialize, Deserialize)]
    struct TestState {
        value: i32,
    }

    impl GraphState for TestState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
        }
    }

    fn noop_handler() -> ClosureHandler<TestState> {
        ClosureHandler::new(
            |state: TestState, _ctx: &crate::graph::node::NodeContext| async move { Ok(state) },
        )
    }

    #[test]
    fn test_dag_scheduler_diamond() {
        // Diamond: entry→A, entry→B, A→end, B→end
        let graph = StateGraphBuilder::new()
            .add_node("entry", noop_handler())
            .add_node("a", noop_handler())
            .add_node("b", noop_handler())
            .add_node("end", noop_handler())
            .add_edge("entry", "a")
            .add_edge("entry", "b")
            .add_edge("a", "end")
            .add_edge("b", "end")
            .set_entry("entry")
            .set_terminal("end")
            .build()
            .unwrap();

        let waves = DagScheduler::compute_waves(&graph);
        assert!(waves.is_some());
        let waves = waves.unwrap();

        // Wave 0: [entry], Wave 1: [a, b] (parallel), Wave 2: [end]
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0].nodes.len(), 1);
        assert_eq!(waves[0].nodes[0], "entry");

        let mut wave1: Vec<String> = waves[1].nodes.clone();
        wave1.sort();
        assert_eq!(wave1, vec!["a".to_string(), "b".to_string()]);

        assert_eq!(waves[2].nodes.len(), 1);
        assert_eq!(waves[2].nodes[0], "end");
    }

    #[test]
    fn test_dag_scheduler_linear_no_parallel() {
        // Linear: A→B→C — no parallelism
        let graph = StateGraphBuilder::new()
            .add_node("a", noop_handler())
            .add_node("b", noop_handler())
            .add_node("c", noop_handler())
            .add_edge("a", "b")
            .add_edge("b", "c")
            .set_entry("a")
            .set_terminal("c")
            .build()
            .unwrap();

        let waves = DagScheduler::compute_waves(&graph);
        assert!(waves.is_none()); // No parallelism benefit
    }

    #[test]
    fn test_dag_scheduler_conditional_returns_none() {
        use crate::graph::edge::ClosurePredicate;

        // Graph with conditional edge → not DAG schedulable
        let graph = StateGraphBuilder::new()
            .add_node("start", noop_handler())
            .add_node("a", noop_handler())
            .add_node("b", noop_handler())
            .add_conditional_edge(
                "start",
                ClosurePredicate::new(|_: &TestState| "a".to_string()),
                vec![("a", "a"), ("b", "b")],
            )
            .set_entry("start")
            .set_terminal("a")
            .set_terminal("b")
            .build()
            .unwrap();

        assert!(DagScheduler::compute_waves(&graph).is_none());
        assert!(!DagScheduler::is_dag_schedulable(&graph));
    }

    #[test]
    fn test_dag_scheduler_wide_fan_out() {
        // Fan-out: entry→A, entry→B, entry→C, entry→D, A→end, B→end, C→end, D→end
        let graph = StateGraphBuilder::new()
            .add_node("entry", noop_handler())
            .add_node("a", noop_handler())
            .add_node("b", noop_handler())
            .add_node("c", noop_handler())
            .add_node("d", noop_handler())
            .add_node("end", noop_handler())
            .add_edge("entry", "a")
            .add_edge("entry", "b")
            .add_edge("entry", "c")
            .add_edge("entry", "d")
            .add_edge("a", "end")
            .add_edge("b", "end")
            .add_edge("c", "end")
            .add_edge("d", "end")
            .set_entry("entry")
            .set_terminal("end")
            .build()
            .unwrap();

        let waves = DagScheduler::compute_waves(&graph).unwrap();
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0].nodes.len(), 1); // entry
        assert_eq!(waves[1].nodes.len(), 4); // a, b, c, d
        assert_eq!(waves[2].nodes.len(), 1); // end
    }

    #[test]
    fn test_dag_scheduler_multi_layer() {
        // Multi-layer: entry→A, entry→B, A→C, B→C, C→D, C→E, D→end, E→end
        let graph = StateGraphBuilder::new()
            .add_node("entry", noop_handler())
            .add_node("a", noop_handler())
            .add_node("b", noop_handler())
            .add_node("c", noop_handler())
            .add_node("d", noop_handler())
            .add_node("e", noop_handler())
            .add_node("end", noop_handler())
            .add_edge("entry", "a")
            .add_edge("entry", "b")
            .add_edge("a", "c")
            .add_edge("b", "c")
            .add_edge("c", "d")
            .add_edge("c", "e")
            .add_edge("d", "end")
            .add_edge("e", "end")
            .set_entry("entry")
            .set_terminal("end")
            .build()
            .unwrap();

        let waves = DagScheduler::compute_waves(&graph).unwrap();
        // Wave 0: [entry]
        // Wave 1: [a, b] (parallel)
        // Wave 2: [c]
        // Wave 3: [d, e] (parallel)
        // Wave 4: [end]
        assert_eq!(waves.len(), 5);
        assert_eq!(waves[0].nodes.len(), 1);

        let mut w1: Vec<String> = waves[1].nodes.clone();
        w1.sort();
        assert_eq!(w1, vec!["a".to_string(), "b".to_string()]);

        assert_eq!(waves[2].nodes.len(), 1);
        assert_eq!(waves[2].nodes[0], "c");

        let mut w3: Vec<String> = waves[3].nodes.clone();
        w3.sort();
        assert_eq!(w3, vec!["d".to_string(), "e".to_string()]);

        assert_eq!(waves[4].nodes.len(), 1);
        assert_eq!(waves[4].nodes[0], "end");
    }

    #[test]
    fn test_is_dag_schedulable() {
        let graph = StateGraphBuilder::new()
            .add_node("entry", noop_handler())
            .add_node("a", noop_handler())
            .add_node("b", noop_handler())
            .add_node("end", noop_handler())
            .add_edge("entry", "a")
            .add_edge("entry", "b")
            .add_edge("a", "end")
            .add_edge("b", "end")
            .set_entry("entry")
            .set_terminal("end")
            .build()
            .unwrap();

        assert!(DagScheduler::is_dag_schedulable(&graph));
    }

    #[test]
    fn test_single_node_graph_not_schedulable() {
        let graph = StateGraphBuilder::new()
            .add_node("only", noop_handler())
            .set_entry("only")
            .set_terminal("only")
            .build()
            .unwrap();

        assert!(!DagScheduler::is_dag_schedulable(&graph));
    }

    #[test]
    fn test_wave_indices_sequential() {
        let graph = StateGraphBuilder::new()
            .add_node("entry", noop_handler())
            .add_node("a", noop_handler())
            .add_node("b", noop_handler())
            .add_node("end", noop_handler())
            .add_edge("entry", "a")
            .add_edge("entry", "b")
            .add_edge("a", "end")
            .add_edge("b", "end")
            .set_entry("entry")
            .set_terminal("end")
            .build()
            .unwrap();

        let waves = DagScheduler::compute_waves(&graph).unwrap();
        for (i, wave) in waves.iter().enumerate() {
            assert_eq!(wave.wave_index, i);
        }
    }
}
