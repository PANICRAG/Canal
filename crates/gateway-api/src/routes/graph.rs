//! Graph and collaboration endpoints.
//!
//! This module provides REST API endpoints for graph-based workflow execution
//! and multi-agent collaboration modes:
//!
//! - Template management for workflow patterns
//! - Graph execution with checkpointing
//! - Collaboration mode selection and execution
//!
//! # Feature Gate
//!
//! This module requires the `orchestration` feature which implies `graph` and `collaboration`.

use std::collections::HashMap;

use axum::http::StatusCode;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use gateway_core::collaboration::{
    CollaborationMode, HandoffRule, TemplateConfig, TemplatePattern, UserWorkflowTemplate,
    WorkflowTemplate,
};
use serde::{Deserialize, Serialize};

use crate::{error::ApiError, state::AppState};

/// Create the graph routes
pub fn routes() -> Router<AppState> {
    use axum::routing::delete;

    Router::new()
        // Template management (built-in)
        .route("/templates", get(list_templates))
        .route("/templates/{id}", get(get_template))
        // Workflow registry (built-in + custom templates)
        .route("/workflows", get(list_workflow_templates).post(register_custom_template))
        .route("/workflows/{id}", delete(delete_custom_template))
        // Collaboration mode execution
        .route("/execute/auto", post(execute_auto_collaboration))
        .route("/execute/direct", post(execute_direct))
        .route("/execute/swarm", post(execute_swarm))
        .route("/execute/expert", post(execute_expert))
}

// ============================================================================
// Template Endpoints
// ============================================================================

/// Template response for API
#[derive(Debug, Serialize)]
pub struct TemplateResponse {
    /// Template ID
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Description of the template
    pub description: String,
    /// Pattern type
    pub pattern: String,
    /// Default configuration
    pub config: TemplateConfigResponse,
}

/// Template configuration response
#[derive(Debug, Serialize)]
pub struct TemplateConfigResponse {
    pub max_retries: u32,
    pub parallel_branches: usize,
    pub max_depth: usize,
}

impl From<&TemplateConfig> for TemplateConfigResponse {
    fn from(c: &TemplateConfig) -> Self {
        Self {
            max_retries: c.max_retries,
            parallel_branches: c.parallel_branches,
            max_depth: c.max_depth,
        }
    }
}

/// Templates list response
#[derive(Debug, Serialize)]
pub struct TemplatesListResponse {
    pub templates: Vec<TemplateResponse>,
    pub count: usize,
}

/// List all available workflow templates
pub async fn list_templates(State(state): State<AppState>) -> Json<TemplatesListResponse> {
    let registry = state.template_registry();

    let template_ids = registry.list();
    let templates: Vec<TemplateResponse> = template_ids
        .iter()
        .filter_map(|id| {
            registry.get(id).map(|t| TemplateResponse {
                id: t.id.clone(),
                name: t.name.clone(),
                description: t.description.clone(),
                pattern: format!("{:?}", t.pattern),
                config: TemplateConfigResponse::from(&t.default_config),
            })
        })
        .collect();

    let count = templates.len();
    Json(TemplatesListResponse { templates, count })
}

/// Get a specific template by ID
pub async fn get_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TemplateResponse>, ApiError> {
    let registry = state.template_registry();

    registry
        .get(&id)
        .map(|t| {
            Json(TemplateResponse {
                id: t.id.clone(),
                name: t.name.clone(),
                description: t.description.clone(),
                pattern: format!("{:?}", t.pattern),
                config: TemplateConfigResponse::from(&t.default_config),
            })
        })
        .ok_or_else(|| ApiError::not_found(format!("Template not found: {}", id)))
}

// ============================================================================
// Collaboration Execution Endpoints
// ============================================================================

/// Request for auto collaboration mode execution
#[derive(Debug, Deserialize)]
pub struct AutoCollaborationRequest {
    /// The task to execute
    pub task: String,
    /// Optional model to use
    #[serde(default)]
    pub model: Option<String>,
    /// Global token budget for the execution (creates an ExecutionBudget)
    #[serde(default)]
    pub budget_tokens: Option<u32>,
    /// Enable DAG-level automatic parallel scheduling
    #[serde(default)]
    pub dag_scheduling: Option<bool>,
    /// Error strategy: "fail_fast" (default), "continue", "retry"
    #[serde(default)]
    pub error_strategy: Option<String>,
}

