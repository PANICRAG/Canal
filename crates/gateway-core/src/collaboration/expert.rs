//! Expert collaboration mode (Supervisor + Specialists).
//!
//! The ExpertOrchestrator uses a supervisor to analyze tasks and dispatch
//! them to specialist agents. Each specialist's output is evaluated by a
//! quality gate. If the quality is insufficient, the supervisor can retry
//! with a different specialist.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::graph::{
    GraphError, GraphExecutor, GraphState, NodeContext, NodeError, NodeHandler, StateGraphBuilder,
};

use super::observer::CollaborationObserver;
use super::quality::{QualityGate, QualityResult};

/// Configuration for the supervisor agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorConfig {
    /// Supervisor agent name.
    pub name: String,
    /// Model to use for the supervisor.
    pub model: Option<String>,
    /// System prompt for the supervisor.
    pub system_prompt: Option<String>,
    /// Maximum number of dispatches before giving up.
    pub max_dispatches: u32,
    /// Minimum quality score to accept a specialist's result.
    pub quality_threshold: f32,
}

/// Specification for a specialist agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecialistSpec {
    /// Specialist name.
    pub name: String,
    /// Description of the specialist's expertise.
    pub description: String,
    /// Model to use.
    pub model: Option<String>,
    /// Tools available to this specialist.
    pub tools: Vec<String>,
}

/// Record of a dispatch to a specialist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchRecord {
    /// Which specialist was dispatched to.
    pub specialist: String,
    /// The quality result from the gate.
    pub quality: QualityResult,
    /// Duration of the specialist's execution in milliseconds.
    pub duration_ms: u64,
    /// Whether this dispatch's result was accepted.
    pub accepted: bool,
}

/// Result of an expert orchestration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpertResult {
    /// All dispatch records.
    pub dispatches: Vec<DispatchRecord>,
    /// Quality scores from all dispatches.
    pub quality_scores: Vec<f32>,
    /// Whether the orchestration succeeded (at least one dispatch accepted).
    pub success: bool,
    /// The specialist whose result was accepted.
    pub accepted_specialist: Option<String>,
}

/// Expert orchestrator: supervisor dispatches to specialist pool.
///
/// The supervisor selects which specialist to dispatch to via a
/// `SupervisorSelector` trait. After each specialist completes, the
/// output is evaluated by a quality gate. If it passes, the result
/// is accepted. Otherwise, the supervisor can retry with a different
/// specialist, up to `max_dispatches`.
pub struct ExpertOrchestrator<S: GraphState> {
    supervisor_config: SupervisorConfig,
    specialists: HashMap<String, Arc<dyn NodeHandler<S>>>,
    specialist_specs: HashMap<String, SpecialistSpec>,
    quality_gate: Arc<dyn QualityGate>,
    selector: Arc<dyn SupervisorSelector<S>>,
    /// Optional observer for collaboration-level events.
    collab_observer: Option<Arc<dyn CollaborationObserver>>,
    /// Execution ID for observer callbacks.
    execution_id: Option<String>,
}

/// Trait for supervisor's specialist selection logic.
///
/// The supervisor decides which specialist to dispatch to based on
/// the current state and the list of available specialists.
#[async_trait::async_trait]
pub trait SupervisorSelector<S: GraphState>: Send + Sync {
    /// Select the next specialist to dispatch to.
    ///
    /// Returns the specialist name, or None if no suitable specialist is found.
    async fn select(
        &self,
        state: &S,
        available: &[String],
        previous_dispatches: &[DispatchRecord],
    ) -> Option<String>;

    /// Extract a text summary from the state for quality evaluation.
    fn extract_result_text(&self, state: &S) -> String;
}

/// Round-robin selector: dispatches to specialists in order, skipping
/// those already tried.
pub struct RoundRobinSelector {
    task_description: String,
}

impl RoundRobinSelector {
    /// Create a new round-robin selector.
    pub fn new(task_description: impl Into<String>) -> Self {
        Self {
            task_description: task_description.into(),
        }
    }
}

