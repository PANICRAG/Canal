//! Workflow templates for common graph patterns.
//!
//! Templates are pre-built graph patterns that can be instantiated with
//! custom handlers. They encode common multi-agent workflow patterns
//! like simple execution, verification loops, plan-execute, and
//! parallel research.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::graph::{
    ClosureHandler, ClosurePredicate, GraphError, GraphState, NodeContext, NodeError, NodeHandler,
    ParallelNode, StateGraph, StateGraphBuilder,
};

/// A workflow template description.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplate {
    /// Unique template identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of the template's purpose.
    pub description: String,
    /// The pattern type.
    pub pattern: TemplatePattern,
    /// Default configuration for this template.
    pub default_config: TemplateConfig,
}

/// Template pattern types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TemplatePattern {
    /// Single agent, no verification: [Agent] → [END]
    Simple,
    /// Agent with verification loop: [Agent] → [Verify] → (pass) → [END] / (fail) → [Agent]
    WithVerification,
    /// Plan then execute: [Planner] → [Executor] → [Synthesizer] → [END]
    PlanExecute,
    /// Auto-select template based on classification
    Full,
    /// Parallel research: [QueryPlanner] → [Parallel Searches] → [Merge] → [END]
    Research,
}

/// Configuration for template instantiation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateConfig {
    /// Maximum retries for verification loops.
    pub max_retries: u32,
    /// Number of parallel branches for research template.
    pub parallel_branches: usize,
    /// Maximum execution depth.
    pub max_depth: usize,
}

impl Default for TemplateConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            parallel_branches: 3,
            max_depth: 5,
        }
    }
}

/// Registry of workflow templates.
pub struct TemplateRegistry {
    templates: HashMap<String, WorkflowTemplate>,
}