/// Response from collaboration execution
#[derive(Debug, Serialize)]
pub struct CollaborationExecutionResponse {
    /// Whether execution succeeded
    pub success: bool,
    /// The collaboration mode that was used
    pub mode: String,
    /// Number of agents involved
    pub agents_used: usize,
    /// Total steps/handoffs/dispatches
    pub total_steps: usize,
    /// Result summary
    pub summary: Option<String>,
    /// Final response text
    pub response: String,
}

/// Execute with auto-selected collaboration mode
pub async fn execute_auto_collaboration(
    State(state): State<AppState>,
    Json(request): Json<AutoCollaborationRequest>,
) -> Result<Json<CollaborationExecutionResponse>, ApiError> {
    tracing::info!(
        task = %request.task,
        budget_tokens = ?request.budget_tokens,
        dag_scheduling = ?request.dag_scheduling,
        error_strategy = ?request.error_strategy,
        "Executing with auto-selected collaboration mode"
    );

    let factory = &state.agent_factory;

    // Execute with auto-selected collaboration mode and A23 config
    let result = factory
        .execute_with_collaboration_config(
            &request.task,
            None,
            request.budget_tokens,
            request.dag_scheduling,
            request.error_strategy.as_deref(),
            None, // user_id — no auth context yet
        )
        .await
        .map_err(|e| ApiError::internal(format!("Graph execution failed: {}", e)))?;

    Ok(Json(CollaborationExecutionResponse {
        success: true,
        mode: "auto".to_string(),
        agents_used: 1,
        total_steps: result.messages.len(),
        summary: Some(format!("Completed {} turns", result.messages.len())),
        response: result.response,
    }))
}

/// Request for direct mode execution
#[derive(Debug, Deserialize)]
pub struct DirectExecutionRequest {
    /// The task to execute
    pub task: String,
    /// Optional model to use
    #[serde(default)]
    pub model: Option<String>,
}

/// Execute in direct mode (single agent)
pub async fn execute_direct(
    State(state): State<AppState>,
    Json(request): Json<DirectExecutionRequest>,
) -> Result<Json<CollaborationExecutionResponse>, ApiError> {
    tracing::info!(
        task = %request.task,
        "Executing in direct mode"
    );

    let factory = &state.agent_factory;

    // Execute with direct collaboration mode
    let result = factory
        .execute_with_collaboration(&request.task, Some(CollaborationMode::Direct))
        .await
        .map_err(|e| ApiError::internal(format!("Direct execution failed: {}", e)))?;

    Ok(Json(CollaborationExecutionResponse {
        success: true,
        mode: "direct".to_string(),
        agents_used: 1,
        total_steps: result.messages.len(),
        summary: Some(format!("Completed {} turns", result.messages.len())),
        response: result.response,
    }))
}

/// Request for swarm mode execution
#[derive(Debug, Deserialize)]
pub struct SwarmExecutionRequest {
    /// The task to execute
    pub task: String,
    /// Initial agent to start with
    pub initial_agent: String,
    /// Handoff rules between agents (optional)
    #[serde(default)]
    pub handoff_rules: Vec<HandoffRuleRequest>,
    /// Per-agent model overrides (agent_name → model_name)
    #[serde(default)]
    pub agent_models: HashMap<String, String>,
}

/// Handoff rule request
#[derive(Debug, Clone, Deserialize)]
pub struct HandoffRuleRequest {
    /// Source agent name
    pub from: String,
    /// Target agent name
    pub to: String,
    /// Condition type: "tool_call", "keyword", "classification", "always"
    pub condition_type: String,
    /// Condition value (e.g., tool name, keyword, classification label)
    #[serde(default)]
    pub condition_value: Option<String>,
}

/// Execute in swarm mode (agent-to-agent handoffs)
pub async fn execute_swarm(
    State(state): State<AppState>,
    Json(request): Json<SwarmExecutionRequest>,
) -> Result<Json<CollaborationExecutionResponse>, ApiError> {
    tracing::info!(
        task = %request.task,
        initial_agent = %request.initial_agent,
        "Executing in swarm mode"
    );

    let factory = &state.agent_factory;

    // Convert handoff rule requests to HandoffRule
    let handoff_rules: Vec<HandoffRule> = request
        .handoff_rules
        .into_iter()
        .map(|r| {
            use gateway_core::collaboration::{ContextTransferMode, HandoffCondition};
            HandoffRule {
                from_agent: r.from,
                to_agent: r.to,
                condition: match r.condition_type.as_str() {
                    "tool_call" => {
                        HandoffCondition::OnToolCall(r.condition_value.unwrap_or_default())
                    }
                    "keyword" => HandoffCondition::OnKeyword(r.condition_value.unwrap_or_default()),
                    "classification" => {
                        HandoffCondition::OnClassification(r.condition_value.unwrap_or_default())
                    }
                    _ => HandoffCondition::Always,
                },
                context_transfer: ContextTransferMode::Full,
            }
        })
        .collect();

    // Execute with swarm collaboration mode
    let mode = CollaborationMode::Swarm {
        initial_agent: request.initial_agent,
        handoff_rules,
        agent_models: request.agent_models,
    };

    let result = factory
        .execute_with_collaboration(&request.task, Some(mode))
        .await
        .map_err(|e| ApiError::internal(format!("Swarm execution failed: {}", e)))?;

    Ok(Json(CollaborationExecutionResponse {
        success: true,
        mode: "swarm".to_string(),
        agents_used: 1, // Would be updated with actual agent count if available
        total_steps: result.messages.len(),
        summary: Some(format!(
            "Completed {} turns in swarm mode",
            result.messages.len()
        )),
        response: result.response,
    }))
}

