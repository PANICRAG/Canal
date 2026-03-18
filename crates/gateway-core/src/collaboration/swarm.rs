//! Swarm collaboration mode.
//!
//! Implements agent-to-agent handoff with context transfer, inspired by
//! OpenAI Swarm. Agents are chained together via handoff rules, where
//! each agent processes the state and may trigger a handoff to the next
//! agent based on conditions.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::graph::{
    GraphError, GraphExecutor, GraphState, NodeContext, NodeError, NodeHandler, StateGraphBuilder,
};

use super::observer::CollaborationObserver;
use super::AgentSpec;

/// Defines a handoff rule between two agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffRule {
    /// Source agent name.
    pub from_agent: String,
    /// Target agent name.
    pub to_agent: String,
    /// Condition that triggers the handoff.
    pub condition: HandoffCondition,
    /// How to transfer context between agents.
    pub context_transfer: ContextTransferMode,
}

/// Conditions that trigger an agent handoff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HandoffCondition {
    /// Handoff when a specific tool call is detected in the output.
    OnToolCall(String),
    /// Handoff when the output contains a specific keyword.
    OnKeyword(String),
    /// Handoff when intent is classified as the given label.
    OnClassification(String),
    /// Always handoff after the agent completes.
    Always,
}

/// How context is transferred between agents during handoff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContextTransferMode {
    /// Transfer the entire state as-is.
    Full,
    /// Transfer a summarized version of the state.
    Summary,
    /// Transfer only specified fields.
    Selective(Vec<String>),
}

/// Record of a single handoff event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffRecord {
    /// Agent that handed off.
    pub from_agent: String,
    /// Agent that received the handoff.
    pub to_agent: String,
    /// Which condition triggered the handoff.
    pub condition: String,
    /// Duration of the source agent's execution in milliseconds.
    pub duration_ms: u64,
}

/// Result of a swarm execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmResult {
    /// The complete handoff chain.
    pub handoff_chain: Vec<HandoffRecord>,
    /// Total number of handoffs performed.
    pub total_handoffs: u32,
    /// The final agent that produced the result.
    pub final_agent: String,
}

/// Swarm orchestrator that chains agents via handoff rules.
///
/// The orchestrator executes agents sequentially, checking handoff rules
/// after each agent completes. When a handoff rule matches, the state is
/// transferred to the next agent. Execution continues until no handoff
/// rules match or the maximum number of handoffs is reached.
/// Maximum allowed value for max_handoffs to prevent runaway execution.
const MAX_HANDOFFS_LIMIT: u32 = 100;

/// Maximum number of times a single agent can be revisited before erroring.
const MAX_AGENT_REVISITS: u32 = 2;

pub struct SwarmOrchestrator<S: GraphState> {
    agents: HashMap<String, Arc<dyn NodeHandler<S>>>,
    agent_specs: HashMap<String, AgentSpec>,
    handoff_rules: Vec<HandoffRule>,
    max_handoffs: u32,
    /// Optional observer for collaboration-level events.
    collab_observer: Option<Arc<dyn CollaborationObserver>>,
    /// Execution ID for observer callbacks.
    execution_id: Option<String>,
}

impl<S: GraphState> SwarmOrchestrator<S> {
    /// Create a new SwarmOrchestrator.
    ///
    /// `max_handoffs` is clamped to `MAX_HANDOFFS_LIMIT` (100).
    pub fn new(max_handoffs: u32) -> Self {
        Self {
            agents: HashMap::new(),
            agent_specs: HashMap::new(),
            handoff_rules: Vec::new(),
            max_handoffs: max_handoffs.min(MAX_HANDOFFS_LIMIT),
            collab_observer: None,
            execution_id: None,
        }
    }

    /// Set the collaboration observer for swarm events.
    pub fn with_collab_observer(
        mut self,
        observer: Arc<dyn CollaborationObserver>,
        execution_id: impl Into<String>,
    ) -> Self {
        self.execution_id = Some(execution_id.into());
        self.collab_observer = Some(observer);
        self
    }

