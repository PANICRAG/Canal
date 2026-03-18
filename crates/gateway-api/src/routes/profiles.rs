//! Model routing profile management endpoints
//!
//! Provides API endpoints for managing LLM routing profiles, including:
//! - Profile CRUD operations
//! - Profile templates
//! - Health status monitoring
//! - Cost tracking

use axum::{
    extract::{Path, State},
    routing::{delete, get, post, put},
    Json, Router,
};
use gateway_core::llm::model_profile::{AgentConfig, ModelProfile, RoutingConfig, RoutingStrategy};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::{error::ApiError, state::AppState};

/// Create the profile routes
pub fn routes() -> Router<AppState> {
    Router::new()
        // Profile CRUD
        .route("/", get(list_profiles))
        .route("/", post(create_profile))
        .route("/{id}", get(get_profile))
        .route("/{id}", put(update_profile))
        .route("/{id}", delete(delete_profile))
        // Profile templates
        .route("/templates", get(list_templates))
        .route("/templates/{id}/instantiate", post(instantiate_template))
        // Health and status
        .route("/health", get(get_health_status))
        .route("/health/{provider}", get(get_provider_health))
        // Cost tracking
        .route("/costs", get(get_cost_summary))
        .route("/costs/{profile_id}", get(get_profile_costs))
        // Testing
        .route("/{id}/test", post(test_profile))
}

/// Profile summary for list response
#[derive(Debug, Serialize)]
pub struct ProfileSummary {
    pub id: String,
    pub name: String,
    pub strategy: String,
    pub enabled: bool,
    pub description: Option<String>,
}

/// List all profiles
pub async fn list_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<ProfileSummary>>, ApiError> {
    // Access profile catalog directly from state
    let catalog = state.profile_catalog.read().await;
    let profiles = catalog.list().await;

    let summaries: Vec<ProfileSummary> = profiles
        .iter()
        .map(|profile| ProfileSummary {
            id: profile.id.clone(),
            name: profile.name.clone(),
            strategy: format!("{:?}", profile.routing.strategy),
            enabled: true, // TODO: Add enabled field to ModelProfile
            description: if profile.description.is_empty() {
                None
            } else {
                Some(profile.description.clone())
            },
        })
        .collect();
    Ok(Json(summaries))
}

/// Full profile details
#[derive(Debug, Serialize, Deserialize)]
pub struct ProfileDetails {
    pub id: String,
    pub name: String,
    pub description: String,
    pub strategy: String,
    pub routing: serde_json::Value,
}

/// Get a specific profile
pub async fn get_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ProfileDetails>, ApiError> {
    let catalog = state.profile_catalog.read().await;

    match catalog.get(&id).await {
        Ok(profile) => Ok(Json(ProfileDetails {
            id: profile.id.clone(),
            name: profile.name.clone(),
            description: profile.description.clone(),
            strategy: format!("{:?}", profile.routing.strategy),
            routing: serde_json::to_value(&profile.routing).unwrap_or_default(),
        })),
        Err(_) => Err(ApiError::not_found(format!("Profile '{}' not found", id))),
    }
}

/// Create profile request
#[derive(Debug, Deserialize)]
pub struct CreateProfileRequest {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub strategy: String,
    pub routing: serde_json::Value,
}

