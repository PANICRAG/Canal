//! Integration tests for orchestration (graph + collaboration).
//!
//! These tests verify the production path from:
//! - StateGraph → GraphExecutor → result
//! - CollaborationMode selection → graph creation
//! - Template instantiation → graph execution
//!
//! # Feature Gate
//!
//! These tests require the `orchestration` feature.

#![cfg(feature = "orchestration")]

use gateway_core::collaboration::{
    CollaborationMode, ContextTransferMode, HandoffCondition, HandoffRule, TemplateConfig,
    TemplateRegistry,
};
use gateway_core::graph::{
    ClosureHandler, ClosurePredicate, GraphExecutor, GraphState, NodeContext, StateGraphBuilder,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

// ============================================================================
// Test State
// ============================================================================

/// A simple state for testing graph execution.
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
// Graph Execution Tests
// ============================================================================

#[tokio::test]
async fn test_graph_linear_execution() {
    // Build a simple linear graph: start → process → finish
    let graph = StateGraphBuilder::new()
        .add_node(
            "start",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("start".into());
                state.value += 10;
                Ok(state)
            }),
        )
        .add_node(
            "process",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("process".into());
                state.value *= 2;
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
        .add_edge("start", "process")
        .add_edge("process", "finish")
        .set_entry("start")
        .set_terminal("finish")
        .build()
        .expect("Failed to build graph");

    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 5,
        steps: vec![],
        done: false,
    };

    let result = executor.execute(initial).await.expect("Execution failed");

    assert_eq!(result.steps, vec!["start", "process", "finish"]);
    assert_eq!(result.value, 30); // (5 + 10) * 2 = 30
    assert!(result.done);
}

#[tokio::test]
async fn test_graph_conditional_branching() {
    // Build a graph with conditional branching based on value
    let graph = StateGraphBuilder::new()
        .add_node(
            "check",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("check".into());
                Ok(state)
            }),
        )
        .add_node(
            "high",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("high".into());
                state.value *= 10;
                Ok(state)
            }),
        )
        .add_node(
            "low",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("low".into());
                state.value += 1;
                Ok(state)
            }),
        )
        .add_conditional_edge(
            "check",
            ClosurePredicate::new(|state: &TestState| {
                if state.value >= 50 {
                    "high".into()
                } else {
                    "low".into()
                }
            }),
            vec![("high", "high"), ("low", "low")],
        )
        .set_entry("check")
        .set_terminal("high")
        .set_terminal("low")
        .build()
        .expect("Failed to build graph");

    let executor = GraphExecutor::new(graph);

    // Test high path
    let high_initial = TestState {
        value: 100,
        steps: vec![],
        done: false,
    };
    let high_result = executor
        .execute(high_initial)
        .await
        .expect("High path failed");
    assert_eq!(high_result.steps, vec!["check", "high"]);
    assert_eq!(high_result.value, 1000); // 100 * 10

    // Test low path - need new executor for new graph instance
    let low_graph = StateGraphBuilder::new()
        .add_node(
            "check",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("check".into());
                Ok(state)
            }),
        )
        .add_node(
            "high",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("high".into());
                state.value *= 10;
                Ok(state)
            }),
        )
        .add_node(
            "low",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("low".into());
                state.value += 1;
                Ok(state)
            }),
        )
        .add_conditional_edge(
            "check",
            ClosurePredicate::new(|state: &TestState| {
                if state.value >= 50 {
                    "high".into()
                } else {
                    "low".into()
                }
            }),
            vec![("high", "high"), ("low", "low")],
        )
        .set_entry("check")
        .set_terminal("high")
        .set_terminal("low")
        .build()
        .expect("Failed to build graph");

    let low_executor = GraphExecutor::new(low_graph);
    let low_initial = TestState {
        value: 10,
        steps: vec![],
        done: false,
    };
    let low_result = low_executor
        .execute(low_initial)
        .await
        .expect("Low path failed");
    assert_eq!(low_result.steps, vec!["check", "low"]);
    assert_eq!(low_result.value, 11); // 10 + 1
}

#[tokio::test]
async fn test_graph_with_loop_and_termination() {
    // Build a graph with a retry loop that terminates after max retries
    let attempt_count = Arc::new(AtomicU32::new(0));
    let attempt_count_clone = attempt_count.clone();

    let graph = StateGraphBuilder::new()
        .add_node(
            "attempt",
            ClosureHandler::new(move |mut state: TestState, _: &NodeContext| {
                let count = attempt_count_clone.clone();
                async move {
                    let n = count.fetch_add(1, Ordering::SeqCst);
                    state.steps.push(format!("attempt_{}", n));
                    state.value += 1;
                    // Mark as done after 3 attempts
                    if n >= 2 {
                        state.done = true;
                    }
                    Ok(state)
                }
            }),
        )
        .add_node(
            "complete",
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("complete".into());
                Ok(state)
            }),
        )
        .add_conditional_edge(
            "attempt",
            ClosurePredicate::new(|state: &TestState| {
                if state.done {
                    "complete".into()
                } else {
                    "retry".into()
                }
            }),
            vec![("complete", "complete"), ("retry", "attempt")],
        )
        .set_entry("attempt")
        .set_terminal("complete")
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
    assert_eq!(result.value, 3); // 3 attempts
    assert!(result.steps.contains(&"attempt_0".to_string()));
    assert!(result.steps.contains(&"attempt_1".to_string()));
    assert!(result.steps.contains(&"attempt_2".to_string()));
    assert!(result.steps.contains(&"complete".to_string()));
}

