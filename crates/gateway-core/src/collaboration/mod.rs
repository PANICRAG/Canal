//! Multi-agent collaboration modes.
//!
//! This module implements three collaboration patterns for multi-agent orchestration:
//!
//! - **Direct**: Simple single-agent execution (wraps existing AgentRunner pattern)
//! - **Swarm**: Agent-to-agent handoff with context transfer (inspired by OpenAI Swarm)
//! - **Expert**: Supervisor dispatches to specialist pool with quality gates
//!
//! Additionally, provides workflow templates for common graph patterns.
//!
//! # Feature Gate
//!
//! This module requires the `collaboration` feature which implies `graph`.
//!
//! ```toml
//! [features]
//! collaboration = ["graph"]
//! ```

pub mod approval;
pub mod clarification;
pub mod direct;
pub mod expert;
pub mod judge;
pub mod observer;
pub mod planner;
pub mod prd;
pub mod prd_approval;
pub mod quality;
pub mod registry;
pub mod state;
pub mod swarm;
pub mod templates;

// Re-export core types.
pub use direct::DirectMode;
pub use expert::{
    DispatchRecord, ExpertOrchestrator, ExpertResult, SpecialistSpec, SupervisorConfig,
};
pub use observer::{CollaborationObserver, CollaborationRecorder, NoOpCollaborationObserver};
pub use quality::{CompositeQualityGate, QualityGate, QualityResult, ThresholdQualityGate};
pub use registry::{UserWorkflowTemplate, WorkflowRegistry, WorkflowTemplateInfo};
pub use swarm::{
    ContextTransferMode, HandoffCondition, HandoffRecord, HandoffRule, SwarmOrchestrator,
    SwarmResult,
};
pub use templates::{TemplateConfig, TemplatePattern, TemplateRegistry, WorkflowTemplate};

// Judge types (A39)
pub use judge::{JudgeConfig, StepJudge};

// Planner types (A24)
pub use planner::{
    ExecutionPlan, PlanProgressEvent, PlanStep, PlanStepPreview, PlannerConfig, StepDependency,
    TaskPlanner, ToolCategory as PlanToolCategory,
};

// Plan approval types (human-in-the-loop)
pub use approval::{PendingPlanApprovals, PlanApprovalDecision, PlanStepReview};

// Typed state overlay (A44)
pub use state::{
    ApprovalDecision, PipelinePhase, PlanExecuteState, StepExecutionState, StepStatus,
    PLAN_STATE_KEY,
};

// PRD pipeline types (A43)
pub use clarification::PendingClarifications;
pub use prd::{
    assess_complexity, build_prd_assembler_prompt, build_step_planner_distilled_context,
    build_step_planner_prd_context, compress_prd, distill_core_concepts,
    generate_prd_expanded_tool_def, generate_prd_tool_def, get_template_questions, is_coding_task,
    parse_prd_response, parse_research_response, submit_research_tool_def,
    RESEARCH_PLANNER_SYSTEM_PROMPT,
};
pub use prd::{
    ClarificationAnswerType, ClarificationResponse, ClarifyingQuestion, CoreConcepts, DistilledPrd,
    ImplementationApproach, PrdApprovalDecision, PrdDocument, ResearchOutput, Risk, TaskComplexity,
    TaskType,
};
pub use prd_approval::PendingPrdApprovals;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Describes an agent's specification for use in collaboration modes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Unique agent identifier.
    pub name: String,
    /// Description of the agent's capabilities.
    pub description: String,
    /// The model to use for this agent (e.g., "claude-sonnet-4-5-20250929").
    pub model: Option<String>,
    /// Tool names this agent has access to.
    pub tools: Vec<String>,
    /// System prompt for this agent.
    pub system_prompt: Option<String>,
}

/// Collaboration mode selection for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollaborationMode {
    /// Simple single-agent execution.
    Direct,
    /// Plan-Execute mode: Planner → Executor → Synthesizer.
    /// Used for multi-step tasks that benefit from explicit planning.
    PlanExecute,
    /// Swarm mode with agent-to-agent handoffs.
    /// Deprecated: use PlanExecute with per-step `executor_agent` fields instead.
    #[deprecated(note = "use PlanExecute with per-step executor_agent/executor_model fields")]
    Swarm {
        /// Initial agent to start with.
        initial_agent: String,
        /// Handoff rules between agents.
        handoff_rules: Vec<HandoffRule>,
        /// Per-agent model overrides (agent_name → model_name).
        #[serde(default)]
        agent_models: HashMap<String, String>,
    },
    /// Expert mode with supervisor + specialists.
    /// Deprecated: use PlanExecute with judge node for quality evaluation.
    #[deprecated(note = "use PlanExecute with judge node for quality evaluation")]
    Expert {
        /// Supervisor agent name.
        supervisor: String,
        /// Specialist agent names.
        specialists: Vec<String>,
        /// Model override for the supervisor agent.
        #[serde(default)]
        supervisor_model: Option<String>,
        /// Default model for all specialists (used when no per-specialist override).
        #[serde(default)]
        default_specialist_model: Option<String>,
        /// Per-specialist model overrides (specialist_name → model_name).
        #[serde(default)]
        specialist_models: HashMap<String, String>,
    },
    /// Graph mode with a pre-built state graph.
    Graph {
        /// Graph template or ID.
        graph_id: String,
    },
}

