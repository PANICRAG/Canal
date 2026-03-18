//! Workflow endpoints
//!
//! This module provides REST API endpoints for workflow management including:
//! - CRUD operations for workflow definitions
//! - Workflow execution with pause/resume/cancel
//! - Workflow recording for learning user patterns
//! - Template management for reusable workflows

use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use gateway_core::workflow::{
    ExecutionStatus, WorkflowDefinition, WorkflowExecution, WorkflowRecorder, WorkflowStep,
    WorkflowTemplate,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{error::ApiError, state::AppState};

/// Shared workflow recorder instance
type SharedRecorder = Arc<RwLock<WorkflowRecorder>>;

/// Create the workflows routes
pub fn routes() -> Router<AppState> {
    // Create a shared recorder instance
    let recorder = Arc::new(RwLock::new(WorkflowRecorder::new()));

    Router::new()
        // Workflow CRUD
        .route("/", get(list_workflows))
        .route("/", post(create_workflow))
        .route("/{id}", get(get_workflow))
        .route("/{id}", delete(delete_workflow))
        .route("/{id}/execute", post(execute_workflow))
        // Execution management
        .route("/executions", get(list_executions))
        .route("/executions/{exec_id}", get(get_execution))
        .route("/executions/{exec_id}/pause", post(pause_execution))
        .route("/executions/{exec_id}/resume", post(resume_execution))
        .route("/executions/{exec_id}/cancel", post(cancel_execution))
        // Recording endpoints
        .route("/record/start", post(start_recording))
        .route("/record/stop", post(stop_recording))
        .route("/record/action", post(record_action))
        .route("/record/status", get(get_recording_status))
        // Template endpoints
        .route("/templates", get(list_templates))
        .route("/templates/{id}", get(get_template))
        .route("/templates/{id}/execute", post(execute_template))
        // Add recorder to state via layer
        .layer(axum::Extension(recorder))
}

/// Workflow representation for API
#[derive(Debug, Serialize)]
pub struct ApiWorkflow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub step_count: usize,
}

impl From<&WorkflowDefinition> for ApiWorkflow {
    fn from(w: &WorkflowDefinition) -> Self {
        ApiWorkflow {
            id: w.id.clone(),
            name: w.name.clone(),
            description: w.description.clone(),
            step_count: w.steps.len(),
        }
    }
}

/// Workflows list response
#[derive(Debug, Serialize)]
pub struct WorkflowsResponse {
    pub workflows: Vec<ApiWorkflow>,
    pub count: usize,
}

/// List all workflows
pub async fn list_workflows(State(state): State<AppState>) -> Json<WorkflowsResponse> {
    let engine = state.workflow_engine.read().await;
    let workflows: Vec<ApiWorkflow> = engine.list().iter().map(|w| (*w).into()).collect();
    let count = workflows.len();

    Json(WorkflowsResponse { workflows, count })
}

/// Create workflow request
#[derive(Debug, Deserialize)]
pub struct CreateWorkflowRequest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub steps: Vec<serde_json::Value>,
}

/// Get a specific workflow
pub async fn get_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiWorkflow>, ApiError> {
    let engine = state.workflow_engine.read().await;

    engine
        .get(&id)
        .map(|w| Json(w.into()))
        .ok_or_else(|| ApiError::not_found(format!("Workflow not found: {}", id)))
}