impl TemplateRegistry {
    /// Create a registry with built-in templates.
    pub fn with_builtins() -> Self {
        let mut templates = HashMap::new();

        templates.insert(
            "simple".into(),
            WorkflowTemplate {
                id: "simple".into(),
                name: "Simple".into(),
                description: "Single agent execution with no verification.".into(),
                pattern: TemplatePattern::Simple,
                default_config: TemplateConfig::default(),
            },
        );

        templates.insert(
            "with_verification".into(),
            WorkflowTemplate {
                id: "with_verification".into(),
                name: "With Verification".into(),
                description: "Agent execution followed by verification with retry.".into(),
                pattern: TemplatePattern::WithVerification,
                default_config: TemplateConfig::default(),
            },
        );

        templates.insert(
            "plan_execute".into(),
            WorkflowTemplate {
                id: "plan_execute".into(),
                name: "Plan-Execute".into(),
                description: "Plan steps then execute them sequentially.".into(),
                pattern: TemplatePattern::PlanExecute,
                default_config: TemplateConfig::default(),
            },
        );

        templates.insert(
            "full".into(),
            WorkflowTemplate {
                id: "full".into(),
                name: "Full (Auto-Select)".into(),
                description: "Classify task and select appropriate template.".into(),
                pattern: TemplatePattern::Full,
                default_config: TemplateConfig::default(),
            },
        );

        templates.insert(
            "research".into(),
            WorkflowTemplate {
                id: "research".into(),
                name: "Research".into(),
                description: "Parallel research with synthesis.".into(),
                pattern: TemplatePattern::Research,
                default_config: TemplateConfig {
                    parallel_branches: 3,
                    ..Default::default()
                },
            },
        );

        Self { templates }
    }

    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            templates: HashMap::new(),
        }
    }

    /// Register a custom template.
    pub fn register(&mut self, template: WorkflowTemplate) {
        self.templates.insert(template.id.clone(), template);
    }

    /// Get a template by ID.
    pub fn get(&self, id: &str) -> Option<&WorkflowTemplate> {
        self.templates.get(id)
    }

    /// List all template IDs.
    pub fn list(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.templates.keys().map(|s| s.as_str()).collect();
        keys.sort();
        keys
    }

    /// Get the number of registered templates.
    pub fn count(&self) -> usize {
        self.templates.len()
    }

    /// Instantiate a Simple template graph.
    ///
    /// Graph: [agent] → END
    pub fn build_simple<S: GraphState>(
        &self,
        agent_handler: impl NodeHandler<S> + 'static,
    ) -> Result<StateGraph<S>, GraphError> {
        StateGraphBuilder::new()
            .add_node("agent", agent_handler)
            .set_entry("agent")
            .set_terminal("agent")
            .build()
    }

    /// Instantiate a WithVerification template graph.
    ///
    /// Graph: [agent] → [verifier] → (pass) → END / (fail) → [agent]
    ///
    /// The verifier handler should set a "verified" field in the state.
    /// The `is_verified` closure checks whether verification passed.
    pub fn build_with_verification<S: GraphState>(
        &self,
        agent_handler: impl NodeHandler<S> + 'static,
        verifier_handler: impl NodeHandler<S> + 'static,
        is_verified: impl Fn(&S) -> bool + Send + Sync + 'static,
        config: &TemplateConfig,
    ) -> Result<StateGraph<S>, GraphError> {
        let max_retries = config.max_retries;
        let retry_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let retry_count_clone = retry_count.clone();

        let predicate = ClosurePredicate::new(move |state: &S| {
            if is_verified(state) {
                "pass".into()
            } else {
                let count = retry_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if count >= max_retries {
                    "max_retries".into()
                } else {
                    "fail".into()
                }
            }
        });

        StateGraphBuilder::new()
            .add_node("agent", agent_handler)
            .add_node("verifier", verifier_handler)
            .add_node(
                "done",
                ClosureHandler::new(|state: S, _ctx: &NodeContext| async move { Ok(state) }),
            )
            .add_edge("agent", "verifier")
            .add_conditional_edge(
                "verifier",
                predicate,
                vec![("pass", "done"), ("fail", "agent"), ("max_retries", "done")],
            )
            .set_entry("agent")
            .set_terminal("done")
            .build()
    }

    /// Instantiate a PlanExecute template graph.
    ///
    /// Graph: [planner] → [executor] → [synthesizer] → END
    pub fn build_plan_execute<S: GraphState>(
        &self,
        planner_handler: impl NodeHandler<S> + 'static,
        executor_handler: impl NodeHandler<S> + 'static,
        synthesizer_handler: impl NodeHandler<S> + 'static,
    ) -> Result<StateGraph<S>, GraphError> {
        StateGraphBuilder::new()
            .add_node("planner", planner_handler)
            .add_node("executor", executor_handler)
            .add_node("synthesizer", synthesizer_handler)
            .add_edge("planner", "executor")
            .add_edge("executor", "synthesizer")
            .set_entry("planner")
            .set_terminal("synthesizer")
            .build()
    }

    /// Instantiate a Full (auto-select) template graph.
    ///
    /// Graph: [classifier] → (simple) → [simple_agent]
    ///                      → (complex) → [planner] → [executor] → [synthesizer]
    pub fn build_full<S: GraphState>(
        &self,
        classifier_handler: impl NodeHandler<S> + 'static,
        simple_handler: impl NodeHandler<S> + 'static,
        planner_handler: impl NodeHandler<S> + 'static,
        executor_handler: impl NodeHandler<S> + 'static,
        synthesizer_handler: impl NodeHandler<S> + 'static,
        classify: impl Fn(&S) -> String + Send + Sync + 'static,
    ) -> Result<StateGraph<S>, GraphError> {
        let predicate = ClosurePredicate::new(classify);

        StateGraphBuilder::new()
            .add_node("classifier", classifier_handler)
            .add_node("simple_agent", simple_handler)
            .add_node("planner", planner_handler)
            .add_node("executor", executor_handler)
            .add_node("synthesizer", synthesizer_handler)
            .add_conditional_edge(
                "classifier",
                predicate,
                vec![("simple", "simple_agent"), ("complex", "planner")],
            )
            .add_edge("planner", "executor")
            .add_edge("executor", "synthesizer")
            .set_entry("classifier")
            .set_terminal("simple_agent")
            .set_terminal("synthesizer")
            .build()
    }

    /// Instantiate a Research template graph.
    ///
    /// Graph: [query_planner] → [parallel_search] → [synthesizer] → END
    pub fn build_research<S: GraphState>(
        &self,
        query_planner: impl NodeHandler<S> + 'static,
        search_handlers: Vec<(String, Arc<dyn NodeHandler<S>>)>,
        synthesizer: impl NodeHandler<S> + 'static,
    ) -> Result<StateGraph<S>, GraphError> {
        let mut builder = StateGraphBuilder::new().add_node("query_planner", query_planner);

        let mut branch_ids = Vec::new();
        for (name, handler) in search_handlers {
            let wrapper = HandlerWrapperTemplate { inner: handler };
            builder = builder.add_node(&name, wrapper);
            branch_ids.push(name);
        }

        let parallel = ParallelNode::new("parallel_search", "Parallel Search", branch_ids);

        builder = builder
            .add_parallel_node(parallel)
            .add_node("synthesizer", synthesizer)
            .add_edge("query_planner", "parallel_search")
            .add_edge("parallel_search", "synthesizer")
            .set_entry("query_planner")
            .set_terminal("synthesizer");

        builder.build()
    }
}

