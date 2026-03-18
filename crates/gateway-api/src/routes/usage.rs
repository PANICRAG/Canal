//! Usage and billing API endpoints
//!
//! Provides per-user usage tracking, cost summaries, and billing history.

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::ApiError, middleware::auth::AuthContext, state::AppState};

/// Create the usage routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(get_usage_summary))
        .route("/summary", get(get_usage_summary))
        .route("/history", get(get_usage_history))
        .route("/balance", get(get_balance))
        .route("/daily", get(get_daily_usage))
        .route("/budget", get(get_budget).put(set_budget))
}

/// Query parameters for usage summary
#[derive(Debug, Deserialize)]
pub struct UsageSummaryQuery {
    /// Semantic period: "today", "week", "month" (frontend-friendly)
    pub period: Option<String>,
    /// Start date for the usage period (ISO 8601) - overrides period if provided
    pub start_date: Option<DateTime<Utc>>,
    /// End date for the usage period (ISO 8601) - overrides period if provided
    pub end_date: Option<DateTime<Utc>>,
}

/// Convert semantic period to date range
fn period_to_date_range(period: Option<&str>) -> (DateTime<Utc>, DateTime<Utc>) {
    let now = Utc::now();
    let start = match period {
        Some("today") => now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc(),
        Some("week") => (now - Duration::days(7))
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc(),
        Some("month") => now
            .date_naive()
            .with_day(1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc(),
        _ => now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc(), // default to today
    };
    (start, now)
}

/// Query parameters for usage history
#[derive(Debug, Deserialize)]
pub struct UsageHistoryQuery {
    /// Maximum number of events to return
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Offset for pagination
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// Frontend-compatible usage response (camelCase)
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendUsageResponse {
    pub period: String,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub api_calls: u64,
    pub cost_usd: f64,
    pub model_breakdown: Vec<ModelBreakdownItem>,
}

/// Model breakdown item for frontend
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelBreakdownItem {
    pub model: String,
    pub tokens: u64,
    pub calls: u64,
    pub percentage: f64,
}

/// Usage summary response (original format, kept for backward compatibility)
#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct UsageSummaryResponse {
    pub user_id: Uuid,
    pub period: UsagePeriod,
    pub totals: UsageTotals,
    pub by_model: Vec<ModelUsageResponse>,
    pub by_provider: Vec<ProviderUsageResponse>,
}

/// Usage period
#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct UsagePeriod {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Total usage statistics
#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct UsageTotals {
    pub request_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: f64,
}

/// Usage breakdown by model
#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct ModelUsageResponse {
    pub model: String,
    pub request_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// Usage breakdown by provider
#[allow(dead_code)]
#[derive(Debug, Serialize)]
pub struct ProviderUsageResponse {
    pub provider: String,
    pub request_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// Usage event for history
#[derive(Debug, Serialize)]
pub struct UsageEventResponse {
    pub id: Uuid,
    pub event_type: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub cost_usd: f64,
    pub timestamp: DateTime<Utc>,
}

/// Balance response
#[derive(Debug, Serialize)]
pub struct BalanceResponse {
    pub user_id: Uuid,
    pub balance_usd: f64,
    pub currency: String,
}

/// Query parameters for daily usage
#[derive(Debug, Deserialize)]
pub struct DailyUsageQuery {
    /// Number of days to look back (default 30)
    #[serde(default = "default_days")]
    pub days: i64,
}

fn default_days() -> i64 {
    30
}

/// Budget response
#[derive(Debug, Serialize)]
pub struct BudgetResponse {
    pub user_id: Uuid,
    pub monthly_budget_limit_usd: Option<f64>,
}

/// Set budget request
#[derive(Debug, Deserialize)]
pub struct SetBudgetRequest {
    pub limit: Option<f64>,
}

/// Get usage summary for the authenticated user
/// Returns frontend-compatible format with camelCase fields
pub async fn get_usage_summary(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Query(query): Query<UsageSummaryQuery>,
) -> Result<Json<FrontendUsageResponse>, ApiError> {
    let user_id = auth.user_id;

    let billing_service = state.billing_service.as_ref().ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Billing service not available",
        )
    })?;

    // Determine date range: explicit dates take precedence over period
    let (start_date, end_date) = if query.start_date.is_some() || query.end_date.is_some() {
        (query.start_date, query.end_date)
    } else {
        let (start, end) = period_to_date_range(query.period.as_deref());
        (Some(start), Some(end))
    };

    let summary = billing_service
        .get_usage_summary(user_id, start_date, end_date)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get usage summary: {}", e)))?;

    let total_tokens = summary.total_input_tokens + summary.total_output_tokens;

    // Build model breakdown with percentages
    let mut model_breakdown: Vec<ModelBreakdownItem> = summary
        .by_model
        .into_values()
        .map(|m| {
            let model_tokens = m.input_tokens + m.output_tokens;
            let percentage = if total_tokens > 0 {
                (model_tokens as f64 / total_tokens as f64) * 100.0
            } else {
                0.0
            };
            ModelBreakdownItem {
                model: m.model,
                tokens: model_tokens,
                calls: m.request_count,
                percentage: (percentage * 10.0).round() / 10.0, // Round to 1 decimal
            }
        })
        .collect();

    // Sort by tokens descending
    model_breakdown.sort_by(|a, b| b.tokens.cmp(&a.tokens));

    // Determine period string for response
    let period_str = query.period.clone().unwrap_or_else(|| "today".to_string());

    Ok(Json(FrontendUsageResponse {
        period: period_str,
        total_tokens,
        input_tokens: summary.total_input_tokens,
        output_tokens: summary.total_output_tokens,
        api_calls: summary.total_requests,
        cost_usd: (summary.total_cost_usd * 1000.0).round() / 1000.0, // Round to 3 decimals
        model_breakdown,
    }))
}

