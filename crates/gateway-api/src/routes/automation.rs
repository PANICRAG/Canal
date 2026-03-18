//! Automation API routes
//!
//! Provides endpoints for the five-layer browser automation architecture.
//! These endpoints allow analyzing tasks and executing automation pipelines
//! with massive token savings compared to pure CV approaches.
//!
//! ## Endpoints
//!
//! - `POST /automation/analyze` - Analyze a task and get routing recommendation
//! - `POST /automation/execute` - Execute through the automation pipeline
//! - `GET /automation/status` - Get orchestrator status and metrics
//! - `GET /automation/scripts` - List cached scripts
//! - `DELETE /automation/scripts/:id` - Delete a cached script

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use gateway_core::agent::automation::{
    AutomationPath, AutomationRequest, MetricsSnapshot, OrchestratorStatus, RouteAnalysis,
};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

// ============================================================================
// Request/Response Types
// ============================================================================

/// Request to analyze a task for automation suitability
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct AnalyzeRequest {
    /// The task description
    pub task: String,
    /// Optional data count for better routing decisions
    pub data_count: Option<usize>,
    /// Target URL (optional, helps with routing)
    pub target_url: Option<String>,
}

/// Response from task analysis
#[derive(Debug, Serialize)]
pub struct AnalyzeResponse {
    /// Whether automation is recommended
    pub recommended: bool,
    /// The routing analysis with path and token estimates
    pub analysis: RouteAnalysis,
    /// Human-readable explanation
    pub explanation: String,
}

/// Request to execute an automation task
#[derive(Debug, Deserialize)]
pub struct ExecuteRequest {
    /// Task description
    pub task: String,
    /// Target URL
    pub target_url: String,
    /// Data to process (array of objects)
    pub data: Vec<serde_json::Value>,
    /// Optional timeout in milliseconds
    pub timeout_ms: Option<u64>,
    /// Optional force re-exploration (ignore cached scripts)
    pub force_explore: Option<bool>,
}

/// Response from automation execution
#[derive(Debug, Serialize)]
pub struct ExecuteResponse {
    /// Whether the operation succeeded
    pub success: bool,
    /// Request ID for tracking
    pub request_id: String,
    /// Path that was taken
    pub path: AutomationPath,
    /// Script ID if a new script was generated
    pub script_id: Option<String>,
    /// Execution statistics
    pub stats: ExecutionStatsResponse,
    /// Error message if failed
    pub error: Option<String>,
}

/// Execution statistics response
#[derive(Debug, Serialize)]
pub struct ExecutionStatsResponse {
    /// Time taken in milliseconds
    pub duration_ms: u64,
    /// Items processed
    pub items_processed: usize,
    /// Items succeeded
    pub items_succeeded: usize,
    /// Items failed
    pub items_failed: usize,
    /// Tokens used for exploration
    pub exploration_tokens: u64,
    /// Tokens used for code generation
    pub generation_tokens: u64,
    /// Total tokens used
    pub total_tokens: u64,
    /// Estimated tokens if using pure CV
    pub pure_cv_estimated_tokens: u64,
    /// Token savings percentage
    pub token_savings_percent: f64,
    /// Whether a cached script was reused
    pub script_reused: bool,
}

/// Status response
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    /// Orchestrator status
    pub status: OrchestratorStatus,
    /// Whether the service is available
    pub available: bool,
}

/// List scripts response
#[derive(Debug, Serialize)]
pub struct ListScriptsResponse {
    /// List of cached scripts
    pub scripts: Vec<ScriptSummary>,
    /// Total count
    pub total: usize,
}

/// Script summary
#[derive(Debug, Serialize)]
pub struct ScriptSummary {
    pub id: String,
    pub task_signature: String,
    pub url_pattern: Option<String>,
    pub script_type: String,
    pub use_count: u64,
    pub success_rate: f64,
    pub created_at: String,
    pub last_used_at: String,
}

// ============================================================================
// Route Handlers
// ============================================================================

/// Analyze a task for automation suitability
///
/// POST /automation/analyze
///
/// Returns routing analysis with recommended path and token estimates.
async fn analyze_task(
    State(state): State<AppState>,
    Json(request): Json<AnalyzeRequest>,
) -> Result<Json<AnalyzeResponse>, ApiError> {
    let orchestrator = state
        .automation_orchestrator()
        .ok_or_else(|| ApiError::service_unavailable("Automation orchestrator not configured"))?;

    let analysis = orchestrator
        .analyze(&request.task, request.data_count)
        .await
        .map_err(|e| ApiError::internal(format!("Analysis failed: {}", e)))?;

    let recommended = matches!(
        &analysis.decision.path,
        AutomationPath::ExploreAndGenerate { .. }
            | AutomationPath::ReuseScript { .. }
            | AutomationPath::DirectApi { .. }
    );

    let explanation = match &analysis.decision.path {
        AutomationPath::ExploreAndGenerate {
            estimated_tokens, ..
        } => {
            format!(
                "Recommended: Explore & Generate. Estimated {} tokens (vs ~{} for pure CV). \
                Will explore page structure once, generate reusable script.",
                estimated_tokens,
                request.data_count.unwrap_or(1) as u64 * 10000
            )
        }
        AutomationPath::ReuseScript {
            script_id,
            last_success_rate,
        } => {
            format!(
                "Recommended: Reuse cached script '{}' (success rate: {:.1}%). \
                Near-zero token cost.",
                script_id,
                last_success_rate * 100.0
            )
        }
        AutomationPath::DirectApi { api_type, .. } => {
            format!(
                "Recommended: Direct API call via {}. Minimal token cost.",
                api_type
            )
        }
        AutomationPath::PureComputerVision {
            max_items,
            estimated_tokens,
        } => {
            format!(
                "Pure CV recommended for {} items. Estimated {} tokens. \
                Consider reducing data size for better efficiency.",
                max_items, estimated_tokens
            )
        }
        AutomationPath::HybridApproach {
            explore_phase_tokens,
            execute_phase_tokens,
        } => {
            format!(
                "Hybrid approach: {} tokens for exploration, {} for execution.",
                explore_phase_tokens, execute_phase_tokens
            )
        }
        AutomationPath::RequiresHumanAssistance { reason } => {
            format!("Human assistance required: {}", reason)
        }
    };

    Ok(Json(AnalyzeResponse {
        recommended,
        analysis,
        explanation,
    }))
}