/// Request for expert mode execution
#[derive(Debug, Deserialize)]
pub struct ExpertExecutionRequest {
    /// The task to execute
    pub task: String,
    /// Supervisor agent name
    pub supervisor: String,
    /// Specialist agent names
    pub specialists: Vec<String>,
    /// Model override for the supervisor agent
    #[serde(default)]
    pub supervisor_model: Option<String>,
    /// Default model for all specialists (fallback when no per-specialist override)
    #[serde(default)]
    pub default_specialist_model: Option<String>,
    /// Per-specialist model overrides (specialist_name → model_name)
    #[serde(default)]
    pub specialist_models: HashMap<String, String>,
}

/// Execute in expert mode (supervisor + specialists)
pub async fn execute_expert(
    State(state): State<AppState>,
    Json(request): Json<ExpertExecutionRequest>,
) -> Result<Json<CollaborationExecutionResponse>, ApiError> {
    tracing::info!(
        task = %request.task,
        supervisor = %request.supervisor,
        specialists = ?request.specialists,
        "Executing in expert mode"
    );

    let factory = &state.agent_factory;

    // Execute with expert collaboration mode
    let mode = CollaborationMode::Expert {
        supervisor: request.supervisor,
        specialists: request.specialists.clone(),
        supervisor_model: request.supervisor_model,
        default_specialist_model: request.default_specialist_model,
        specialist_models: request.specialist_models,
    };

    let result = factory
        .execute_with_collaboration(&request.task, Some(mode))
        .await
        .map_err(|e| ApiError::internal(format!("Expert execution failed: {}", e)))?;

    Ok(Json(CollaborationExecutionResponse {
        success: true,
        mode: "expert".to_string(),
        agents_used: 1 + request.specialists.len(),
        total_steps: result.messages.len(),
        summary: Some(format!(
            "Completed {} turns with supervisor and {} specialists",
            result.messages.len(),
            request.specialists.len()
        )),
        response: result.response,
    }))
}

// ============================================================================
// Workflow Registry Endpoints (built-in + custom templates)
// ============================================================================

/// List all workflow templates (built-in + custom) with usage statistics.
pub async fn list_workflow_templates(State(state): State<AppState>) -> Json<serde_json::Value> {
    let registry = &state.workflow_registry;
    let templates = registry.list_all().await;
    let count = templates.len();

    Json(serde_json::json!({
        "templates": templates,
        "count": count,
    }))
}

/// Request to register a custom workflow template
#[derive(Debug, Deserialize)]
pub struct RegisterTemplateRequest {
    /// Unique template ID
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Description
    pub description: String,
    /// Pattern type: "simple", "with_verification", "plan_execute", "full", "research"
    pub pattern: String,
    /// Optional creator name
    #[serde(default)]
    pub created_by: Option<String>,
    /// Whether the template is published (visible to all users)
    #[serde(default)]
    pub published: bool,
}

/// Register a custom workflow template.
pub async fn register_custom_template(
    State(state): State<AppState>,
    Json(request): Json<RegisterTemplateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let pattern = match request.pattern.as_str() {
        "simple" => TemplatePattern::Simple,
        "with_verification" => TemplatePattern::WithVerification,
        "plan_execute" => TemplatePattern::PlanExecute,
        "full" => TemplatePattern::Full,
        "research" => TemplatePattern::Research,
        other => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("Unknown pattern: {}. Valid: simple, with_verification, plan_execute, full, research", other),
            ));
        }
    };

    let template = WorkflowTemplate {
        id: request.id.clone(),
        name: request.name,
        description: request.description,
        pattern,
        default_config: TemplateConfig::default(),
    };

    let user_template = UserWorkflowTemplate {
        template,
        created_by: request.created_by,
        published: request.published,
        usage_count: 0,
        avg_execution_ms: None,
    };

    state
        .workflow_registry
        .register_custom(user_template)
        .await
        .map_err(|e| ApiError::new(StatusCode::CONFLICT, e))?;

    Ok(Json(serde_json::json!({
        "registered": true,
        "id": request.id,
    })))
}

