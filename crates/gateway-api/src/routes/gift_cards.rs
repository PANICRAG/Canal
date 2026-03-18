//! Gift card and billing routes
//!
//! Admin endpoints for gift card management and user endpoints for
//! balance, transactions, and gift card redemption.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json, Router,
};
use gateway_core::billing::gift_card::GiftCardService;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::ApiError, middleware::auth::AuthContext, state::AppState};

// ---------------------------------------------------------------------------
// Admin Routes
// ---------------------------------------------------------------------------

/// Admin routes for gift card management (nest under /admin/gift-cards)
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/generate", post(admin_generate_cards))
        .route("/", get(admin_list_cards))
        .route("/stats", get(admin_get_stats))
        .route("/{code}/disable", post(admin_disable_card))
}

/// User-facing billing routes (nest under /billing)
pub fn billing_routes() -> Router<AppState> {
    Router::new()
        .route("/redeem", post(redeem_gift_card))
        .route("/balance", get(get_balance))
        .route("/transactions", get(get_transactions))
}

// ---------------------------------------------------------------------------
// Request / Response Types
// ---------------------------------------------------------------------------

/// Request to generate gift cards (admin)
#[derive(Debug, Deserialize)]
pub struct GenerateCardsRequest {
    pub count: u32,
    pub amount_usd: f64,
    pub expires_days: Option<i64>,
}

/// Query params for listing gift cards (admin)
#[derive(Debug, Deserialize)]
pub struct ListCardsQuery {
    pub status: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// Request to redeem a gift card (user)
#[derive(Debug, Deserialize)]
pub struct RedeemRequest {
    pub code: String,
}

/// Transaction history query params
#[derive(Debug, Deserialize)]
pub struct TransactionQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

/// Balance response
#[derive(Debug, Serialize)]
pub struct BalanceResponse {
    pub user_id: Uuid,
    pub balance_usd: f64,
    pub currency: String,
}

// ---------------------------------------------------------------------------
// Admin Handlers
// ---------------------------------------------------------------------------

/// Generate gift cards (admin only)
///
/// Creates a batch of gift cards with the specified amount and optional expiry.
pub async fn admin_generate_cards(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<GenerateCardsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Admin check
    if auth.role != "admin" {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Admin access required",
        ));
    }

    // Validate input
    if req.count == 0 || req.count > 1000 {
        return Err(ApiError::bad_request(
            "Count must be between 1 and 1000",
        ));
    }
    if req.amount_usd <= 0.0 {
        return Err(ApiError::bad_request(
            "Amount must be greater than 0",
        ));
    }

    let gift_card_service = GiftCardService::new(state.db.clone());

    let cards = gift_card_service
        .generate_cards(req.count, req.amount_usd, req.expires_days, None, auth.user_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to generate gift cards: {}", e)))?;

    Ok(Json(serde_json::json!({
        "count": cards.len(),
        "cards": cards,
    })))
}

/// List gift cards with optional status filter (admin only)
pub async fn admin_list_cards(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<ListCardsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Admin check
    if auth.role != "admin" {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Admin access required",
        ));
    }

    let gift_card_service = GiftCardService::new(state.db.clone());

    let limit = query.limit.min(200).max(1);
    let offset = query.offset.max(0);

    let cards = gift_card_service
        .list_cards(query.status.as_deref(), None, limit, offset)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list gift cards: {}", e)))?;

    Ok(Json(serde_json::json!({
        "cards": cards,
        "count": cards.len(),
        "limit": limit,
        "offset": offset,
    })))
}

/// Get gift card statistics (admin only)
pub async fn admin_get_stats(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Admin check
    if auth.role != "admin" {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Admin access required",
        ));
    }

    let gift_card_service = GiftCardService::new(state.db.clone());

    let stats = gift_card_service
        .get_stats()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get gift card stats: {}", e)))?;

    Ok(Json(serde_json::json!(stats)))
}