#[async_trait::async_trait]
impl<S: GraphState> SupervisorSelector<S> for RoundRobinSelector {
    async fn select(
        &self,
        _state: &S,
        available: &[String],
        previous_dispatches: &[DispatchRecord],
    ) -> Option<String> {
        let tried: std::collections::HashSet<&str> = previous_dispatches
            .iter()
            .map(|d| d.specialist.as_str())
            .collect();
        available
            .iter()
            .find(|s| !tried.contains(s.as_str()))
            .cloned()
    }

    fn extract_result_text(&self, _state: &S) -> String {
        self.task_description.clone()
    }
}

impl<S: GraphState> ExpertOrchestrator<S> {
    /// Create a new ExpertOrchestrator.
    /// Maximum dispatch limit to prevent runaway loops (matches CLAUDE.md resource limit).
    const MAX_DISPATCHES_LIMIT: u32 = 50;

    pub fn new(
        mut config: SupervisorConfig,
        quality_gate: impl QualityGate + 'static,
        selector: impl SupervisorSelector<S> + 'static,
    ) -> Self {
        // R2-M98: Clamp max_dispatches like Swarm clamps max_handoffs
        config.max_dispatches = config.max_dispatches.min(Self::MAX_DISPATCHES_LIMIT);
        Self {
            supervisor_config: config,
            specialists: HashMap::new(),
            specialist_specs: HashMap::new(),
            quality_gate: Arc::new(quality_gate),
            selector: Arc::new(selector),
            collab_observer: None,
            execution_id: None,
        }
    }

    /// Set the collaboration observer for expert events.
    pub fn with_collab_observer(
        mut self,
        observer: Arc<dyn CollaborationObserver>,
        execution_id: impl Into<String>,
    ) -> Self {
        self.execution_id = Some(execution_id.into());
        self.collab_observer = Some(observer);
        self
    }

    /// Register a specialist.
    pub fn add_specialist(
        mut self,
        spec: SpecialistSpec,
        handler: impl NodeHandler<S> + 'static,
    ) -> Self {
        let name = spec.name.clone();
        self.specialist_specs.insert(name.clone(), spec);
        self.specialists.insert(name, Arc::new(handler));
        self
    }

    /// Execute the expert orchestration.
    pub async fn execute(&self, initial_state: S) -> Result<(S, ExpertResult), GraphError> {
        let available_specialists: Vec<String> = self.specialist_specs.keys().cloned().collect();
        let mut dispatches = Vec::new();
        let mut quality_scores = Vec::new();
        let state = initial_state;
        let mut dispatch_count = 0u32;

        loop {
            if dispatch_count >= self.supervisor_config.max_dispatches {
                tracing::warn!(
                    max = self.supervisor_config.max_dispatches,
                    "max dispatches reached"
                );
                return Ok((
                    state,
                    ExpertResult {
                        dispatches,
                        quality_scores,
                        success: false,
                        accepted_specialist: None,
                    },
                ));
            }

            // Ask the supervisor to select a specialist
            let specialist_name = self
                .selector
                .select(&state, &available_specialists, &dispatches)
                .await;

            let exec_id = self.execution_id.as_deref().unwrap_or("");

            // Notify observer of supervisor decision
            if let Some(ref obs) = self.collab_observer {
                obs.on_supervisor_decision(
                    exec_id,
                    specialist_name.as_deref(),
                    &available_specialists,
                )
                .await;
            }

            let specialist_name = match specialist_name {
                Some(name) => name,
                None => {
                    // No more specialists to try
                    return Ok((
                        state,
                        ExpertResult {
                            dispatches,
                            quality_scores,
                            success: false,
                            accepted_specialist: None,
                        },
                    ));
                }
            };

            // Get the specialist handler
            let handler = self
                .specialists
                .get(&specialist_name)
                .ok_or_else(|| GraphError::NodeNotFound(specialist_name.clone()))?;

            dispatch_count += 1;

            // Notify observer of specialist dispatch
            if let Some(ref obs) = self.collab_observer {
                obs.on_specialist_dispatched(exec_id, &specialist_name, dispatch_count)
                    .await;
            }

            // Execute the specialist
            let handler_clone = handler.clone();
            let wrapper = HandlerWrapperExpert {
                inner: handler_clone,
            };

            let graph = StateGraphBuilder::new()
                .add_node(&specialist_name, wrapper)
                .set_entry(&specialist_name)
                .set_terminal(&specialist_name)
                .build()?;

            let start = Instant::now();
            let executor = GraphExecutor::new(graph);
            let new_state = executor.execute(state.clone()).await?;
            let duration_ms = start.elapsed().as_millis() as u64;

            // Evaluate quality
            let result_text = self.selector.extract_result_text(&new_state);
            let quality = self
                .quality_gate
                .evaluate(&specialist_name, &result_text)
                .await;

            // R2-M: Use quality_threshold from config alongside quality gate
            let accepted =
                quality.passed && quality.score >= self.supervisor_config.quality_threshold;
            quality_scores.push(quality.score);

            // Notify observer of quality gate result
            if let Some(ref obs) = self.collab_observer {
                obs.on_quality_gate_result(
                    exec_id,
                    &specialist_name,
                    quality.score,
                    quality.passed,
                    quality.feedback.as_deref().unwrap_or(""),
                )
                .await;
            }

            dispatches.push(DispatchRecord {
                specialist: specialist_name.clone(),
                quality: quality.clone(),
                duration_ms,
                accepted,
            });

            if accepted {
                return Ok((
                    new_state,
                    ExpertResult {
                        dispatches,
                        quality_scores,
                        success: true,
                        accepted_specialist: Some(specialist_name),
                    },
                ));
            }

            // Quality gate failed, try another specialist
            tracing::debug!(
                specialist = %specialist_name,
                score = quality.score,
                feedback = ?quality.feedback,
                "specialist output rejected by quality gate"
            );

            // Keep the original state for the next attempt
        }
    }

