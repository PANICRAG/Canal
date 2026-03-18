//! Billing v2 routes — PigaToken-based billing via billing-core crate
//!
//! Endpoints match the frontend `billingApi.ts` contract exactly.
//! Feature-gated behind `billing` feature flag.

use axum::{
    extract::{Query, State},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;

use crate::middleware::auth::AuthContext;
use crate::state::AppState;

/// Create billing v2 router
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/balance", get(get_balance))
        .route("/events", get(get_events))
        .route("/summary", get(get_summary))
        .route("/pricing", get(get_pricing))
        .route("/plan", get(get_plan))
        .route("/gift-cards/validate", post(validate_gift_card))
        .route("/gift-cards/redeem", post(redeem_gift_card))
        .route("/budget", get(get_budget).put(set_budget))
}

// ============================================================================
// Query / Request types
// ============================================================================

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Deserialize)]
struct SummaryQuery {
    #[serde(default = "default_period")]
    period: String,
}

fn default_period() -> String {
    "month".to_string()
}

#[derive(Debug, Deserialize)]
struct GiftCardRequest {
    code: String,
}

#[derive(Debug, Deserialize)]
struct SetBudgetRequest {
    monthly_budget_limit_mpt: Option<i64>,
    alert_threshold_percent: Option<f64>,
}

// ============================================================================
// Handlers
// ============================================================================

/// GET /api/billing/balance
async fn get_balance(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let user_id = auth.user_id;
    match state.billing_service_v2.get_balance(user_id).await {
        Ok(balance) => Ok(Json(serde_json::to_value(balance).unwrap())),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        )),
    }
}

/// GET /api/billing/events?limit=50
async fn get_events(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Query(query): Query<EventsQuery>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let user_id = auth.user_id;
    match state.billing_service_v2.get_events(user_id, query.limit).await {
        Ok(events) => Ok(Json(serde_json::to_value(events).unwrap())),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        )),
    }
}

/// GET /api/billing/summary?period=month
async fn get_summary(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Query(query): Query<SummaryQuery>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let user_id = auth.user_id;
    match state
        .billing_service_v2
        .get_spending_summary(user_id, &query.period)
        .await
    {
        Ok(summary) => Ok(Json(serde_json::to_value(summary).unwrap())),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        )),
    }
}

/// GET /api/billing/pricing
async fn get_pricing(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let pricing = state.billing_service_v2.get_pricing().await;
    Json(serde_json::to_value(pricing).unwrap())
}

/// GET /api/billing/plan
async fn get_plan(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let user_id = auth.user_id;
    match state.billing_service_v2.get_current_plan(user_id).await {
        Ok(plan) => Ok(Json(serde_json::to_value(plan).unwrap())),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        )),
    }
}

/// POST /api/billing/gift-cards/validate
async fn validate_gift_card(
    State(state): State<AppState>,
    Json(body): Json<GiftCardRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    match state.gift_card_service_v2.validate(&body.code).await {
        Ok(result) => Ok(Json(serde_json::to_value(result).unwrap())),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        )),
    }
}

/// POST /api/billing/gift-cards/redeem
async fn redeem_gift_card(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(body): Json<GiftCardRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let user_id = auth.user_id;
    match state
        .gift_card_service_v2
        .redeem(&body.code, user_id)
        .await
    {
        Ok(result) => Ok(Json(serde_json::to_value(result).unwrap())),
        Err(e) => Err((axum::http::StatusCode::BAD_REQUEST, e.to_string())),
    }
}

/// GET /api/billing/budget
async fn get_budget(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let user_id = auth.user_id;
    match state.billing_service_v2.get_budget(user_id).await {
        Ok(budget) => Ok(Json(serde_json::to_value(budget).unwrap())),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        )),
    }
}

/// PUT /api/billing/budget
async fn set_budget(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(body): Json<SetBudgetRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let user_id = auth.user_id;
    match state
        .billing_service_v2
        .set_budget(
            user_id,
            body.monthly_budget_limit_mpt,
            body.alert_threshold_percent,
        )
        .await
    {
        Ok(budget) => Ok(Json(serde_json::to_value(budget).unwrap())),
        Err(e) => Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e.to_string(),
        )),
    }
}