// ============================================================================
// Template Integration Tests
// ============================================================================

#[tokio::test]
async fn test_template_registry_simple() {
    let registry = TemplateRegistry::with_builtins();

    // Verify all built-in templates are registered
    assert!(registry.get("simple").is_some());
    assert!(registry.get("with_verification").is_some());
    assert!(registry.get("plan_execute").is_some());
    assert!(registry.get("full").is_some());
    assert!(registry.get("research").is_some());

    // Build and execute a simple template
    let graph = registry
        .build_simple(ClosureHandler::new(
            |mut state: TestState, _: &NodeContext| async move {
                state.value = 42;
                state.steps.push("simple_agent".into());
                Ok(state)
            },
        ))
        .expect("Failed to build simple template");

    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 0,
        steps: vec![],
        done: false,
    };

    let result = executor.execute(initial).await.expect("Execution failed");
    assert_eq!(result.value, 42);
    assert_eq!(result.steps, vec!["simple_agent"]);
}

#[tokio::test]
async fn test_template_plan_execute() {
    let registry = TemplateRegistry::with_builtins();

    let graph = registry
        .build_plan_execute(
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("planner".into());
                state.value = 1; // Plan: do step 1
                Ok(state)
            }),
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("executor".into());
                state.value *= 100; // Execute: multiply by 100
                Ok(state)
            }),
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("synthesizer".into());
                state.done = true;
                Ok(state)
            }),
        )
        .expect("Failed to build plan_execute template");

    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 0,
        steps: vec![],
        done: false,
    };

    let result = executor.execute(initial).await.expect("Execution failed");
    assert_eq!(result.steps, vec!["planner", "executor", "synthesizer"]);
    assert_eq!(result.value, 100);
    assert!(result.done);
}

#[tokio::test]
async fn test_template_with_verification_pass() {
    let registry = TemplateRegistry::with_builtins();
    let config = TemplateConfig::default();

    let graph = registry
        .build_with_verification(
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("agent".into());
                state.value = 100;
                state.done = true; // Mark as verified
                Ok(state)
            }),
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("verifier".into());
                Ok(state)
            }),
            |state: &TestState| state.done, // Verification check
            &config,
        )
        .expect("Failed to build with_verification template");

    let executor = GraphExecutor::new(graph);
    let initial = TestState {
        value: 0,
        steps: vec![],
        done: false,
    };

    let result = executor.execute(initial).await.expect("Execution failed");
    assert!(result.steps.contains(&"agent".to_string()));
    assert!(result.steps.contains(&"verifier".to_string()));
    assert_eq!(result.value, 100);
}

#[tokio::test]
async fn test_template_full_classification() {
    let registry = TemplateRegistry::with_builtins();

    let graph = registry
        .build_full(
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("classifier".into());
                // Classify based on value: high value = complex, low = simple
                if state.value > 50 {
                    state.done = false; // Mark for complex path
                } else {
                    state.done = true; // Mark for simple path
                }
                Ok(state)
            }),
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("simple_agent".into());
                state.value += 10;
                Ok(state)
            }),
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("planner".into());
                Ok(state)
            }),
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("executor".into());
                state.value *= 10;
                Ok(state)
            }),
            ClosureHandler::new(|mut state: TestState, _: &NodeContext| async move {
                state.steps.push("synthesizer".into());
                Ok(state)
            }),
            |state: &TestState| {
                if state.done {
                    "simple".into()
                } else {
                    "complex".into()
                }
            },
        )
        .expect("Failed to build full template");

    // Test simple path
    let executor = GraphExecutor::new(graph);
    let simple_initial = TestState {
        value: 10,
        steps: vec![],
        done: false,
    };

    let simple_result = executor
        .execute(simple_initial)
        .await
        .expect("Simple path failed");
    assert!(simple_result.steps.contains(&"classifier".to_string()));
    assert!(simple_result.steps.contains(&"simple_agent".to_string()));
    assert!(!simple_result.steps.contains(&"planner".to_string()));
    assert_eq!(simple_result.value, 20); // 10 + 10
}

// ============================================================================
// Collaboration Mode Tests
// ============================================================================