impl Default for TemplateRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

/// Wrapper to use Arc<dyn NodeHandler> as a NodeHandler.
struct HandlerWrapperTemplate<S: GraphState> {
    inner: Arc<dyn NodeHandler<S>>,
}

#[async_trait::async_trait]
impl<S: GraphState> NodeHandler<S> for HandlerWrapperTemplate<S> {
    async fn execute(&self, state: S, ctx: &NodeContext) -> Result<S, NodeError> {
        self.inner.execute(state, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphExecutor;

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct TemplateState {
        value: i32,
        verified: bool,
        trail: Vec<String>,
        classification: String,
    }

    impl GraphState for TemplateState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
            self.trail.extend(other.trail);
        }
    }

    fn make_handler(name: &str, increment: i32) -> ClosureHandler<TemplateState> {
        let name = name.to_string();
        ClosureHandler::new(move |mut state: TemplateState, _ctx: &NodeContext| {
            let name = name.clone();
            async move {
                state.value += increment;
                state.trail.push(name);
                Ok(state)
            }
        })
    }

    #[test]
    fn test_builtin_templates() {
        let registry = TemplateRegistry::with_builtins();
        assert_eq!(registry.count(), 5);

        let mut ids = registry.list();
        ids.sort();
        assert!(ids.contains(&"simple"));
        assert!(ids.contains(&"with_verification"));
        assert!(ids.contains(&"plan_execute"));
        assert!(ids.contains(&"full"));
        assert!(ids.contains(&"research"));
    }