    /// Get the supervisor configuration.
    pub fn supervisor_config(&self) -> &SupervisorConfig {
        &self.supervisor_config
    }
}

/// Wrapper to use Arc<dyn NodeHandler> as a NodeHandler.
struct HandlerWrapperExpert<S: GraphState> {
    inner: Arc<dyn NodeHandler<S>>,
}

#[async_trait::async_trait]
impl<S: GraphState> NodeHandler<S> for HandlerWrapperExpert<S> {
    async fn execute(&self, state: S, ctx: &NodeContext) -> Result<S, NodeError> {
        self.inner.execute(state, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collaboration::quality::tests::{AlwaysFailGate, AlwaysPassGate};
    use crate::collaboration::quality::ThresholdQualityGate;
    use crate::graph::ClosureHandler;

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct ExpertState {
        value: i32,
        result_text: String,
    }

    impl GraphState for ExpertState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
            self.result_text.push_str(&other.result_text);
        }
    }

    struct TestSelector {
        task: String,
    }

    #[async_trait::async_trait]
    impl SupervisorSelector<ExpertState> for TestSelector {
        async fn select(
            &self,
            _state: &ExpertState,
            available: &[String],
            previous_dispatches: &[DispatchRecord],
        ) -> Option<String> {
            let tried: std::collections::HashSet<&str> = previous_dispatches
                .iter()
                .map(|d| d.specialist.as_str())
                .collect();
            // Sort for deterministic test behavior (HashMap order is arbitrary)
            let mut sorted: Vec<&String> = available.iter().collect();
            sorted.sort();
            sorted
                .into_iter()
                .find(|s| !tried.contains(s.as_str()))
                .cloned()
        }

        fn extract_result_text(&self, state: &ExpertState) -> String {
            if state.result_text.is_empty() {
                self.task.clone()
            } else {
                state.result_text.clone()
            }
        }
    }

    fn make_specialist(name: &str) -> SpecialistSpec {
        SpecialistSpec {
            name: name.into(),
            description: format!("{} specialist", name),
            model: None,
            tools: vec![],
        }
    }

    fn make_specialist_handler(name: &str, result: &str) -> ClosureHandler<ExpertState> {
        let name = name.to_string();
        let result = result.to_string();
        ClosureHandler::new(move |mut state: ExpertState, _ctx: &NodeContext| {
            let name = name.clone();
            let result = result.clone();
            async move {
                state.value += 1;
                state.result_text = format!("[{}] {}", name, result);
                Ok(state)
            }
        })
    }

