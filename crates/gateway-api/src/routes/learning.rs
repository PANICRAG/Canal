//! Learning system endpoints.
//!
//! This module provides REST API endpoints for interacting with the closed-loop
//! learning engine:
//!
//! - **Experience recording**: Manually record task outcomes for learning
//! - **Learning cycles**: Trigger knowledge distillation from collected experiences
//! - **Knowledge queries**: Retrieve learned patterns relevant to a task
//! - **Engine control**: Enable or disable the learning system at runtime
//!
//! # Feature Gate
//!
//! This module requires the `learning` feature flag. The feature gate is applied
//! at the `mod.rs` level when nesting these routes.

use axum::{
    extract::{Json, Query, State},
    routing::{get, post},
    Router,
};
use gateway_core::learning::Experience;
use serde::{Deserialize, Serialize};

use crate::{error::ApiError, state::AppState};

// Input validation limits
const MAX_TASK_LEN: usize = 10_000;

/// Create the learning routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/status", get(get_status))
        .route("/record", post(record_experience))
        .route("/learn", post(trigger_learning))
        .route("/knowledge", get(query_knowledge))
        .route("/toggle", post(toggle_learning))
}

// ============================================================================
// Request / Response Types
// ============================================================================

/// Learning engine status response
#[derive(Debug, Serialize)]
pub struct LearningStatusResponse {
    /// Whether the learning engine is currently enabled
    pub enabled: bool,
    /// Number of experiences in the buffer awaiting processing
    pub buffer_size: usize,
    /// Total number of distilled knowledge entries
    pub knowledge_count: usize,
}

/// Request body for recording an experience
#[derive(Debug, Deserialize)]
pub struct RecordExperienceRequest {
    /// Description of the task that was performed
    pub task: String,
    /// Whether the task completed successfully
    pub success: bool,
}

/// Response after recording an experience
#[derive(Debug, Serialize)]
pub struct RecordExperienceResponse {
    /// Whether the experience was recorded
    pub recorded: bool,
    /// Updated buffer size after recording
    pub buffer_size: usize,
}

/// Response from a learning cycle
#[derive(Debug, Serialize)]
pub struct LearningCycleResponse {
    /// Whether the learning cycle completed
    pub completed: bool,
    /// Number of experiences processed in this cycle
    pub experiences_processed: usize,
    /// Number of patterns mined in this cycle
    pub patterns_mined: usize,
    /// Number of patterns stored as knowledge
    pub patterns_stored: usize,
    /// Total knowledge entries after this cycle
    pub total_knowledge: usize,
}

/// Query parameters for knowledge lookup
#[derive(Debug, Deserialize)]
pub struct KnowledgeQueryParams {
    /// Task description to find relevant knowledge for
    pub task: String,
}

/// A single knowledge entry returned from a query
#[derive(Debug, Serialize)]
pub struct KnowledgeEntryResponse {
    /// Category of the knowledge entry
    pub category: String,
    /// The learned insight or recommendation
    pub content: String,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,
}

/// Response for a knowledge query
#[derive(Debug, Serialize)]
pub struct KnowledgeQueryResponse {
    /// The task that was queried
    pub task: String,
    /// Matching knowledge entries
    pub entries: Vec<KnowledgeEntryResponse>,
    /// Total number of matching entries
    pub count: usize,
}

/// Request body for toggling the learning engine
#[derive(Debug, Deserialize)]
pub struct ToggleLearningRequest {
    /// Whether to enable or disable learning
    pub enabled: bool,
}

/// Response after toggling the learning engine
#[derive(Debug, Serialize)]
pub struct ToggleLearningResponse {
    /// The new enabled state
    pub enabled: bool,
    /// Previous enabled state before the toggle
    pub was_enabled: bool,
}

// ============================================================================
// Handler Functions
// ============================================================================

/// Get the current status of the learning engine.
pub async fn get_status(
    State(state): State<AppState>,
) -> Result<Json<LearningStatusResponse>, ApiError> {
    let engine = &state.learning_engine;

    Ok(Json(LearningStatusResponse {
        enabled: engine.is_enabled(),
        buffer_size: engine.buffer_size().await,
        knowledge_count: engine.knowledge_count(),
    }))
}

/// Record a task experience manually.
pub async fn record_experience(
    State(state): State<AppState>,
    Json(request): Json<RecordExperienceRequest>,
) -> Result<Json<RecordExperienceResponse>, ApiError> {
    if request.task.len() > MAX_TASK_LEN {
        return Err(ApiError::bad_request("Task description too long"));
    }
    tracing::info!(task = %request.task, success = request.success, "Recording experience");

    let experience = if request.success {
        Experience::test_success(&request.task)
    } else {
        Experience::test_failure(&request.task, "manual recording")
    };

    state
        .learning_engine
        .record(experience)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to record experience");
            ApiError::internal("Failed to record experience")
        })?;

    let buffer_size = state.learning_engine.buffer_size().await;

    Ok(Json(RecordExperienceResponse {
        recorded: true,
        buffer_size,
    }))
}

/// Trigger a learning cycle.
pub async fn trigger_learning(
    State(state): State<AppState>,
) -> Result<Json<LearningCycleResponse>, ApiError> {
    tracing::info!("Triggering learning cycle");

    let report = state.learning_engine.learn().await.map_err(|e| {
        tracing::error!(error = %e, "Learning cycle failed");
        ApiError::internal("Learning cycle failed")
    })?;

    let total_knowledge = state.learning_engine.knowledge_count();

    Ok(Json(LearningCycleResponse {
        completed: true,
        experiences_processed: report.experiences_processed,
        patterns_mined: report.patterns_mined,
        patterns_stored: report.patterns_stored,
        total_knowledge,
    }))
}

/// Query knowledge relevant to a task.
pub async fn query_knowledge(
    State(state): State<AppState>,
    Query(params): Query<KnowledgeQueryParams>,
) -> Result<Json<KnowledgeQueryResponse>, ApiError> {
    if params.task.len() > MAX_TASK_LEN {
        return Err(ApiError::bad_request("Task description too long"));
    }
    let entries = state.learning_engine.query_knowledge(&params.task);

    let response_entries: Vec<KnowledgeEntryResponse> = entries
        .into_iter()
        .map(|entry| KnowledgeEntryResponse {
            category: format!("{:?}", entry.category),
            content: entry.content,
            confidence: entry.confidence,
        })
        .collect();

    let count = response_entries.len();

    Ok(Json(KnowledgeQueryResponse {
        task: params.task,
        entries: response_entries,
        count,
    }))
}

/// Enable or disable the learning engine at runtime.
pub async fn toggle_learning(
    State(state): State<AppState>,
    Json(request): Json<ToggleLearningRequest>,
) -> Result<Json<ToggleLearningResponse>, ApiError> {
    tracing::info!(enabled = request.enabled, "Toggling learning engine");

    let was_enabled = state.learning_engine.is_enabled();
    state.learning_engine.set_enabled(request.enabled);

    Ok(Json(ToggleLearningResponse {
        enabled: request.enabled,
        was_enabled,
    }))
}