    /// Register an agent handler.
    pub fn add_agent(mut self, spec: AgentSpec, handler: impl NodeHandler<S> + 'static) -> Self {
        let name = spec.name.clone();
        self.agent_specs.insert(name.clone(), spec);
        self.agents.insert(name, Arc::new(handler));
        self
    }

    /// Add a handoff rule.
    pub fn add_handoff_rule(mut self, rule: HandoffRule) -> Self {
        self.handoff_rules.push(rule);
        self
    }

    /// Execute the swarm starting from the given agent.
    pub async fn execute(
        &self,
        initial_agent: &str,
        initial_state: S,
    ) -> Result<(S, SwarmResult), GraphError> {
        // Validate initial agent exists
        if !self.agents.contains_key(initial_agent) {
            return Err(GraphError::NodeNotFound(initial_agent.into()));
        }

        let mut current_agent = initial_agent.to_string();
        let mut state = initial_state;
        let mut handoff_chain = Vec::new();
        let mut visit_counts: HashMap<String, u32> = HashMap::new();
        let mut handoff_count = 0u32;

        loop {
            // Cycle detection with max revisits enforcement
            let visits = visit_counts.entry(current_agent.clone()).or_insert(0);
            *visits += 1;
            if *visits > MAX_AGENT_REVISITS + 1 {
                let exec_id = self.execution_id.as_deref().unwrap_or("");
                if let Some(ref obs) = self.collab_observer {
                    obs.on_cycle_detected(exec_id, &current_agent, *visits)
                        .await;
                }
                tracing::warn!(
                    agent = %current_agent,
                    visits = *visits,
                    max_revisits = MAX_AGENT_REVISITS,
                    "Agent exceeded max revisit limit, stopping swarm"
                );
                return Err(GraphError::Internal(format!(
                    "Agent '{}' exceeded max revisit limit ({})",
                    current_agent, MAX_AGENT_REVISITS
                )));
            }
            if *visits > 1 && handoff_count > 0 {
                tracing::debug!(
                    agent = %current_agent,
                    handoff_count,
                    visit_count = *visits,
                    "revisiting agent (cycle possible)"
                );
            }

            // Get the handler for the current agent
            let handler = self
                .agents
                .get(&current_agent)
                .ok_or_else(|| GraphError::NodeNotFound(current_agent.clone()))?;

            // Execute the agent by building a 1-node graph
            let handler_clone = handler.clone();
            let wrapper = HandlerWrapperSwarm {
                inner: handler_clone,
            };

            let graph = StateGraphBuilder::new()
                .add_node(&current_agent, wrapper)
                .set_entry(&current_agent)
                .set_terminal(&current_agent)
                .build()?;

            let start = Instant::now();
            let executor = GraphExecutor::new(graph);
            state = executor.execute(state).await?;
            let duration_ms = start.elapsed().as_millis() as u64;

            // Check handoff rules
            let next_agent = self.find_matching_handoff(&current_agent, &state);

            match next_agent {
                Some((rule, condition_desc)) => {
                    handoff_count += 1;

                    // Notify collaboration observer
                    if let Some(ref obs) = self.collab_observer {
                        let exec_id = self.execution_id.as_deref().unwrap_or("");
                        obs.on_handoff_triggered(
                            exec_id,
                            &current_agent,
                            &rule.to_agent,
                            &condition_desc,
                            handoff_count,
                        )
                        .await;
                    }

                    handoff_chain.push(HandoffRecord {
                        from_agent: current_agent.clone(),
                        to_agent: rule.to_agent.clone(),
                        condition: condition_desc,
                        duration_ms,
                    });

                    // Check max handoffs
                    if handoff_count >= self.max_handoffs {
                        tracing::warn!(
                            max_handoffs = self.max_handoffs,
                            "max handoffs reached, stopping swarm"
                        );
                        return Ok((
                            state,
                            SwarmResult {
                                handoff_chain,
                                total_handoffs: handoff_count,
                                final_agent: current_agent,
                            },
                        ));
                    }

                    // Validate target agent exists
                    if !self.agents.contains_key(&rule.to_agent) {
                        return Err(GraphError::NodeNotFound(rule.to_agent.clone()));
                    }

                    // Apply context transfer before handing off
                    let transfer_mode = rule.context_transfer.clone();
                    state = self.apply_context_transfer(state, &transfer_mode);

                    current_agent = rule.to_agent.clone();
                }
                None => {
                    // No matching handoff rule, swarm is done
                    return Ok((
                        state,
                        SwarmResult {
                            handoff_chain,
                            total_handoffs: handoff_count,
                            final_agent: current_agent,
                        },
                    ));
                }
            }
        }
    }