/// Create a new profile
pub async fn create_profile(
    State(state): State<AppState>,
    Json(request): Json<CreateProfileRequest>,
) -> Result<Json<ProfileDetails>, ApiError> {
    tracing::info!(
        profile_id = %request.id,
        name = %request.name,
        strategy = %request.strategy,
        "Creating new model profile"
    );

    // Parse routing strategy
    let strategy = parse_routing_strategy(&request.strategy)?;

    // Parse routing config from JSON
    let routing: RoutingConfig = serde_json::from_value(request.routing.clone())
        .map_err(|e| ApiError::bad_request(format!("Invalid routing config: {}", e)))?;

    // Create the profile
    let profile = ModelProfile {
        id: request.id.clone(),
        name: request.name.clone(),
        description: request.description.clone().unwrap_or_default(),
        enabled: true,
        routing,
        agent: AgentConfig::default(),
        cache_enabled: false,
        cache_ttl_seconds: 3600,
    };

    // Insert into catalog
    {
        let catalog = state.profile_catalog.read().await;
        let replaced = catalog.upsert(profile.clone()).await;
        if replaced {
            tracing::info!(profile_id = %request.id, "Replaced existing profile");
        }

        // Persist to YAML if config file is available
        if let Err(e) = catalog.save_to_yaml().await {
            tracing::warn!(error = %e, "Failed to persist profile to YAML (changes are in-memory only)");
        }
    }

    Ok(Json(ProfileDetails {
        id: profile.id,
        name: profile.name,
        description: profile.description,
        strategy: format!("{:?}", strategy),
        routing: request.routing,
    }))
}

/// Parse routing strategy from string
fn parse_routing_strategy(s: &str) -> Result<RoutingStrategy, ApiError> {
    match s.to_lowercase().as_str() {
        "primary_fallback" | "primaryfallback" => Ok(RoutingStrategy::PrimaryFallback),
        "task_type_rules" | "tasktyperules" => Ok(RoutingStrategy::TaskTypeRules),
        "router_agent" | "routeragent" => Ok(RoutingStrategy::RouterAgent),
        "ab_test" | "abtest" => Ok(RoutingStrategy::AbTest),
        "cascade" => Ok(RoutingStrategy::Cascade),
        _ => Err(ApiError::bad_request(format!(
            "Invalid routing strategy '{}'. Valid values: primary_fallback, task_type_rules, router_agent, ab_test, cascade",
            s
        ))),
    }
}

/// Update profile request
#[derive(Debug, Deserialize)]
pub struct UpdateProfileRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub routing: Option<serde_json::Value>,
}

/// Update an existing profile
pub async fn update_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateProfileRequest>,
) -> Result<Json<ProfileDetails>, ApiError> {
    tracing::info!(
        profile_id = %id,
        name = ?request.name,
        "Updating model profile"
    );

    let catalog = state.profile_catalog.read().await;

    // Get existing profile
    let mut profile = catalog
        .get(&id)
        .await
        .map_err(|_| ApiError::not_found(format!("Profile '{}' not found", id)))?;

    // Apply updates
    if let Some(name) = request.name {
        profile.name = name;
    }
    if let Some(desc) = request.description {
        profile.description = desc;
    }
    if let Some(routing_json) = request.routing {
        profile.routing = serde_json::from_value(routing_json)
            .map_err(|e| ApiError::bad_request(format!("Invalid routing config: {}", e)))?;
    }

    // Update in catalog
    catalog.upsert(profile.clone()).await;

    // Persist to YAML
    if let Err(e) = catalog.save_to_yaml().await {
        tracing::warn!(error = %e, "Failed to persist profile update to YAML");
    }

    Ok(Json(ProfileDetails {
        id: profile.id.clone(),
        name: profile.name,
        description: profile.description,
        strategy: format!("{:?}", profile.routing.strategy),
        routing: serde_json::to_value(&profile.routing).unwrap_or_default(),
    }))
}

/// Delete a profile
pub async fn delete_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    tracing::info!(
        profile_id = %id,
        "Deleting model profile"
    );

    let catalog = state.profile_catalog.read().await;

    // Remove from catalog
    let removed = catalog
        .remove(&id)
        .await
        .map_err(|_| ApiError::not_found(format!("Profile '{}' not found", id)))?;

    // Persist to YAML
    if let Err(e) = catalog.save_to_yaml().await {
        tracing::warn!(error = %e, "Failed to persist profile deletion to YAML");
    }

    tracing::info!(profile_id = %id, name = %removed.name, "Profile deleted");

    Ok(Json(serde_json::json!({
        "success": true,
        "deleted": {
            "id": removed.id,
            "name": removed.name
        }
    })))
}