/// Disable a gift card by code (admin only)
pub async fn admin_disable_card(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Path(code): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Admin check
    if auth.role != "admin" {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Admin access required",
        ));
    }

    let gift_card_service = GiftCardService::new(state.db.clone());

    let card = gift_card_service
        .disable_card(&code)
        .await
        .map_err(|e| match &e {
            gateway_core::Error::NotFound(_) => ApiError::not_found(format!("{}", e)),
            _ => ApiError::internal(format!("Failed to disable gift card: {}", e)),
        })?;

    Ok(Json(serde_json::json!({
        "message": "Gift card disabled",
        "card": card,
    })))
}

// ---------------------------------------------------------------------------
// User Billing Handlers
// ---------------------------------------------------------------------------

/// Redeem a gift card
pub async fn redeem_gift_card(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<RedeemRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.code.trim().is_empty() {
        return Err(ApiError::bad_request("Gift card code cannot be empty"));
    }

    let gift_card_service = GiftCardService::new(state.db.clone());

    let result = gift_card_service
        .redeem_card(&req.code, auth.user_id)
        .await
        .map_err(|e| match &e {
            gateway_core::Error::NotFound(_) => ApiError::not_found(format!("{}", e)),
            gateway_core::Error::InvalidInput(msg) => ApiError::bad_request(msg.clone()),
            _ => ApiError::internal(format!("Failed to redeem gift card: {}", e)),
        })?;

    Ok(Json(serde_json::json!(result)))
}

/// Get current balance
pub async fn get_balance(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<BalanceResponse>, ApiError> {
    let billing_service = state.billing_service.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Billing service not available",
        )
    })?;

    let balance = billing_service
        .get_balance(auth.user_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get balance: {}", e)))?;

    Ok(Json(BalanceResponse {
        user_id: auth.user_id,
        balance_usd: balance,
        currency: "USD".to_string(),
    }))
}

/// Get transaction history
pub async fn get_transactions(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<TransactionQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let billing_service = state.billing_service.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Billing service not available",
        )
    })?;

    let limit = query.limit.min(200).max(1);
    let offset = query.offset.max(0);

    let transactions = billing_service
        .get_transactions(auth.user_id, limit, offset)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get transactions: {}", e)))?;

    Ok(Json(serde_json::json!({
        "transactions": transactions,
        "count": transactions.len(),
        "limit": limit,
        "offset": offset,
    })))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_admin_routes_creates_router() {
        let _router = admin_routes();
    }

    #[test]
    fn test_billing_routes_creates_router() {
        let _router = billing_routes();
    }

    #[test]
    fn test_generate_cards_request_deserialize() {
        let json = r#"{"count":10,"amount_usd":25.0,"expires_days":90}"#;
        let req: GenerateCardsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.count, 10);
        assert_eq!(req.amount_usd, 25.0);
        assert_eq!(req.expires_days, Some(90));
    }

    #[test]
    fn test_generate_cards_request_no_expiry() {
        let json = r#"{"count":5,"amount_usd":10.0}"#;
        let req: GenerateCardsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.count, 5);
        assert_eq!(req.amount_usd, 10.0);
        assert!(req.expires_days.is_none());
    }

    #[test]
    fn test_list_cards_query_defaults() {
        let json = r#"{}"#;
        let query: ListCardsQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.limit, 50);
        assert_eq!(query.offset, 0);
        assert!(query.status.is_none());
    }

    #[test]
    fn test_list_cards_query_with_status() {
        let json = r#"{"status":"active","limit":20,"offset":10}"#;
        let query: ListCardsQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.status, Some("active".to_string()));
        assert_eq!(query.limit, 20);
        assert_eq!(query.offset, 10);
    }

    #[test]
    fn test_redeem_request_deserialize() {
        let json = r#"{"code":"ABCD-EFGH-JKMN-PQRS"}"#;
        let req: RedeemRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.code, "ABCD-EFGH-JKMN-PQRS");
    }

    #[test]
    fn test_transaction_query_defaults() {
        let json = r#"{}"#;
        let query: TransactionQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.limit, 50);
        assert_eq!(query.offset, 0);
    }

    #[test]
    fn test_balance_response_serialize() {
        let resp = BalanceResponse {
            user_id: Uuid::nil(),
            balance_usd: 42.50,
            currency: "USD".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("42.5"));
        assert!(json.contains("USD"));
    }
}