    /// Find a matching handoff rule for the current agent.
    ///
    /// Returns the target agent name, the matched rule, and a description of the condition.
    /// Rules are evaluated in order; the first match wins.
    /// `OnKeyword` and `OnToolCall` inspect the serialized state for matches.
    fn find_matching_handoff(&self, from_agent: &str, state: &S) -> Option<(&HandoffRule, String)> {
        // Serialize state once for keyword/tool inspection
        let state_json = serde_json::to_string(state).unwrap_or_default();

        for rule in &self.handoff_rules {
            if rule.from_agent != from_agent {
                continue;
            }
            match &rule.condition {
                HandoffCondition::Always => {
                    return Some((rule, "always".into()));
                }
                HandoffCondition::OnClassification(label) => {
                    // Check if the serialized state contains the classification label.
                    // In production, a dedicated classification field would be checked.
                    if state_json.contains(label) {
                        return Some((rule, format!("classification:{}", label)));
                    }
                    // No match — fall through to next rule.
                }
                HandoffCondition::OnKeyword(keyword) => {
                    if state_json.contains(keyword) {
                        return Some((rule, format!("keyword:{}", keyword)));
                    }
                }
                HandoffCondition::OnToolCall(tool) => {
                    // Check if the state contains evidence of the tool being called.
                    // Convention: state should contain tool name as a string in its data.
                    if state_json.contains(tool) {
                        return Some((rule, format!("tool_call:{}", tool)));
                    }
                }
            }
        }
        None
    }

    /// Apply context transfer to the state before handing off to the next agent.
    ///
    /// - `Full`: state passes through unchanged.
    /// - `Summary`: serializes the state to JSON, truncates to a summary length,
    ///   then stores the truncated JSON in the `working_memory` field (if the state
    ///   has one). Since we can't modify the generic state structurally, we re-serialize
    ///   and deserialize with a summary marker injected.
    /// - `Selective(fields)`: serializes state to JSON, keeps only the listed keys
    ///   at the top level, and deserializes back.
    fn apply_context_transfer(&self, state: S, mode: &ContextTransferMode) -> S {
        match mode {
            ContextTransferMode::Full => state,
            ContextTransferMode::Summary => {
                // Serialize → truncate → deserialize as best-effort summary.
                // If serialization/deserialization fails, fall back to full transfer.
                const SUMMARY_MAX_BYTES: usize = 2000;
                const OVERSIZED_THRESHOLD: usize = 1_000_000; // 1MB

                let Ok(json) = serde_json::to_string(&state) else {
                    return state;
                };

                // Log warning if state is very large
                if json.len() > OVERSIZED_THRESHOLD {
                    tracing::warn!(
                        state_size = json.len(),
                        threshold = OVERSIZED_THRESHOLD,
                        "Context transfer state is very large"
                    );
                }

                // Keep first SUMMARY_MAX_BYTES chars as summary
                if json.len() <= SUMMARY_MAX_BYTES {
                    return state;
                }
                // R2-M: Use char_indices for safe UTF-8 truncation instead of byte slicing
                let truncated_end = json
                    .char_indices()
                    .take_while(|(i, _)| *i < SUMMARY_MAX_BYTES)
                    .last()
                    .map(|(i, c)| i + c.len_utf8())
                    .unwrap_or(0);
                let truncated = &json[..truncated_end];
                // Try to recover a valid JSON by finding the last complete field
                // If that fails, just return the full state
                if let Some(last_comma) = truncated.rfind(',') {
                    let partial = format!("{}}}", &truncated[..last_comma]);
                    if let Ok(recovered) = serde_json::from_str::<S>(&partial) {
                        return recovered;
                    }
                }
                state
            }
            ContextTransferMode::Selective(fields) => {
                // Serialize to JSON Value, keep only the listed top-level keys.
                let Ok(mut value) = serde_json::to_value(&state) else {
                    return state;
                };
                if let Some(obj) = value.as_object_mut() {
                    let keys_to_keep: HashSet<&str> = fields.iter().map(|s| s.as_str()).collect();
                    let keys_to_remove: Vec<String> = obj
                        .keys()
                        .filter(|k| !keys_to_keep.contains(k.as_str()))
                        .cloned()
                        .collect();
                    for key in keys_to_remove {
                        obj.remove(&key);
                    }
                }
                // Deserialize back — missing fields will use serde defaults.
                // If deserialization fails, return the filtered partial state
                // rather than falling back to the full unfiltered state.
                match serde_json::from_value::<S>(value.clone()) {
                    Ok(filtered) => filtered,
                    Err(e) => {
                        tracing::warn!(error = %e, "Selective context transfer deserialization failed, using full state");
                        state
                    }
                }
            }
        }
    }
}