/// Create a new workflow
pub async fn create_workflow(
    State(state): State<AppState>,
    Json(request): Json<CreateWorkflowRequest>,
) -> Result<Json<ApiWorkflow>, ApiError> {
    tracing::info!(
        workflow_id = %request.id,
        workflow_name = %request.name,
        "Creating workflow"
    );

    let steps: Vec<WorkflowStep> = request
        .steps
        .into_iter()
        .enumerate()
        .map(|(i, val)| {
            serde_json::from_value::<WorkflowStep>(val)
                .map_err(|e| ApiError::bad_request(format!("Invalid step at index {}: {}", i, e)))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let workflow = WorkflowDefinition {
        id: request.id,
        name: request.name,
        description: request.description,
        steps,
    };

    let api_workflow = ApiWorkflow::from(&workflow);

    let mut engine = state.workflow_engine.write().await;
    engine.register(workflow);

    Ok(Json(api_workflow))
}

/// Execute workflow request
#[derive(Debug, Deserialize)]
pub struct ExecuteWorkflowRequest {
    #[serde(default)]
    pub input: serde_json::Value,
}

/// Execution response
#[derive(Debug, Serialize)]
pub struct ExecutionResponse {
    pub id: String,
    pub workflow_id: String,
    pub status: String,
    pub current_step: usize,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl From<WorkflowExecution> for ExecutionResponse {
    fn from(e: WorkflowExecution) -> Self {
        ExecutionResponse {
            id: e.id,
            workflow_id: e.workflow_id,
            status: match e.status {
                ExecutionStatus::Pending => "pending",
                ExecutionStatus::Running => "running",
                ExecutionStatus::Success => "success",
                ExecutionStatus::Error => "error",
                ExecutionStatus::Cancelled => "cancelled",
                ExecutionStatus::Paused => "paused",
            }
            .to_string(),
            current_step: e.current_step,
            output: e.output,
            error: e.error,
        }
    }
}

/// Execute a workflow
pub async fn execute_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ExecuteWorkflowRequest>,
) -> Result<Json<ExecutionResponse>, ApiError> {
    tracing::info!(
        workflow_id = %id,
        "Executing workflow"
    );

    let engine = state.workflow_engine.read().await;
    let execution = engine.execute(&id, request.input).await?;

    tracing::info!(
        workflow_id = %id,
        execution_id = %execution.id,
        status = ?execution.status,
        "Workflow execution completed"
    );

    Ok(Json(execution.into()))
}

/// Delete a workflow
pub async fn delete_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut engine = state.workflow_engine.write().await;

    if engine.unregister(&id) {
        tracing::info!(workflow_id = %id, "Workflow deleted");
        Ok(Json(serde_json::json!({ "deleted": true, "id": id })))
    } else {
        Err(ApiError::not_found(format!("Workflow not found: {}", id)))
    }
}

/// Executions list response
#[derive(Debug, Serialize)]
pub struct ExecutionsResponse {
    pub executions: Vec<ExecutionResponse>,
    pub count: usize,
}

/// List all executions
pub async fn list_executions(State(state): State<AppState>) -> Json<ExecutionsResponse> {
    let executor = state.workflow_executor.read().await;
    let executions: Vec<ExecutionResponse> = executor
        .list_executions()
        .await
        .into_iter()
        .map(|e| e.into())
        .collect();

    let count = executions.len();
    Json(ExecutionsResponse { executions, count })
}

/// Get a specific execution
pub async fn get_execution(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
) -> Result<Json<ExecutionResponse>, ApiError> {
    let executor = state.workflow_executor.read().await;

    executor
        .get_execution(&exec_id)
        .await
        .map(|e| Json(e.into()))
        .ok_or_else(|| ApiError::not_found(format!("Execution not found: {}", exec_id)))
}

/// Pause an execution
pub async fn pause_execution(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let executor = state.workflow_executor.read().await;

    executor
        .pause(&exec_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    tracing::info!(execution_id = %exec_id, "Execution paused");
    Ok(Json(serde_json::json!({ "paused": true, "id": exec_id })))
}

/// Resume an execution
pub async fn resume_execution(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let executor = state.workflow_executor.read().await;

    executor
        .resume(&exec_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    tracing::info!(execution_id = %exec_id, "Execution resumed");
    Ok(Json(serde_json::json!({ "resumed": true, "id": exec_id })))
}

/// Cancel an execution
pub async fn cancel_execution(
    State(state): State<AppState>,
    Path(exec_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let executor = state.workflow_executor.read().await;

    executor
        .cancel(&exec_id)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;

    tracing::info!(execution_id = %exec_id, "Execution cancelled");
    Ok(Json(
        serde_json::json!({ "cancelled": true, "id": exec_id }),
    ))
}

// ============================================================================
// Recording Endpoints
// ============================================================================

/// Start recording request
#[derive(Debug, Deserialize)]
pub struct StartRecordingRequest {
    /// Name for the recording session
    pub name: String,
    /// Optional description
    #[serde(default)]
    pub description: Option<String>,
}

/// Recording session response
#[derive(Debug, Serialize)]
pub struct RecordingSessionResponse {
    pub session_id: String,
    pub name: String,
    pub status: String,
    pub action_count: usize,
    pub started_at: String,
}

/// Start a new recording session
pub async fn start_recording(
    axum::Extension(recorder): axum::Extension<SharedRecorder>,
    Json(request): Json<StartRecordingRequest>,
) -> Result<Json<RecordingSessionResponse>, ApiError> {
    let recorder = recorder.write().await;

    let session_id = recorder
        .start_recording(request.name.clone(), request.description)
        .await;

    tracing::info!(
        session_id = %session_id,
        name = %request.name,
        "Started workflow recording"
    );

    Ok(Json(RecordingSessionResponse {
        session_id,
        name: request.name,
        status: "recording".to_string(),
        action_count: 0,
        started_at: chrono::Utc::now().to_rfc3339(),
    }))
}

/// Stop recording and create template
pub async fn stop_recording(
    axum::Extension(recorder): axum::Extension<SharedRecorder>,
) -> Result<Json<TemplateResponse>, ApiError> {
    let recorder = recorder.write().await;

    let session = recorder
        .stop_recording()
        .await
        .ok_or_else(|| ApiError::bad_request("No active recording session"))?;

    // Analyze and create template from the session
    let template = recorder.analyze_and_create_template(&session).await;

    tracing::info!(
        template_id = %template.id,
        template_name = %template.name,
        step_count = template.steps.len(),
        "Created workflow template from recording"
    );

    Ok(Json(TemplateResponse::from(&template)))
}

/// Record action request
#[derive(Debug, Deserialize)]
pub struct RecordActionRequest {
    /// Tool name
    pub tool_name: String,
    /// Tool parameters
    pub params: serde_json::Value,
    /// Tool result
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    /// Whether the action succeeded
    pub success: bool,
    /// Optional duration in milliseconds
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// Record an action during the current session
pub async fn record_action(
    axum::Extension(recorder): axum::Extension<SharedRecorder>,
    Json(request): Json<RecordActionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let recorder = recorder.read().await;

    let action_id = recorder
        .record_action(
            request.tool_name.clone(),
            request.params,
            request.result,
            request.success,
            request.duration_ms,
        )
        .await;

    match action_id {
        Some(id) => {
            tracing::debug!(tool = %request.tool_name, action_id = %id, "Recorded action");
            Ok(Json(serde_json::json!({
                "recorded": true,
                "tool": request.tool_name,
                "action_id": id
            })))
        }
        None => Err(ApiError::bad_request("No active recording session")),
    }
}

/// Get current recording status
pub async fn get_recording_status(
    axum::Extension(recorder): axum::Extension<SharedRecorder>,
) -> Json<serde_json::Value> {
    let recorder = recorder.read().await;

    match recorder.get_session().await {
        Some(session) => Json(serde_json::json!({
            "recording": true,
            "session_id": session.id,
            "name": session.name,
            "action_count": session.actions.len(),
            "status": format!("{:?}", session.status),
            "started_at": session.started_at.to_rfc3339(),
        })),
        None => Json(serde_json::json!({
            "recording": false
        })),
    }
}

// ============================================================================
// Template Endpoints
// ============================================================================

/// Template response
#[derive(Debug, Serialize)]
pub struct TemplateResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub step_count: usize,
    pub parameter_count: usize,
    pub success_rate: f32,
    pub execution_count: u32,
    pub created_at: String,
}

impl From<&WorkflowTemplate> for TemplateResponse {
    fn from(t: &WorkflowTemplate) -> Self {
        TemplateResponse {
            id: t.id.clone(),
            name: t.name.clone(),
            description: t.description.clone(),
            step_count: t.steps.len(),
            parameter_count: t.parameters.len(),
            success_rate: t.success_rate,
            execution_count: t.execution_count as u32,
            created_at: t.created_at.to_rfc3339(),
        }
    }
}

/// Templates list response
#[derive(Debug, Serialize)]
pub struct TemplatesResponse {
    pub templates: Vec<TemplateResponse>,
    pub count: usize,
}

/// List all workflow templates
pub async fn list_templates(
    axum::Extension(recorder): axum::Extension<SharedRecorder>,
) -> Json<TemplatesResponse> {
    let recorder = recorder.read().await;
    let templates: Vec<TemplateResponse> = recorder
        .list_templates()
        .await
        .iter()
        .map(|t| TemplateResponse::from(t))
        .collect();

    let count = templates.len();
    Json(TemplatesResponse { templates, count })
}

/// Get a specific template
pub async fn get_template(
    axum::Extension(recorder): axum::Extension<SharedRecorder>,
    Path(id): Path<String>,
) -> Result<Json<TemplateResponse>, ApiError> {
    let recorder = recorder.read().await;

    recorder
        .get_template(&id)
        .await
        .map(|t| Json(TemplateResponse::from(&t)))
        .ok_or_else(|| ApiError::not_found(format!("Template not found: {}", id)))
}

/// Execute template request
#[derive(Debug, Deserialize)]
pub struct ExecuteTemplateRequest {
    /// Parameter values to substitute
    #[serde(default)]
    pub parameters: std::collections::HashMap<String, serde_json::Value>,
}

/// Execute a workflow template
pub async fn execute_template(
    State(state): State<AppState>,
    axum::Extension(recorder): axum::Extension<SharedRecorder>,
    Path(id): Path<String>,
    Json(request): Json<ExecuteTemplateRequest>,
) -> Result<Json<ExecutionResponse>, ApiError> {
    let recorder_guard = recorder.read().await;

    let template = recorder_guard
        .get_template(&id)
        .await
        .ok_or_else(|| ApiError::not_found(format!("Template not found: {}", id)))?;

    // Convert template to workflow definition
    let steps: Vec<WorkflowStep> = template
        .steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            // Substitute parameters in the step
            let mut params = step.params.clone();
            for (key, value) in &request.parameters {
                let placeholder = format!("{{{{{}}}}}", key);
                if let Some(s) = params.as_str() {
                    if s.contains(&placeholder) {
                        params = serde_json::json!(s.replace(&placeholder, &value.to_string()));
                    }
                }
            }

            WorkflowStep {
                id: format!("step_{}", i),
                name: step.tool.clone(),
                step_type: gateway_core::workflow::StepType::ToolCall,
                config: serde_json::json!({
                    "tool_name": step.tool,
                    "parameters": params,
                }),
                depends_on: vec![],
            }
        })
        .collect();

    let workflow = WorkflowDefinition {
        id: format!("template_exec_{}", uuid::Uuid::new_v4()),
        name: format!("Execution of {}", template.name),
        description: template.description.clone(),
        steps,
    };

    // Execute the workflow
    let engine = state.workflow_engine.read().await;
    let execution = engine
        .execute(&workflow.id, serde_json::Value::Null)
        .await?;

    // Update template statistics
    drop(recorder_guard);
    let recorder_write = recorder.write().await;
    let success = execution.status == ExecutionStatus::Success;
    recorder_write
        .update_template_stats(&id, success, 0.0)
        .await;

    tracing::info!(
        template_id = %id,
        execution_id = %execution.id,
        status = ?execution.status,
        "Executed workflow template"
    );

    Ok(Json(execution.into()))
}