/// Result of a collaboration execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaborationResult {
    /// The collaboration mode that was used.
    pub mode: String,
    /// Whether the collaboration completed successfully.
    pub success: bool,
    /// Number of agents involved.
    pub agents_used: usize,
    /// Total steps/handoffs/dispatches performed.
    pub total_steps: usize,
    /// Summary of what happened.
    pub summary: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_spec_creation() {
        let spec = AgentSpec {
            name: "researcher".into(),
            description: "Research agent".into(),
            model: Some("claude-sonnet-4-5-20250929".into()),
            tools: vec!["web_search".into(), "read_file".into()],
            system_prompt: Some("You are a research assistant.".into()),
        };
        assert_eq!(spec.name, "researcher");
        assert_eq!(spec.tools.len(), 2);
    }

    #[test]
    fn test_collaboration_mode_serialization() {
        let mode = CollaborationMode::Direct;
        let json = serde_json::to_string(&mode).unwrap();
        assert!(json.contains("Direct"));

        let mode = CollaborationMode::Expert {
            supervisor: "supervisor".into(),
            specialists: vec!["coder".into(), "reviewer".into()],
            supervisor_model: None,
            default_specialist_model: None,
            specialist_models: HashMap::new(),
        };
        let json = serde_json::to_string(&mode).unwrap();
        assert!(json.contains("supervisor"));
        assert!(json.contains("coder"));
    }

    #[test]
    fn test_expert_mode_with_model_fields() {
        let mut specialist_models = HashMap::new();
        specialist_models.insert("browser_agent".into(), "qwen3-vl-plus".into());

        let mode = CollaborationMode::Expert {
            supervisor: "coordinator".into(),
            specialists: vec!["browser_agent".into(), "coder".into()],
            supervisor_model: Some("qwen-max".into()),
            default_specialist_model: Some("qwen-turbo".into()),
            specialist_models,
        };

        let json = serde_json::to_string(&mode).unwrap();
        assert!(json.contains("qwen-max"));
        assert!(json.contains("qwen3-vl-plus"));
        assert!(json.contains("qwen-turbo"));

        // Roundtrip
        let deserialized: CollaborationMode = serde_json::from_str(&json).unwrap();
        if let CollaborationMode::Expert {
            supervisor_model,
            default_specialist_model,
            specialist_models,
            ..
        } = deserialized
        {
            assert_eq!(supervisor_model, Some("qwen-max".into()));
            assert_eq!(default_specialist_model, Some("qwen-turbo".into()));
            assert_eq!(
                specialist_models.get("browser_agent"),
                Some(&"qwen3-vl-plus".into())
            );
        } else {
            panic!("Expected Expert mode");
        }
    }

    #[test]
    fn test_swarm_mode_with_model_fields() {
        let mut agent_models = HashMap::new();
        agent_models.insert("researcher".into(), "qwen-max".into());

        let mode = CollaborationMode::Swarm {
            initial_agent: "researcher".into(),
            handoff_rules: vec![],
            agent_models,
        };

        let json = serde_json::to_string(&mode).unwrap();
        assert!(json.contains("qwen-max"));

        let deserialized: CollaborationMode = serde_json::from_str(&json).unwrap();
        if let CollaborationMode::Swarm { agent_models, .. } = deserialized {
            assert_eq!(agent_models.get("researcher"), Some(&"qwen-max".into()));
        } else {
            panic!("Expected Swarm mode");
        }
    }

    #[test]
    fn test_backward_compatible_expert_deserialization() {
        // JSON without model fields (old format) should deserialize with defaults
        let json = r#"{
            "Expert": {
                "supervisor": "lead",
                "specialists": ["coder", "reviewer"]
            }
        }"#;
        let mode: CollaborationMode = serde_json::from_str(json).unwrap();
        if let CollaborationMode::Expert {
            supervisor,
            specialists,
            supervisor_model,
            default_specialist_model,
            specialist_models,
        } = mode
        {
            assert_eq!(supervisor, "lead");
            assert_eq!(specialists.len(), 2);
            assert_eq!(supervisor_model, None);
            assert_eq!(default_specialist_model, None);
            assert!(specialist_models.is_empty());
        } else {
            panic!("Expected Expert mode");
        }
    }

    #[test]
    fn test_backward_compatible_swarm_deserialization() {
        // JSON without agent_models (old format) should deserialize with empty map
        let json = r#"{
            "Swarm": {
                "initial_agent": "agent_a",
                "handoff_rules": []
            }
        }"#;
        let mode: CollaborationMode = serde_json::from_str(json).unwrap();
        if let CollaborationMode::Swarm {
            initial_agent,
            agent_models,
            ..
        } = mode
        {
            assert_eq!(initial_agent, "agent_a");
            assert!(agent_models.is_empty());
        } else {
            panic!("Expected Swarm mode");
        }
    }

    #[test]
    fn test_collaboration_result() {
        let result = CollaborationResult {
            mode: "swarm".into(),
            success: true,
            agents_used: 3,
            total_steps: 5,
            summary: Some("Completed research and coding".into()),
        };
        assert!(result.success);
        assert_eq!(result.agents_used, 3);
    }
}