    #[tokio::test]
    async fn test_expert_single_dispatch_pass() {
        let config = SupervisorConfig {
            name: "supervisor".into(),
            model: None,
            system_prompt: None,
            max_dispatches: 5,
            quality_threshold: 0.5,
        };

        let orchestrator = ExpertOrchestrator::new(
            config,
            AlwaysPassGate,
            TestSelector {
                task: "test task".into(),
            },
        )
        .add_specialist(
            make_specialist("coder"),
            make_specialist_handler("coder", "Here is the implementation code for the feature."),
        );

        let state = ExpertState {
            value: 0,
            result_text: String::new(),
        };
        let (result, expert_result) = orchestrator.execute(state).await.unwrap();

        assert!(expert_result.success);
        assert_eq!(expert_result.accepted_specialist, Some("coder".into()));
        assert_eq!(expert_result.dispatches.len(), 1);
        assert!(expert_result.dispatches[0].accepted);
        assert_eq!(result.value, 1);
        assert!(result.result_text.contains("coder"));
    }

    #[tokio::test]
    async fn test_expert_quality_gate_fail_retry() {
        let config = SupervisorConfig {
            name: "supervisor".into(),
            model: None,
            system_prompt: None,
            max_dispatches: 5,
            quality_threshold: 0.5,
        };

        // ThresholdQualityGate(0.8): short results fail, long results pass
        let orchestrator = ExpertOrchestrator::new(
            config,
            ThresholdQualityGate::new(0.8),
            TestSelector {
                task: "test task".into(),
            },
        )
        .add_specialist(
            make_specialist("bad_coder"),
            make_specialist_handler("bad_coder", "short"), // Too short, will fail
        )
        .add_specialist(
            make_specialist("good_coder"),
            make_specialist_handler(
                "good_coder",
                "Here is a comprehensive and detailed implementation of the requested feature with full test coverage.",
            ),
        );

        let state = ExpertState {
            value: 0,
            result_text: String::new(),
        };
        let (result, expert_result) = orchestrator.execute(state).await.unwrap();

        assert!(expert_result.success);
        assert_eq!(expert_result.dispatches.len(), 2);
        assert!(!expert_result.dispatches[0].accepted); // bad_coder rejected
        assert!(expert_result.dispatches[1].accepted); // good_coder accepted
        assert_eq!(expert_result.accepted_specialist, Some("good_coder".into()));
        // Only good_coder's state is used (state was reset for retry)
        assert_eq!(result.value, 1);
    }

    #[tokio::test]
    async fn test_expert_all_fail() {
        let config = SupervisorConfig {
            name: "supervisor".into(),
            model: None,
            system_prompt: None,
            max_dispatches: 5,
            quality_threshold: 0.5,
        };

        let orchestrator = ExpertOrchestrator::new(
            config,
            AlwaysFailGate::new("not good enough"),
            TestSelector {
                task: "test task".into(),
            },
        )
        .add_specialist(
            make_specialist("s1"),
            make_specialist_handler("s1", "attempt 1"),
        )
        .add_specialist(
            make_specialist("s2"),
            make_specialist_handler("s2", "attempt 2"),
        );

        let state = ExpertState {
            value: 0,
            result_text: String::new(),
        };
        let (_result, expert_result) = orchestrator.execute(state).await.unwrap();

        assert!(!expert_result.success);
        assert_eq!(expert_result.dispatches.len(), 2);
        assert!(expert_result.accepted_specialist.is_none());
    }

    #[tokio::test]
    async fn test_expert_max_dispatches_limit() {
        let config = SupervisorConfig {
            name: "supervisor".into(),
            model: None,
            system_prompt: None,
            max_dispatches: 1, // Only allow 1 dispatch
            quality_threshold: 0.5,
        };

        let orchestrator = ExpertOrchestrator::new(
            config,
            AlwaysFailGate::new("nope"),
            TestSelector {
                task: "test task".into(),
            },
        )
        .add_specialist(
            make_specialist("s1"),
            make_specialist_handler("s1", "result 1"),
        )
        .add_specialist(
            make_specialist("s2"),
            make_specialist_handler("s2", "result 2"),
        );

        let state = ExpertState {
            value: 0,
            result_text: String::new(),
        };
        let (_result, expert_result) = orchestrator.execute(state).await.unwrap();

        assert!(!expert_result.success);
        assert_eq!(expert_result.dispatches.len(), 1); // Only 1 dispatch attempted
    }