/// Template summary
#[derive(Debug, Serialize)]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub strategy: String,
}

/// List available profile templates
pub async fn list_templates(
    State(state): State<AppState>,
) -> Result<Json<Vec<TemplateSummary>>, ApiError> {
    let catalog = state.profile_catalog.read().await;
    let templates = catalog.list_templates().await;

    let summaries: Vec<TemplateSummary> = templates
        .iter()
        .map(|template| TemplateSummary {
            id: template.template_id.clone(),
            name: template.name.clone(),
            description: template.description.clone(),
            strategy: format!("{:?}", template.base_profile.routing.strategy),
        })
        .collect();
    Ok(Json(summaries))
}

/// Instantiate template request
#[derive(Debug, Deserialize)]
pub struct InstantiateTemplateRequest {
    pub new_profile_id: String,
    pub name: Option<String>,
    pub overrides: Option<serde_json::Value>,
}

/// Instantiate a template as a new profile
pub async fn instantiate_template(
    State(state): State<AppState>,
    Path(template_id): Path<String>,
    Json(request): Json<InstantiateTemplateRequest>,
) -> Result<Json<ProfileDetails>, ApiError> {
    tracing::info!(
        template_id = %template_id,
        new_profile_id = %request.new_profile_id,
        name = ?request.name,
        "Instantiating profile from template"
    );

    let catalog = state.profile_catalog.read().await;

    // Create profile from template
    let mut profile = catalog
        .create_from_template(&template_id, &request.new_profile_id)
        .await
        .map_err(|e| ApiError::not_found(e.to_string()))?;

    // Apply optional name override
    if let Some(name) = request.name {
        profile.name = name;
        // Update in catalog with the new name
        catalog.upsert(profile.clone()).await;
    }

    // Apply optional routing overrides
    if let Some(overrides) = request.overrides {
        if let Some(overrides_obj) = overrides.as_object() {
            // Merge overrides into existing routing config
            let mut routing_json = serde_json::to_value(&profile.routing).unwrap_or_default();
            if let Some(routing_obj) = routing_json.as_object_mut() {
                for (key, value) in overrides_obj {
                    routing_obj.insert(key.clone(), value.clone());
                }
            }
            profile.routing = serde_json::from_value(routing_json)
                .map_err(|e| ApiError::bad_request(format!("Invalid routing override: {}", e)))?;
            catalog.upsert(profile.clone()).await;
        }
    }

    // Persist to YAML
    if let Err(e) = catalog.save_to_yaml().await {
        tracing::warn!(error = %e, "Failed to persist new profile to YAML");
    }

    tracing::info!(
        template_id = %template_id,
        new_profile_id = %profile.id,
        name = %profile.name,
        "Profile created from template"
    );

    Ok(Json(ProfileDetails {
        id: profile.id,
        name: profile.name,
        description: profile.description,
        strategy: format!("{:?}", profile.routing.strategy),
        routing: serde_json::to_value(&profile.routing).unwrap_or_default(),
    }))
}

/// Provider health status
#[derive(Debug, Serialize)]
pub struct ProviderHealth {
    pub provider: String,
    pub state: String,
    pub consecutive_failures: u32,
    pub total_requests: u64,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
}

/// Overall health status
#[derive(Debug, Serialize)]
pub struct HealthStatus {
    pub providers: Vec<ProviderHealth>,
    pub total_providers: usize,
    pub healthy_count: usize,
    pub unhealthy_count: usize,
}

