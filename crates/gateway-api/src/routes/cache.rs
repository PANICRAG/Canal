//! Cache management endpoints.
//!
//! This module provides REST API endpoints for interacting with the multi-level
//! cache system:
//!
//! - **L2 Semantic Cache**: Embedding-based response caching with similarity lookup
//! - **L3 Plan Cache**: LRU-based plan caching for repeated task patterns
//!
//! # Feature Gate
//!
//! This module requires the `cache` feature flag. The feature gate is applied
//! at the `mod.rs` level when nesting these routes.

use axum::{
    extract::{Json, State},
    routing::{get, post},
    Router,
};
use gateway_core::cache::{CachedPlan, PlanCache};
use serde::{Deserialize, Serialize};

use crate::{error::ApiError, state::AppState};

// Input validation limits
const MAX_TASK_LEN: usize = 10_000;
const MAX_PLAN_LEN: usize = 100_000;
const MAX_PATTERN_LEN: usize = 1_000;

/// Create the cache routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/stats", get(get_cache_stats))
        .route("/plan/get", post(lookup_plan))
        .route("/plan/put", post(store_plan))
        .route("/invalidate", post(invalidate_semantic_cache))
}

// ============================================================================
// Request / Response Types
// ============================================================================

/// Cache statistics response combining L2 (semantic) and L3 (plan) metrics.
#[derive(Debug, Serialize)]
pub struct CacheStatsResponse {
    /// Plan cache (L3) entry count
    pub plan_cache_entries: usize,
    /// Semantic cache similarity threshold
    pub semantic_similarity_threshold: f32,
}

/// Request body for plan lookup
#[derive(Debug, Deserialize)]
pub struct PlanLookupRequest {
    /// The task description to look up
    pub task: String,
}

/// Response for a plan lookup
#[derive(Debug, Serialize)]
pub struct PlanLookupResponse {
    /// Whether a cached plan was found
    pub found: bool,
    /// The cached plan text, if found
    pub plan: Option<String>,
    /// The success rate of the plan, if found
    pub success_rate: Option<f32>,
    /// The normalized key used for lookup
    pub normalized_key: String,
}

/// Request body for storing a plan
#[derive(Debug, Deserialize)]
pub struct PlanStoreRequest {
    /// The task description to store the plan under
    pub task: String,
    /// The plan text to cache
    pub plan: String,
}

/// Response after storing a plan
#[derive(Debug, Serialize)]
pub struct PlanStoreResponse {
    /// Whether the plan was stored successfully
    pub stored: bool,
    /// The normalized key under which the plan was stored
    pub normalized_key: String,
}

/// Request body for invalidating semantic cache entries
#[derive(Debug, Deserialize)]
pub struct InvalidateRequest {
    /// Pattern to match against cached entries for invalidation
    pub pattern: String,
}

/// Response after invalidating semantic cache entries
#[derive(Debug, Serialize)]
pub struct InvalidateResponse {
    /// Number of entries invalidated
    pub invalidated_count: usize,
    /// The pattern that was used
    pub pattern: String,
}

// ============================================================================
// Handler Functions
// ============================================================================

/// Get cache status information.
///
/// Returns entry counts and configuration for both cache layers.
pub async fn get_cache_stats(
    State(state): State<AppState>,
) -> Result<Json<CacheStatsResponse>, ApiError> {
    Ok(Json(CacheStatsResponse {
        plan_cache_entries: state.plan_cache.len(),
        semantic_similarity_threshold: state.semantic_cache.similarity_threshold(),
    }))
}

/// Look up a cached plan by task description.
///
/// The task key is normalized before lookup, so minor wording differences
/// in equivalent tasks will still produce cache hits.
pub async fn lookup_plan(
    State(state): State<AppState>,
    Json(request): Json<PlanLookupRequest>,
) -> Result<Json<PlanLookupResponse>, ApiError> {
    if request.task.len() > MAX_TASK_LEN {
        return Err(ApiError::bad_request("Task description too long"));
    }
    let normalized_key = PlanCache::normalize_key(&request.task);
    let cached = state.plan_cache.get(&normalized_key).await;

    Ok(Json(PlanLookupResponse {
        found: cached.is_some(),
        success_rate: cached.as_ref().map(|c| c.success_rate()),
        plan: cached.map(|c| c.plan),
        normalized_key,
    }))
}

/// Store a plan in the L3 plan cache.
///
/// The task key is normalized before storage. Subsequent lookups with
/// equivalent task descriptions will return this plan.
pub async fn store_plan(
    State(state): State<AppState>,
    Json(request): Json<PlanStoreRequest>,
) -> Result<Json<PlanStoreResponse>, ApiError> {
    if request.task.len() > MAX_TASK_LEN {
        return Err(ApiError::bad_request("Task description too long"));
    }
    if request.plan.len() > MAX_PLAN_LEN {
        return Err(ApiError::bad_request("Plan content too long"));
    }
    let normalized_key = PlanCache::normalize_key(&request.task);
    let plan = CachedPlan::new(normalized_key.clone(), request.plan);
    state.plan_cache.put(plan).await;

    Ok(Json(PlanStoreResponse {
        stored: true,
        normalized_key,
    }))
}

/// Invalidate semantic cache entries matching a pattern.
///
/// Removes all L2 semantic cache entries whose query text matches
/// the given pattern, forcing fresh LLM responses on the next request.
pub async fn invalidate_semantic_cache(
    State(state): State<AppState>,
    Json(request): Json<InvalidateRequest>,
) -> Result<Json<InvalidateResponse>, ApiError> {
    if request.pattern.len() > MAX_PATTERN_LEN {
        return Err(ApiError::bad_request("Invalidation pattern too long"));
    }
    tracing::info!(pattern = %request.pattern, "Invalidating semantic cache entries");

    let count = state
        .semantic_cache
        .invalidate(&request.pattern)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Cache invalidation failed");
            ApiError::internal("Cache invalidation failed")
        })?;

    Ok(Json(InvalidateResponse {
        invalidated_count: count,
        pattern: request.pattern,
    }))
}