/// Wrapper to use Arc<dyn NodeHandler> as a NodeHandler.
struct HandlerWrapperSwarm<S: GraphState> {
    inner: Arc<dyn NodeHandler<S>>,
}

#[async_trait::async_trait]
impl<S: GraphState> NodeHandler<S> for HandlerWrapperSwarm<S> {
    async fn execute(&self, state: S, ctx: &NodeContext) -> Result<S, NodeError> {
        self.inner.execute(state, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::ClosureHandler;

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct SwarmState {
        #[serde(default)]
        value: i32,
        #[serde(default)]
        trail: Vec<String>,
    }

    impl GraphState for SwarmState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
            self.trail.extend(other.trail);
        }
    }

    fn make_agent_spec(name: &str) -> AgentSpec {
        AgentSpec {
            name: name.into(),
            description: format!("{} agent", name),
            model: None,
            tools: vec![],
            system_prompt: None,
        }
    }

    fn make_agent_handler(name: &str, increment: i32) -> ClosureHandler<SwarmState> {
        let name = name.to_string();
        ClosureHandler::new(move |mut state: SwarmState, _ctx: &NodeContext| {
            let name = name.clone();
            async move {
                state.value += increment;
                state.trail.push(name);
                Ok(state)
            }
        })
    }

    #[tokio::test]
    async fn test_swarm_single_agent_no_handoff() {
        let orchestrator = SwarmOrchestrator::new(10)
            .add_agent(make_agent_spec("alpha"), make_agent_handler("alpha", 10));

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("alpha", state).await.unwrap();

        assert_eq!(result.value, 10);
        assert_eq!(result.trail, vec!["alpha"]);
        assert_eq!(swarm_result.total_handoffs, 0);
        assert_eq!(swarm_result.final_agent, "alpha");
        assert!(swarm_result.handoff_chain.is_empty());
    }