/// Execute an automation task
///
/// POST /automation/execute
///
/// Executes the task through the five-layer pipeline.
async fn execute_task(
    State(state): State<AppState>,
    Json(request): Json<ExecuteRequest>,
) -> Result<Json<ExecuteResponse>, ApiError> {
    let orchestrator = state
        .automation_orchestrator()
        .ok_or_else(|| ApiError::service_unavailable("Automation orchestrator not configured"))?;

    let mut automation_request = AutomationRequest::new(&request.task)
        .with_url(&request.target_url)
        .with_data(request.data);

    // Set timeout if provided
    if let Some(timeout) = request.timeout_ms {
        automation_request.timeout_ms = timeout;
    }

    // Set force_explore option if true
    if request.force_explore.unwrap_or(false) {
        automation_request
            .options
            .insert("force_explore".to_string(), serde_json::json!(true));
    }

    let result = orchestrator
        .execute(automation_request)
        .await
        .map_err(|e| ApiError::internal(format!("Execution failed: {}", e)))?;

    // Calculate items_succeeded from items_processed - items_failed
    let items_succeeded = result
        .stats
        .items_processed
        .saturating_sub(result.stats.items_failed);

    let stats = ExecutionStatsResponse {
        duration_ms: result.stats.duration_ms,
        items_processed: result.stats.items_processed,
        items_succeeded,
        items_failed: result.stats.items_failed,
        exploration_tokens: result.stats.exploration_tokens,
        generation_tokens: result.stats.generation_tokens,
        total_tokens: result.stats.total_tokens,
        pure_cv_estimated_tokens: result.stats.pure_cv_estimated_tokens,
        token_savings_percent: result.stats.savings_percent,
        script_reused: result.stats.script_reused,
    };

    Ok(Json(ExecuteResponse {
        success: result.success,
        request_id: result.request_id,
        path: result.path_used,
        script_id: result.script_id,
        stats,
        error: result.error,
    }))
}

/// Get orchestrator status
///
/// GET /automation/status
async fn get_status(State(state): State<AppState>) -> Result<Json<StatusResponse>, ApiError> {
    match state.automation_orchestrator() {
        Some(orchestrator) => {
            let status = orchestrator.status().await;
            Ok(Json(StatusResponse {
                status,
                available: true,
            }))
        }
        None => Ok(Json(StatusResponse {
            status: OrchestratorStatus {
                ready: false,
                browser_connected: false,
                llm_available: false,
                cached_scripts: 0,
                metrics: MetricsSnapshot {
                    total_requests: 0,
                    successful_requests: 0,
                    failed_requests: 0,
                    scripts_generated: 0,
                    scripts_reused: 0,
                    exploration_tokens: 0,
                    generation_tokens: 0,
                    tokens_saved: 0,
                    items_processed: 0,
                },
            },
            available: false,
        })),
    }
}

/// List cached scripts
///
/// GET /automation/scripts
async fn list_scripts(
    State(state): State<AppState>,
) -> Result<Json<ListScriptsResponse>, ApiError> {
    let orchestrator = state
        .automation_orchestrator()
        .ok_or_else(|| ApiError::service_unavailable("Automation orchestrator not configured"))?;

    let scripts = orchestrator
        .list_scripts(Some(100))
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list scripts: {}", e)))?;

    let summaries: Vec<ScriptSummary> = scripts
        .iter()
        .map(|s| ScriptSummary {
            id: s.id.clone(),
            task_signature: s.task_signature.clone(),
            url_pattern: s.url_pattern.clone(),
            script_type: format!("{:?}", s.script_type),
            use_count: s.use_count,
            success_rate: s.success_rate,
            created_at: s.created_at.to_rfc3339(),
            last_used_at: s.last_used_at.to_rfc3339(),
        })
        .collect();

    let total = summaries.len();

    Ok(Json(ListScriptsResponse {
        scripts: summaries,
        total,
    }))
}

/// Delete a cached script
///
/// DELETE /automation/scripts/:id
async fn delete_script(
    State(state): State<AppState>,
    Path(script_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let orchestrator = state
        .automation_orchestrator()
        .ok_or_else(|| ApiError::service_unavailable("Automation orchestrator not configured"))?;

    orchestrator
        .delete_script(&script_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to delete script: {}", e)))?;

    Ok(StatusCode::NO_CONTENT)
}

// ============================================================================
// Router
// ============================================================================

/// Create the automation routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/analyze", post(analyze_task))
        .route("/execute", post(execute_task))
        .route("/status", get(get_status))
        .route("/scripts", get(list_scripts))
        .route("/scripts/{id}", delete(delete_script))
}