/// Delete a custom workflow template.
pub async fn delete_custom_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let deleted = state.workflow_registry.delete_custom(&id).await;

    match deleted {
        Some(_) => Ok(Json(serde_json::json!({
            "deleted": true,
            "id": id,
        }))),
        None => Err(ApiError::not_found(format!(
            "Custom template not found: {}",
            id
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_config_response_conversion() {
        let config = TemplateConfig {
            max_retries: 5,
            parallel_branches: 4,
            max_depth: 10,
        };
        let response = TemplateConfigResponse::from(&config);
        assert_eq!(response.max_retries, 5);
        assert_eq!(response.parallel_branches, 4);
        assert_eq!(response.max_depth, 10);
    }

    #[test]
    fn test_handoff_rule_request_deserialization() {
        let json = r#"{
            "from": "agent_a",
            "to": "agent_b",
            "condition_type": "keyword",
            "condition_value": "handoff"
        }"#;
        let rule: HandoffRuleRequest = serde_json::from_str(json).unwrap();
        assert_eq!(rule.from, "agent_a");
        assert_eq!(rule.to, "agent_b");
        assert_eq!(rule.condition_type, "keyword");
        assert_eq!(rule.condition_value, Some("handoff".to_string()));
    }

    #[test]
    fn test_auto_collaboration_request_deserialization() {
        let json = r#"{
            "task": "Write a hello world program"
        }"#;
        let request: AutoCollaborationRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.task, "Write a hello world program");
        assert!(request.model.is_none());
    }

    #[test]
    fn test_swarm_request_with_rules() {
        let json = r#"{
            "task": "Research and code",
            "initial_agent": "researcher",
            "handoff_rules": [
                {
                    "from": "researcher",
                    "to": "coder",
                    "condition_type": "keyword",
                    "condition_value": "implement"
                }
            ]
        }"#;
        let request: SwarmExecutionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.task, "Research and code");
        assert_eq!(request.initial_agent, "researcher");
        assert_eq!(request.handoff_rules.len(), 1);
    }

    #[test]
    fn test_expert_request_deserialization() {
        let json = r#"{
            "task": "Build a web app",
            "supervisor": "architect",
            "specialists": ["frontend", "backend", "database"]
        }"#;
        let request: ExpertExecutionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.task, "Build a web app");
        assert_eq!(request.supervisor, "architect");
        assert_eq!(request.specialists.len(), 3);
        // Defaults for model fields
        assert!(request.supervisor_model.is_none());
        assert!(request.default_specialist_model.is_none());
        assert!(request.specialist_models.is_empty());
    }

    #[test]
    fn test_expert_request_with_model_fields() {
        let json = r#"{
            "task": "Take a screenshot and summarize",
            "supervisor": "coordinator",
            "specialists": ["browser_agent", "summarizer"],
            "supervisor_model": "qwen-max",
            "default_specialist_model": "qwen-turbo",
            "specialist_models": {
                "browser_agent": "qwen3-vl-plus"
            }
        }"#;
        let request: ExpertExecutionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.supervisor_model, Some("qwen-max".into()));
        assert_eq!(request.default_specialist_model, Some("qwen-turbo".into()));
        assert_eq!(
            request.specialist_models.get("browser_agent"),
            Some(&"qwen3-vl-plus".into())
        );
        // summarizer not in specialist_models → should use default_specialist_model
        assert!(request.specialist_models.get("summarizer").is_none());
    }

    #[test]
    fn test_swarm_request_with_agent_models() {
        let json = r#"{
            "task": "Research and code",
            "initial_agent": "researcher",
            "handoff_rules": [],
            "agent_models": {
                "researcher": "qwen-max",
                "coder": "qwen-coder"
            }
        }"#;
        let request: SwarmExecutionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.agent_models.len(), 2);
        assert_eq!(
            request.agent_models.get("researcher"),
            Some(&"qwen-max".into())
        );
    }

    #[test]
    fn test_swarm_request_backward_compat() {
        // Old format without agent_models
        let json = r#"{
            "task": "Research",
            "initial_agent": "researcher"
        }"#;
        let request: SwarmExecutionRequest = serde_json::from_str(json).unwrap();
        assert!(request.agent_models.is_empty());
        assert!(request.handoff_rules.is_empty());
    }
}