    #[tokio::test]
    async fn test_expert_handler_error_propagation() {
        let config = SupervisorConfig {
            name: "supervisor".into(),
            model: None,
            system_prompt: None,
            max_dispatches: 5,
            quality_threshold: 0.5,
        };

        let failing_handler =
            ClosureHandler::new(|_state: ExpertState, _ctx: &NodeContext| async move {
                Err(NodeError::HandlerError("specialist crashed".into()))
            });

        let orchestrator = ExpertOrchestrator::new(
            config,
            AlwaysPassGate,
            TestSelector {
                task: "test task".into(),
            },
        )
        .add_specialist(make_specialist("crasher"), failing_handler);

        let state = ExpertState {
            value: 0,
            result_text: String::new(),
        };
        let result = orchestrator.execute(state).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_expert_collab_observer_dispatch() {
        use crate::collaboration::observer::CollaborationRecorder;
        use crate::graph::execution_store::{EventPayload, ExecutionMode, ExecutionStore};

        let store = Arc::new(ExecutionStore::new(10));
        store
            .start_execution("expert_obs", ExecutionMode::Expert)
            .await;

        let recorder = Arc::new(CollaborationRecorder::new(store.clone(), "expert_obs"));

        let config = SupervisorConfig {
            name: "supervisor".into(),
            model: None,
            system_prompt: None,
            max_dispatches: 5,
            quality_threshold: 0.5,
        };

        let orchestrator = ExpertOrchestrator::new(
            config,
            AlwaysPassGate,
            TestSelector {
                task: "test task".into(),
            },
        )
        .with_collab_observer(recorder, "expert_obs")
        .add_specialist(
            make_specialist("analyzer"),
            make_specialist_handler("analyzer", "Detailed analysis result."),
        );

        let state = ExpertState {
            value: 0,
            result_text: String::new(),
        };
        let (result, expert_result) = orchestrator.execute(state).await.unwrap();

        assert!(expert_result.success);
        assert_eq!(result.value, 1);

        // Verify observer recorded events
        let events = store.get_events("expert_obs", 0, None);
        assert!(events.len() >= 3); // supervisor_decision + specialist_dispatched + quality_gate_result

        let dispatch_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e.payload, EventPayload::SpecialistDispatched { .. }))
            .collect();
        assert_eq!(dispatch_events.len(), 1);

        let quality_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e.payload, EventPayload::QualityGateResult { .. }))
            .collect();
        assert_eq!(quality_events.len(), 1);

        let decision_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e.payload, EventPayload::SupervisorDecision { .. }))
            .collect();
        assert_eq!(decision_events.len(), 1);
    }

    #[test]
    fn test_supervisor_config_serialization() {
        let config = SupervisorConfig {
            name: "supervisor".into(),
            model: Some("claude-opus-4-5-20251101".into()),
            system_prompt: Some("You are a supervisor.".into()),
            max_dispatches: 3,
            quality_threshold: 0.7,
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: SupervisorConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "supervisor");
        assert_eq!(deserialized.max_dispatches, 3);
    }

    #[test]
    fn test_expert_result_serialization() {
        let result = ExpertResult {
            dispatches: vec![],
            quality_scores: vec![0.8, 0.9],
            success: true,
            accepted_specialist: Some("coder".into()),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"success\":true"));
    }

    #[tokio::test]
    async fn test_round_robin_selector() {
        let selector = RoundRobinSelector::new("test task");
        let state = ExpertState {
            value: 0,
            result_text: String::new(),
        };

        let available = vec!["a".into(), "b".into(), "c".into()];

        // First selection: picks first available
        let result = selector.select(&state, &available, &[]).await;
        assert_eq!(result, Some("a".into()));

        // After dispatching to "a", picks "b"
        let dispatches = vec![DispatchRecord {
            specialist: "a".into(),
            quality: QualityResult {
                passed: false,
                score: 0.3,
                feedback: None,
            },
            duration_ms: 100,
            accepted: false,
        }];
        let result = selector.select(&state, &available, &dispatches).await;
        assert_eq!(result, Some("b".into()));
    }
}