/// Get usage history for the authenticated user
pub async fn get_usage_history(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Query(query): Query<UsageHistoryQuery>,
) -> Result<Json<Vec<UsageEventResponse>>, ApiError> {
    let user_id = auth.user_id;

    let billing_service = state.billing_service.as_ref().ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Billing service not available",
        )
    })?;

    let events = billing_service
        .get_event_history(user_id, query.limit, query.offset)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get usage history: {}", e)))?;

    let response: Vec<UsageEventResponse> = events
        .into_iter()
        .map(|e| UsageEventResponse {
            id: e.id,
            event_type: e.event_type,
            provider: e.provider,
            model: e.model,
            input_tokens: e.input_tokens,
            output_tokens: e.output_tokens,
            cost_usd: e.cost_usd,
            timestamp: e.timestamp,
        })
        .collect();

    Ok(Json(response))
}

/// Get current balance for the authenticated user
pub async fn get_balance(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<BalanceResponse>, ApiError> {
    let user_id = auth.user_id;

    let billing_service = state.billing_service.as_ref().ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Billing service not available",
        )
    })?;

    let balance = billing_service
        .get_balance(user_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get balance: {}", e)))?;

    Ok(Json(BalanceResponse {
        user_id,
        balance_usd: balance,
        currency: "USD".to_string(),
    }))
}

/// Get daily usage data for the authenticated user
pub async fn get_daily_usage(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Query(query): Query<DailyUsageQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = auth.user_id;

    let billing_service = state.billing_service.as_ref().ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Billing service not available",
        )
    })?;

    let days = query.days.min(365).max(1);

    let daily = billing_service
        .get_daily_usage(user_id, days)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get daily usage: {}", e)))?;

    Ok(Json(serde_json::json!({
        "user_id": user_id,
        "days": days,
        "data": daily,
    })))
}

/// Get budget for the authenticated user
pub async fn get_budget(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<BudgetResponse>, ApiError> {
    let user_id = auth.user_id;

    let billing_service = state.billing_service.as_ref().ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Billing service not available",
        )
    })?;

    let budget = billing_service
        .get_budget(user_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get budget: {}", e)))?;

    Ok(Json(BudgetResponse {
        user_id,
        monthly_budget_limit_usd: budget,
    }))
}

/// Set budget for the authenticated user
pub async fn set_budget(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(req): Json<SetBudgetRequest>,
) -> Result<Json<BudgetResponse>, ApiError> {
    let user_id = auth.user_id;

    let billing_service = state.billing_service.as_ref().ok_or_else(|| {
        ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Billing service not available",
        )
    })?;

    // Validate budget limit if provided
    if let Some(limit) = req.limit {
        if limit < 0.0 {
            return Err(ApiError::bad_request("Budget limit cannot be negative"));
        }
    }

    billing_service
        .set_budget(user_id, req.limit)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to set budget: {}", e)))?;

    Ok(Json(BudgetResponse {
        user_id,
        monthly_budget_limit_usd: req.limit,
    }))
}