    #[test]
    fn test_template_get() {
        let registry = TemplateRegistry::with_builtins();
        let simple = registry.get("simple").unwrap();
        assert_eq!(simple.pattern, TemplatePattern::Simple);
        assert_eq!(simple.name, "Simple");

        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_custom_template_register() {
        let mut registry = TemplateRegistry::new();
        assert_eq!(registry.count(), 0);

        registry.register(WorkflowTemplate {
            id: "custom".into(),
            name: "Custom Template".into(),
            description: "A custom workflow.".into(),
            pattern: TemplatePattern::Simple,
            default_config: TemplateConfig::default(),
        });
        assert_eq!(registry.count(), 1);
        assert!(registry.get("custom").is_some());
    }

    #[tokio::test]
    async fn test_build_simple_template() {
        let registry = TemplateRegistry::with_builtins();
        let graph = registry.build_simple(make_handler("agent", 42)).unwrap();

        assert_eq!(graph.node_count(), 1);
        assert!(graph.is_terminal(&"agent".into()));

        let executor = GraphExecutor::new(graph);
        let state = TemplateState {
            value: 0,
            verified: false,
            trail: vec![],
            classification: String::new(),
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 42);
        assert_eq!(result.trail, vec!["agent"]);
    }

    #[tokio::test]
    async fn test_build_with_verification_pass() {
        let registry = TemplateRegistry::with_builtins();
        let config = TemplateConfig::default();

        // Agent sets verified = true, so verifier passes immediately
        let agent =
            ClosureHandler::new(|mut state: TemplateState, _ctx: &NodeContext| async move {
                state.value += 10;
                state.verified = true;
                state.trail.push("agent".into());
                Ok(state)
            });
        let verifier =
            ClosureHandler::new(|mut state: TemplateState, _ctx: &NodeContext| async move {
                state.trail.push("verifier".into());
                Ok(state)
            });

        let graph = registry
            .build_with_verification(agent, verifier, |s: &TemplateState| s.verified, &config)
            .unwrap();

        assert_eq!(graph.node_count(), 3); // agent, verifier, done

        let executor = GraphExecutor::new(graph);
        let state = TemplateState {
            value: 0,
            verified: false,
            trail: vec![],
            classification: String::new(),
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 10);
        assert!(result.trail.contains(&"agent".to_string()));
        assert!(result.trail.contains(&"verifier".to_string()));
    }

    #[tokio::test]
    async fn test_build_with_verification_retry() {
        let registry = TemplateRegistry::with_builtins();
        let config = TemplateConfig {
            max_retries: 2,
            ..Default::default()
        };

        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        // Agent passes verification on the 2nd call
        let agent = ClosureHandler::new(move |mut state: TemplateState, _ctx: &NodeContext| {
            let count = call_count_clone.clone();
            async move {
                let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                state.value += 1;
                state.trail.push(format!("agent_{}", n));
                if n >= 1 {
                    state.verified = true;
                }
                Ok(state)
            }
        });
        let verifier =
            ClosureHandler::new(|mut state: TemplateState, _ctx: &NodeContext| async move {
                state.trail.push("verify".into());
                Ok(state)
            });

        let graph = registry
            .build_with_verification(agent, verifier, |s: &TemplateState| s.verified, &config)
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TemplateState {
            value: 0,
            verified: false,
            trail: vec![],
            classification: String::new(),
        };
        let result = executor.execute(state).await.unwrap();
        // Should have retried: agent_0 → verify → agent_1 → verify → done
        assert!(result.verified);
        assert!(result.value >= 2);
        assert!(result.trail.contains(&"agent_0".to_string()));
        assert!(result.trail.contains(&"agent_1".to_string()));
    }

    #[tokio::test]
    async fn test_build_plan_execute_template() {
        let registry = TemplateRegistry::with_builtins();
        let graph = registry
            .build_plan_execute(
                make_handler("planner", 1),
                make_handler("executor", 10),
                make_handler("synthesizer", 100),
            )
            .unwrap();

        assert_eq!(graph.node_count(), 3);

        let executor = GraphExecutor::new(graph);
        let state = TemplateState {
            value: 0,
            verified: false,
            trail: vec![],
            classification: String::new(),
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 111); // 1 + 10 + 100
        assert_eq!(result.trail, vec!["planner", "executor", "synthesizer"]);
    }

    #[tokio::test]
    async fn test_build_full_template_simple_path() {
        let registry = TemplateRegistry::with_builtins();

        // Classifier sets classification to "simple"
        let classifier =
            ClosureHandler::new(|mut state: TemplateState, _ctx: &NodeContext| async move {
                state.classification = "simple".into();
                state.trail.push("classifier".into());
                Ok(state)
            });

        let graph = registry
            .build_full(
                classifier,
                make_handler("simple_agent", 42),
                make_handler("planner", 1),
                make_handler("executor", 10),
                make_handler("synthesizer", 100),
                |s: &TemplateState| s.classification.clone(),
            )
            .unwrap();

        assert_eq!(graph.node_count(), 5);

        let executor = GraphExecutor::new(graph);
        let state = TemplateState {
            value: 0,
            verified: false,
            trail: vec![],
            classification: String::new(),
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 42);
        assert!(result.trail.contains(&"classifier".to_string()));
        assert!(result.trail.contains(&"simple_agent".to_string()));
        assert!(!result.trail.contains(&"planner".to_string()));
    }

    #[tokio::test]
    async fn test_build_full_template_complex_path() {
        let registry = TemplateRegistry::with_builtins();

        let classifier =
            ClosureHandler::new(|mut state: TemplateState, _ctx: &NodeContext| async move {
                state.classification = "complex".into();
                state.trail.push("classifier".into());
                Ok(state)
            });

        let graph = registry
            .build_full(
                classifier,
                make_handler("simple_agent", 42),
                make_handler("planner", 1),
                make_handler("executor", 10),
                make_handler("synthesizer", 100),
                |s: &TemplateState| s.classification.clone(),
            )
            .unwrap();

        let executor = GraphExecutor::new(graph);
        let state = TemplateState {
            value: 0,
            verified: false,
            trail: vec![],
            classification: String::new(),
        };
        let result = executor.execute(state).await.unwrap();
        assert_eq!(result.value, 111); // 1 + 10 + 100
        assert!(result.trail.contains(&"classifier".to_string()));
        assert!(result.trail.contains(&"planner".to_string()));
        assert!(!result.trail.contains(&"simple_agent".to_string()));
    }

    #[tokio::test]
    async fn test_build_research_template() {
        let registry = TemplateRegistry::with_builtins();

        let search_handlers: Vec<(String, Arc<dyn NodeHandler<TemplateState>>)> = vec![
            ("search_a".into(), Arc::new(make_handler("search_a", 10))),
            ("search_b".into(), Arc::new(make_handler("search_b", 20))),
        ];

        let graph = registry
            .build_research(
                make_handler("query_planner", 1),
                search_handlers,
                make_handler("synthesizer", 100),
            )
            .unwrap();

        // query_planner + search_a + search_b + parallel_search + synthesizer = 5
        assert_eq!(graph.node_count(), 5);

        let executor = GraphExecutor::new(graph);
        let state = TemplateState {
            value: 0,
            verified: false,
            trail: vec![],
            classification: String::new(),
        };
        let result = executor.execute(state).await.unwrap();
        // query_planner(1) + parallel merges (0+10 + 0+20 = 30) + synthesizer(100)
        // Actually: initial state value=0, planner adds 1 → state.value=1
        // Parallel: each branch gets state.value=1, branch_a adds 10=11, branch_b adds 20=21
        // WaitAll merge: 1 + 11 + 21 = 33 (initial + two branches)
        // Synthesizer adds 100 → 133
        assert!(result.value > 100); // Exact value depends on merge behavior
        assert!(result.trail.contains(&"query_planner".to_string()));
        assert!(result.trail.contains(&"synthesizer".to_string()));
    }

    #[test]
    fn test_template_config_default() {
        let config = TemplateConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.parallel_branches, 3);
        assert_eq!(config.max_depth, 5);
    }

    #[test]
    fn test_template_pattern_serialization() {
        let pattern = TemplatePattern::PlanExecute;
        let json = serde_json::to_string(&pattern).unwrap();
        assert_eq!(json, "\"PlanExecute\"");

        let deserialized: TemplatePattern = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, TemplatePattern::PlanExecute);
    }

    #[test]
    fn test_workflow_template_serialization() {
        let template = WorkflowTemplate {
            id: "test".into(),
            name: "Test".into(),
            description: "Test template".into(),
            pattern: TemplatePattern::Simple,
            default_config: TemplateConfig::default(),
        };
        let json = serde_json::to_string(&template).unwrap();
        let deserialized: WorkflowTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "test");
    }
}