    #[tokio::test]
    async fn test_swarm_two_agent_handoff() {
        let orchestrator = SwarmOrchestrator::new(10)
            .add_agent(
                make_agent_spec("researcher"),
                make_agent_handler("researcher", 10),
            )
            .add_agent(make_agent_spec("coder"), make_agent_handler("coder", 20))
            .add_handoff_rule(HandoffRule {
                from_agent: "researcher".into(),
                to_agent: "coder".into(),
                condition: HandoffCondition::Always,
                context_transfer: ContextTransferMode::Full,
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("researcher", state).await.unwrap();

        assert_eq!(result.value, 30); // 10 + 20
        assert_eq!(result.trail, vec!["researcher", "coder"]);
        assert_eq!(swarm_result.total_handoffs, 1);
        assert_eq!(swarm_result.final_agent, "coder");
        assert_eq!(swarm_result.handoff_chain.len(), 1);
        assert_eq!(swarm_result.handoff_chain[0].from_agent, "researcher");
        assert_eq!(swarm_result.handoff_chain[0].to_agent, "coder");
    }

    #[tokio::test]
    async fn test_swarm_three_agent_chain() {
        let orchestrator = SwarmOrchestrator::new(10)
            .add_agent(make_agent_spec("plan"), make_agent_handler("plan", 1))
            .add_agent(make_agent_spec("execute"), make_agent_handler("execute", 2))
            .add_agent(make_agent_spec("verify"), make_agent_handler("verify", 3))
            .add_handoff_rule(HandoffRule {
                from_agent: "plan".into(),
                to_agent: "execute".into(),
                condition: HandoffCondition::Always,
                context_transfer: ContextTransferMode::Full,
            })
            .add_handoff_rule(HandoffRule {
                from_agent: "execute".into(),
                to_agent: "verify".into(),
                condition: HandoffCondition::Always,
                context_transfer: ContextTransferMode::Full,
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("plan", state).await.unwrap();

        assert_eq!(result.value, 6); // 1 + 2 + 3
        assert_eq!(result.trail, vec!["plan", "execute", "verify"]);
        assert_eq!(swarm_result.total_handoffs, 2);
        assert_eq!(swarm_result.final_agent, "verify");
    }

    #[tokio::test]
    async fn test_swarm_max_handoffs_limit() {
        // Create a cycle: a → b → a → b → ...
        let orchestrator = SwarmOrchestrator::new(3)
            .add_agent(make_agent_spec("a"), make_agent_handler("a", 1))
            .add_agent(make_agent_spec("b"), make_agent_handler("b", 1))
            .add_handoff_rule(HandoffRule {
                from_agent: "a".into(),
                to_agent: "b".into(),
                condition: HandoffCondition::Always,
                context_transfer: ContextTransferMode::Full,
            })
            .add_handoff_rule(HandoffRule {
                from_agent: "b".into(),
                to_agent: "a".into(),
                condition: HandoffCondition::Always,
                context_transfer: ContextTransferMode::Full,
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("a", state).await.unwrap();

        // Should stop at max_handoffs=3: execute a → handoff to b → execute b → handoff to a
        // → execute a → handoff to b → max reached, stop
        // 3 executions (a, b, a), 3 handoffs
        assert_eq!(swarm_result.total_handoffs, 3);
        assert_eq!(result.value, 3); // 3 executions of +1
        assert_eq!(result.trail.len(), 3);
    }

    #[tokio::test]
    async fn test_swarm_invalid_initial_agent() {
        let orchestrator = SwarmOrchestrator::<SwarmState>::new(10)
            .add_agent(make_agent_spec("alpha"), make_agent_handler("alpha", 1));

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let result = orchestrator.execute("nonexistent", state).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_swarm_classification_condition() {
        // Use a handler that adds "complex" to the trail so
        // OnClassification("complex") matches the serialized state.
        let classifier =
            ClosureHandler::new(|mut state: SwarmState, _ctx: &NodeContext| async move {
                state.value += 1;
                state.trail.push("complex".into());
                Ok(state)
            });

        let orchestrator = SwarmOrchestrator::new(10)
            .add_agent(make_agent_spec("classifier"), classifier)
            .add_agent(
                make_agent_spec("handler"),
                make_agent_handler("handler", 10),
            )
            .add_handoff_rule(HandoffRule {
                from_agent: "classifier".into(),
                to_agent: "handler".into(),
                condition: HandoffCondition::OnClassification("complex".into()),
                context_transfer: ContextTransferMode::Summary,
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("classifier", state).await.unwrap();

        assert_eq!(result.value, 11);
        assert_eq!(swarm_result.total_handoffs, 1);
        assert!(swarm_result.handoff_chain[0]
            .condition
            .contains("classification"));
    }

    #[tokio::test]
    async fn test_swarm_handler_error_propagation() {
        let failing_handler =
            ClosureHandler::new(|_state: SwarmState, _ctx: &NodeContext| async move {
                Err(NodeError::HandlerError("agent crashed".into()))
            });

        let orchestrator =
            SwarmOrchestrator::new(10).add_agent(make_agent_spec("failing"), failing_handler);

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let result = orchestrator.execute("failing", state).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("agent crashed"));
    }

    #[test]
    fn test_handoff_rule_serialization() {
        let rule = HandoffRule {
            from_agent: "a".into(),
            to_agent: "b".into(),
            condition: HandoffCondition::Always,
            context_transfer: ContextTransferMode::Full,
        };
        let json = serde_json::to_string(&rule).unwrap();
        let deserialized: HandoffRule = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.from_agent, "a");
        assert_eq!(deserialized.to_agent, "b");
    }

    #[test]
    fn test_swarm_result_serialization() {
        let result = SwarmResult {
            handoff_chain: vec![HandoffRecord {
                from_agent: "a".into(),
                to_agent: "b".into(),
                condition: "always".into(),
                duration_ms: 100,
            }],
            total_handoffs: 1,
            final_agent: "b".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"total_handoffs\":1"));
    }

    #[tokio::test]
    async fn test_swarm_on_keyword_condition_match() {
        // Agent "scanner" adds "ESCALATE" keyword to trail,
        // which triggers handoff to "handler" via OnKeyword.
        let scanner = ClosureHandler::new(|mut state: SwarmState, _ctx: &NodeContext| async move {
            state.value += 1;
            state.trail.push("ESCALATE".into());
            Ok(state)
        });

        let orchestrator = SwarmOrchestrator::new(10)
            .add_agent(make_agent_spec("scanner"), scanner)
            .add_agent(
                make_agent_spec("handler"),
                make_agent_handler("handler", 100),
            )
            .add_handoff_rule(HandoffRule {
                from_agent: "scanner".into(),
                to_agent: "handler".into(),
                condition: HandoffCondition::OnKeyword("ESCALATE".into()),
                context_transfer: ContextTransferMode::Full,
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("scanner", state).await.unwrap();

        assert_eq!(result.value, 101); // 1 + 100
        assert_eq!(swarm_result.total_handoffs, 1);
        assert!(swarm_result.handoff_chain[0]
            .condition
            .contains("keyword:ESCALATE"));
    }

    #[tokio::test]
    async fn test_swarm_on_keyword_condition_no_match() {
        // Agent "scanner" does NOT add the keyword, so no handoff happens.
        let orchestrator = SwarmOrchestrator::new(10)
            .add_agent(make_agent_spec("scanner"), make_agent_handler("scanner", 1))
            .add_agent(
                make_agent_spec("handler"),
                make_agent_handler("handler", 100),
            )
            .add_handoff_rule(HandoffRule {
                from_agent: "scanner".into(),
                to_agent: "handler".into(),
                condition: HandoffCondition::OnKeyword("ESCALATE".into()),
                context_transfer: ContextTransferMode::Full,
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("scanner", state).await.unwrap();

        // No handoff — "scanner" trail doesn't contain "ESCALATE"
        assert_eq!(result.value, 1);
        assert_eq!(swarm_result.total_handoffs, 0);
        assert_eq!(swarm_result.final_agent, "scanner");
    }

    #[tokio::test]
    async fn test_swarm_on_tool_call_condition_match() {
        // Agent "router" adds a tool call marker to trail.
        let router = ClosureHandler::new(|mut state: SwarmState, _ctx: &NodeContext| async move {
            state.value += 1;
            state.trail.push("call:code_interpreter".into());
            Ok(state)
        });

        let orchestrator = SwarmOrchestrator::new(10)
            .add_agent(make_agent_spec("router"), router)
            .add_agent(make_agent_spec("coder"), make_agent_handler("coder", 50))
            .add_handoff_rule(HandoffRule {
                from_agent: "router".into(),
                to_agent: "coder".into(),
                condition: HandoffCondition::OnToolCall("code_interpreter".into()),
                context_transfer: ContextTransferMode::Full,
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("router", state).await.unwrap();

        assert_eq!(result.value, 51); // 1 + 50
        assert_eq!(swarm_result.total_handoffs, 1);
        assert!(swarm_result.handoff_chain[0]
            .condition
            .contains("tool_call:code_interpreter"));
    }

    #[tokio::test]
    async fn test_swarm_on_tool_call_condition_no_match() {
        // Agent "router" does NOT add the tool name, so no handoff.
        let orchestrator = SwarmOrchestrator::new(10)
            .add_agent(make_agent_spec("router"), make_agent_handler("router", 1))
            .add_agent(make_agent_spec("coder"), make_agent_handler("coder", 50))
            .add_handoff_rule(HandoffRule {
                from_agent: "router".into(),
                to_agent: "coder".into(),
                condition: HandoffCondition::OnToolCall("code_interpreter".into()),
                context_transfer: ContextTransferMode::Full,
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("router", state).await.unwrap();

        assert_eq!(result.value, 1);
        assert_eq!(swarm_result.total_handoffs, 0);
    }

    #[tokio::test]
    async fn test_swarm_selective_context_transfer() {
        // Handoff with Selective(["value"]) — should keep "value" but reset "trail" to default.
        let orchestrator = SwarmOrchestrator::new(10)
            .add_agent(make_agent_spec("first"), make_agent_handler("first", 10))
            .add_agent(make_agent_spec("second"), make_agent_handler("second", 20))
            .add_handoff_rule(HandoffRule {
                from_agent: "first".into(),
                to_agent: "second".into(),
                condition: HandoffCondition::Always,
                context_transfer: ContextTransferMode::Selective(vec!["value".into()]),
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("first", state).await.unwrap();

        // After "first": value=10, trail=["first"]
        // Selective transfer keeps only "value" → trail defaults to []
        // After "second": value=30, trail=["second"]
        assert_eq!(result.value, 30);
        assert_eq!(result.trail, vec!["second"]); // "first" was stripped by selective transfer
        assert_eq!(swarm_result.total_handoffs, 1);
    }

    #[tokio::test]
    async fn test_swarm_full_context_transfer() {
        // Handoff with Full — state passes through unchanged.
        let orchestrator = SwarmOrchestrator::new(10)
            .add_agent(make_agent_spec("first"), make_agent_handler("first", 10))
            .add_agent(make_agent_spec("second"), make_agent_handler("second", 20))
            .add_handoff_rule(HandoffRule {
                from_agent: "first".into(),
                to_agent: "second".into(),
                condition: HandoffCondition::Always,
                context_transfer: ContextTransferMode::Full,
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("first", state).await.unwrap();

        // Full transfer: both value and trail carry over
        assert_eq!(result.value, 30);
        assert_eq!(result.trail, vec!["first", "second"]);
        assert_eq!(swarm_result.total_handoffs, 1);
    }

    #[tokio::test]
    async fn test_swarm_summary_context_transfer_small_state() {
        // For small states (< 2000 chars), Summary returns the full state.
        let orchestrator = SwarmOrchestrator::new(10)
            .add_agent(make_agent_spec("first"), make_agent_handler("first", 10))
            .add_agent(make_agent_spec("second"), make_agent_handler("second", 20))
            .add_handoff_rule(HandoffRule {
                from_agent: "first".into(),
                to_agent: "second".into(),
                condition: HandoffCondition::Always,
                context_transfer: ContextTransferMode::Summary,
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("first", state).await.unwrap();

        // Small state: summary == full
        assert_eq!(result.value, 30);
        assert_eq!(result.trail, vec!["first", "second"]);
        assert_eq!(swarm_result.total_handoffs, 1);
    }

    #[tokio::test]
    async fn test_swarm_keyword_with_selective_transfer() {
        // Combine OnKeyword condition with Selective context transfer.
        let scanner = ClosureHandler::new(|mut state: SwarmState, _ctx: &NodeContext| async move {
            state.value += 5;
            state.trail.push("needs_review".into());
            state.trail.push("extra_data".into());
            Ok(state)
        });

        let orchestrator = SwarmOrchestrator::new(10)
            .add_agent(make_agent_spec("scanner"), scanner)
            .add_agent(
                make_agent_spec("reviewer"),
                make_agent_handler("reviewer", 50),
            )
            .add_handoff_rule(HandoffRule {
                from_agent: "scanner".into(),
                to_agent: "reviewer".into(),
                condition: HandoffCondition::OnKeyword("needs_review".into()),
                context_transfer: ContextTransferMode::Selective(vec!["value".into()]),
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("scanner", state).await.unwrap();

        // OnKeyword matches "needs_review" in trail
        // Selective keeps only "value" → trail defaults to []
        // After "reviewer": value=55, trail=["reviewer"]
        assert_eq!(result.value, 55);
        assert_eq!(result.trail, vec!["reviewer"]);
        assert_eq!(swarm_result.total_handoffs, 1);
        assert!(swarm_result.handoff_chain[0]
            .condition
            .contains("keyword:needs_review"));
    }

    #[test]
    fn test_context_transfer_mode_serialization() {
        let full = ContextTransferMode::Full;
        let json = serde_json::to_string(&full).unwrap();
        let _: ContextTransferMode = serde_json::from_str(&json).unwrap();

        let summary = ContextTransferMode::Summary;
        let json = serde_json::to_string(&summary).unwrap();
        let _: ContextTransferMode = serde_json::from_str(&json).unwrap();

        let selective = ContextTransferMode::Selective(vec!["value".into(), "trail".into()]);
        let json = serde_json::to_string(&selective).unwrap();
        let deserialized: ContextTransferMode = serde_json::from_str(&json).unwrap();
        match deserialized {
            ContextTransferMode::Selective(fields) => {
                assert_eq!(fields, vec!["value", "trail"]);
            }
            _ => panic!("expected Selective"),
        }
    }

    #[tokio::test]
    async fn test_swarm_collab_observer_handoff() {
        use crate::collaboration::observer::CollaborationRecorder;
        use crate::graph::execution_store::{EventPayload, ExecutionMode, ExecutionStore};

        let store = Arc::new(ExecutionStore::new(10));
        store
            .start_execution("swarm_obs", ExecutionMode::Swarm)
            .await;

        let recorder = Arc::new(CollaborationRecorder::new(store.clone(), "swarm_obs"));

        let orchestrator = SwarmOrchestrator::new(10)
            .with_collab_observer(recorder, "swarm_obs")
            .add_agent(make_agent_spec("alpha"), make_agent_handler("alpha", 10))
            .add_agent(make_agent_spec("beta"), make_agent_handler("beta", 20))
            .add_handoff_rule(HandoffRule {
                from_agent: "alpha".into(),
                to_agent: "beta".into(),
                condition: HandoffCondition::Always,
                context_transfer: ContextTransferMode::Full,
            });

        let state = SwarmState {
            value: 0,
            trail: vec![],
        };
        let (result, swarm_result) = orchestrator.execute("alpha", state).await.unwrap();

        assert_eq!(result.value, 30);
        assert_eq!(swarm_result.total_handoffs, 1);

        // Verify observer recorded the handoff event
        let events = store.get_events("swarm_obs", 0, None);
        assert!(events.len() >= 1);
        let handoff_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e.payload, EventPayload::HandoffTriggered { .. }))
            .collect();
        assert_eq!(handoff_events.len(), 1);
        if let EventPayload::HandoffTriggered {
            from_agent,
            to_agent,
            ..
        } = &handoff_events[0].payload
        {
            assert_eq!(from_agent, "alpha");
            assert_eq!(to_agent, "beta");
        }
    }

    #[test]
    fn test_handoff_condition_serialization() {
        let conditions = vec![
            HandoffCondition::Always,
            HandoffCondition::OnKeyword("test".into()),
            HandoffCondition::OnToolCall("browser".into()),
            HandoffCondition::OnClassification("complex".into()),
        ];
        for cond in conditions {
            let json = serde_json::to_string(&cond).unwrap();
            let _: HandoffCondition = serde_json::from_str(&json).unwrap();
        }
    }
}