#[test]
fn test_collaboration_mode_serialization() {
    // Test Direct mode
    let direct = CollaborationMode::Direct;
    let json = serde_json::to_string(&direct).unwrap();
    assert!(json.contains("Direct"));

    // Test Swarm mode
    let swarm = CollaborationMode::Swarm {
        initial_agent: "researcher".into(),
        handoff_rules: vec![HandoffRule {
            from_agent: "researcher".into(),
            to_agent: "coder".into(),
            condition: HandoffCondition::OnKeyword("implement".into()),
            context_transfer: ContextTransferMode::Full,
        }],
        agent_models: Default::default(),
    };
    let json = serde_json::to_string(&swarm).unwrap();
    assert!(json.contains("Swarm"));
    assert!(json.contains("researcher"));
    assert!(json.contains("coder"));

    // Test Expert mode
    let expert = CollaborationMode::Expert {
        supervisor: "architect".into(),
        specialists: vec!["frontend".into(), "backend".into()],
        supervisor_model: None,
        default_specialist_model: None,
        specialist_models: Default::default(),
    };
    let json = serde_json::to_string(&expert).unwrap();
    assert!(json.contains("Expert"));
    assert!(json.contains("architect"));
    assert!(json.contains("frontend"));
}

#[test]
fn test_handoff_condition_variants() {
    // OnToolCall
    let tool_cond = HandoffCondition::OnToolCall("web_search".into());
    let json = serde_json::to_string(&tool_cond).unwrap();
    assert!(json.contains("OnToolCall"));
    assert!(json.contains("web_search"));

    // OnKeyword
    let keyword_cond = HandoffCondition::OnKeyword("handoff".into());
    let json = serde_json::to_string(&keyword_cond).unwrap();
    assert!(json.contains("OnKeyword"));

    // OnClassification
    let class_cond = HandoffCondition::OnClassification("code_related".into());
    let json = serde_json::to_string(&class_cond).unwrap();
    assert!(json.contains("OnClassification"));

    // Always
    let always_cond = HandoffCondition::Always;
    let json = serde_json::to_string(&always_cond).unwrap();
    assert!(json.contains("Always"));
}

#[test]
fn test_context_transfer_modes() {
    // Test Full mode
    let full = ContextTransferMode::Full;
    let json = serde_json::to_string(&full).unwrap();
    let deserialized: ContextTransferMode = serde_json::from_str(&json).unwrap();
    assert_eq!(format!("{:?}", full), format!("{:?}", deserialized));

    // Test Summary mode
    let summary = ContextTransferMode::Summary;
    let json = serde_json::to_string(&summary).unwrap();
    let deserialized: ContextTransferMode = serde_json::from_str(&json).unwrap();
    assert_eq!(format!("{:?}", summary), format!("{:?}", deserialized));

    // Test Selective mode
    let selective = ContextTransferMode::Selective(vec!["messages".into(), "context".into()]);
    let json = serde_json::to_string(&selective).unwrap();
    let deserialized: ContextTransferMode = serde_json::from_str(&json).unwrap();
    assert_eq!(format!("{:?}", selective), format!("{:?}", deserialized));
}

// ============================================================================
// Production Path Validation
// ============================================================================

/// This test validates that the orchestration modules are properly connected
/// in the production code path.
#[test]
fn test_production_path_components_exist() {
    // Verify core types are accessible
    let _: Option<gateway_core::graph::StateGraph<TestState>> = None;
    let _: Option<gateway_core::graph::GraphExecutor<TestState>> = None;
    let _: Option<gateway_core::graph::StateGraphBuilder<TestState>> = None;

    // Verify collaboration types are accessible
    let _: Option<gateway_core::collaboration::CollaborationMode> = None;
    let _: Option<gateway_core::collaboration::TemplateRegistry> = None;
    let _: Option<gateway_core::collaboration::SwarmOrchestrator<TestState>> = None;
    let _: Option<gateway_core::collaboration::ExpertOrchestrator<TestState>> = None;

    // Verify adapter types are accessible
    let _: Option<gateway_core::graph::AgentGraphState> = None;
}

#[tokio::test]
async fn test_template_registry_lifecycle() {
    // Create registry
    let mut registry = TemplateRegistry::new();
    assert_eq!(registry.count(), 0);

    // Register custom template
    use gateway_core::collaboration::{TemplatePattern, WorkflowTemplate};
    registry.register(WorkflowTemplate {
        id: "custom_test".into(),
        name: "Custom Test".into(),
        description: "A custom test template".into(),
        pattern: TemplatePattern::Simple,
        default_config: TemplateConfig::default(),
    });
    assert_eq!(registry.count(), 1);

    // Verify template is retrievable
    let template = registry.get("custom_test").unwrap();
    assert_eq!(template.name, "Custom Test");
    assert_eq!(template.pattern, TemplatePattern::Simple);

    // Build graph from template
    let graph = registry
        .build_simple(ClosureHandler::new(
            |mut state: TestState, _: &NodeContext| async move {
                state.value = 999;
                Ok(state)
            },
        ))
        .expect("Failed to build graph");

    // Execute graph
    let executor = GraphExecutor::new(graph);
    let result = executor
        .execute(TestState {
            value: 0,
            steps: vec![],
            done: false,
        })
        .await
        .expect("Execution failed");

    assert_eq!(result.value, 999);
}