/// Get overall health status
pub async fn get_health_status(
    State(state): State<AppState>,
) -> Result<Json<HealthStatus>, ApiError> {
    let snapshots = state.health_tracker.get_all_status();

    let providers: Vec<ProviderHealth> = snapshots
        .iter()
        .map(|(key, snapshot)| ProviderHealth {
            provider: key.clone(),
            state: snapshot.state.clone(),
            consecutive_failures: snapshot.consecutive_failures,
            total_requests: snapshot.total_requests,
            success_rate: snapshot.success_rate,
            avg_latency_ms: snapshot.avg_latency_ms,
        })
        .collect();

    let healthy_count = providers
        .iter()
        .filter(|p| p.state.contains("Closed"))
        .count();
    let unhealthy_count = providers.len() - healthy_count;

    Ok(Json(HealthStatus {
        total_providers: providers.len(),
        healthy_count,
        unhealthy_count,
        providers,
    }))
}

/// Get health status for a specific provider
pub async fn get_provider_health(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> Result<Json<ProviderHealth>, ApiError> {
    let snapshots = state.health_tracker.get_all_status();

    // Find the provider (may have model suffix)
    for (key, snapshot) in snapshots.iter() {
        if key.starts_with(&provider) || key == &provider {
            return Ok(Json(ProviderHealth {
                provider: key.clone(),
                state: snapshot.state.clone(),
                consecutive_failures: snapshot.consecutive_failures,
                total_requests: snapshot.total_requests,
                success_rate: snapshot.success_rate,
                avg_latency_ms: snapshot.avg_latency_ms,
            }));
        }
    }

    Err(ApiError::not_found(format!(
        "Provider '{}' not found in health tracker",
        provider
    )))
}

/// Cost summary
#[derive(Debug, Serialize)]
pub struct CostSummary {
    pub total_cost_usd: f64,
    pub total_tokens: i64,
    pub by_provider: HashMap<String, ProviderCost>,
    pub by_model: HashMap<String, ModelCost>,
}

/// Cost per provider
#[derive(Debug, Serialize)]
pub struct ProviderCost {
    pub provider: String,
    pub cost_usd: f64,
    pub request_count: u64,
    pub total_tokens: i64,
}

/// Cost per model
#[derive(Debug, Serialize)]
pub struct ModelCost {
    pub model: String,
    pub cost_usd: f64,
    pub request_count: u64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

/// Get cost summary
pub async fn get_cost_summary(
    State(state): State<AppState>,
) -> Result<Json<CostSummary>, ApiError> {
    let records = state.cost_tracker.get_summary();

    let mut total_cost = 0.0;
    let mut total_tokens: i64 = 0;
    let mut by_provider: HashMap<String, ProviderCost> = HashMap::new();
    let mut by_model: HashMap<String, ModelCost> = HashMap::new();

    for record in records {
        total_cost += record.estimated_cost_usd;
        let record_total_tokens = record.total_input_tokens + record.total_output_tokens;
        total_tokens += record_total_tokens as i64;

        // Aggregate by provider
        let provider_cost = by_provider
            .entry(record.provider.clone())
            .or_insert_with(|| ProviderCost {
                provider: record.provider.clone(),
                cost_usd: 0.0,
                request_count: 0,
                total_tokens: 0,
            });
        provider_cost.cost_usd += record.estimated_cost_usd;
        provider_cost.request_count += record.total_requests;
        provider_cost.total_tokens += record_total_tokens as i64;

        // Aggregate by model
        let model_cost = by_model
            .entry(record.model.clone())
            .or_insert_with(|| ModelCost {
                model: record.model.clone(),
                cost_usd: 0.0,
                request_count: 0,
                prompt_tokens: 0,
                completion_tokens: 0,
            });
        model_cost.cost_usd += record.estimated_cost_usd;
        model_cost.request_count += record.total_requests;
        model_cost.prompt_tokens += record.total_input_tokens as i64;
        model_cost.completion_tokens += record.total_output_tokens as i64;
    }

    Ok(Json(CostSummary {
        total_cost_usd: total_cost,
        total_tokens,
        by_provider,
        by_model,
    }))
}

/// Profile-specific cost summary
#[derive(Debug, Serialize)]
pub struct ProfileCostSummary {
    pub profile_id: String,
    pub total_cost_usd: f64,
    pub total_tokens: i64,
    pub request_count: u64,
    pub by_model: HashMap<String, ModelCost>,
}

/// Get costs for a specific profile
pub async fn get_profile_costs(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
) -> Result<Json<ProfileCostSummary>, ApiError> {
    // Verify profile exists
    let catalog = state.profile_catalog.read().await;
    if catalog.get(&profile_id).await.is_err() {
        return Err(ApiError::not_found(format!(
            "Profile '{}' not found",
            profile_id
        )));
    }
    drop(catalog);

    let records = state.cost_tracker.get_summary();

    // Filter records by profile (TODO: Add profile_id to ModelUsageRecord)
    let mut total_cost = 0.0;
    let mut total_tokens: i64 = 0;
    let mut request_count: u64 = 0;
    let mut by_model: HashMap<String, ModelCost> = HashMap::new();

    for record in records {
        // For now, include all records (profile filtering not yet implemented)
        total_cost += record.estimated_cost_usd;
        let record_total_tokens = record.total_input_tokens + record.total_output_tokens;
        total_tokens += record_total_tokens as i64;
        request_count += record.total_requests;

        let model_cost = by_model
            .entry(record.model.clone())
            .or_insert_with(|| ModelCost {
                model: record.model.clone(),
                cost_usd: 0.0,
                request_count: 0,
                prompt_tokens: 0,
                completion_tokens: 0,
            });
        model_cost.cost_usd += record.estimated_cost_usd;
        model_cost.request_count += record.total_requests;
        model_cost.prompt_tokens += record.total_input_tokens as i64;
        model_cost.completion_tokens += record.total_output_tokens as i64;
    }

    Ok(Json(ProfileCostSummary {
        profile_id,
        total_cost_usd: total_cost,
        total_tokens,
        request_count,
        by_model,
    }))
}

/// Test profile request
#[derive(Debug, Deserialize)]
pub struct TestProfileRequest {
    pub message: Option<String>,
    pub task_type: Option<String>,
}

/// Test profile response
#[derive(Debug, Serialize)]
pub struct TestProfileResponse {
    pub success: bool,
    pub profile_id: String,
    pub resolved_provider: String,
    pub resolved_model: String,
    pub routing_reason: String,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// Test a profile with a sample request
pub async fn test_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<TestProfileRequest>,
) -> Result<Json<TestProfileResponse>, ApiError> {
    // Verify profile exists
    {
        let catalog = state.profile_catalog.read().await;
        if catalog.get(&id).await.is_err() {
            return Err(ApiError::not_found(format!("Profile '{}' not found", id)));
        }
    }

    // Check if routing engine is available
    let routing_engine = state.routing_engine.as_ref().ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Routing engine not available",
        )
    })?;

    let start = std::time::Instant::now();

    // Create a test chat request
    use gateway_core::llm::{ChatRequest, Message};
    let test_message = request
        .message
        .unwrap_or_else(|| "Test message".to_string());
    let test_request = ChatRequest {
        messages: vec![Message::text("user", &test_message)],
        profile_id: Some(id.clone()),
        task_type: request.task_type,
        ..Default::default()
    };

    // Try to route the request
    match routing_engine.route_with_profile(&id, &test_request).await {
        Ok(decision) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            Ok(Json(TestProfileResponse {
                success: true,
                profile_id: id,
                resolved_provider: decision.target.provider,
                resolved_model: decision.target.model,
                routing_reason: decision.reason,
                latency_ms,
                error: None,
            }))
        }
        Err(e) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            Ok(Json(TestProfileResponse {
                success: false,
                profile_id: id,
                resolved_provider: String::new(),
                resolved_model: String::new(),
                routing_reason: String::new(),
                latency_ms,
                error: Some(e.to_string()),
            }))
        }
    }
}
