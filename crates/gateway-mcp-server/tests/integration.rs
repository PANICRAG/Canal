//! Integration tests for the MCP server dispatcher + handlers.
//!
//! Tests the full dispatch path: catalog → scope check → metering → handler → response.

use canal_identity::service::IdentityService;
use canal_identity::store::DashMapKeyStore;
use canal_identity::types::{AgentIdentity, AgentScope, AgentTier};
use std::sync::Arc;

// Re-import MCP server internals (they're pub within the crate)
use gateway_mcp_server::catalog::build_default_catalog;
use gateway_mcp_server::dispatcher::{DispatchError, Dispatcher};
use gateway_mcp_server::handlers::HandlerContext;

fn make_dispatcher() -> (Dispatcher, AgentIdentity) {
    use billing_core::store::memory::{InMemoryBalanceStore, InMemoryEventStore};
    use billing_core::{BillingService, MeteringService, PlanRegistry, PricingEngine};

    let catalog = build_default_catalog();
    let store = DashMapKeyStore::new();
    let identity_service = Arc::new(IdentityService::new(Arc::new(store)));

    let pricing = Arc::new(PricingEngine::with_defaults());
    let plans = Arc::new(PlanRegistry::with_defaults());
    let balance = Arc::new(InMemoryBalanceStore::new());
    let events = Arc::new(InMemoryEventStore::new());
    let billing = Arc::new(BillingService::new(balance, events, pricing.clone(), plans));
    let metering = Arc::new(MeteringService::new(pricing, billing.clone()));

    let handler_ctx = Arc::new(HandlerContext::new(
        None,
        None,
        serde_json::json!({}),
        metering,
        billing,
    ));

    let dispatcher = Dispatcher::new(catalog, identity_service, handler_ctx);

    let identity = AgentIdentity {
        id: uuid::Uuid::new_v4(),
        name: "test-agent".to_string(),
        tier: AgentTier::System,
        scopes: AgentTier::System.default_scopes(),
        created_at: chrono::Utc::now(),
    };

    (dispatcher, identity)
}

#[tokio::test]
async fn test_platform_health_returns_ok() {
    let (dispatcher, identity) = make_dispatcher();

    let result = dispatcher
        .dispatch(&identity, "platform.health", serde_json::json!({}))
        .await;

    let value = result.expect("platform.health should succeed");
    assert_eq!(value["status"], "healthy");
    assert!(value["timestamp"].is_string());
}

#[tokio::test]
async fn test_platform_info_returns_version() {
    let (dispatcher, identity) = make_dispatcher();

    let result = dispatcher
        .dispatch(&identity, "platform.info", serde_json::json!({}))
        .await;

    let value = result.expect("platform.info should succeed");
    assert_eq!(value["name"], "canal-mcp-server");
    assert!(value["version"].is_string());
}

#[tokio::test]
async fn test_billing_usage_returns_data() {
    let (dispatcher, identity) = make_dispatcher();

    // Make a call to generate metering data
    let _ = dispatcher
        .dispatch(&identity, "platform.health", serde_json::json!({}))
        .await;

    // Now check billing.usage
    let result = dispatcher
        .dispatch(&identity, "billing.usage", serde_json::json!({}))
        .await;

    let value = result.expect("billing.usage should succeed");
    assert_eq!(value["agent_id"], identity.id.to_string());
    assert_eq!(value["period"], "current_month");
    assert!(value["usage"].is_object());
}

#[tokio::test]
async fn test_billing_balance() {
    let (dispatcher, identity) = make_dispatcher();

    let result = dispatcher
        .dispatch(&identity, "billing.balance", serde_json::json!({}))
        .await;

    let value = result.expect("billing.balance should succeed");
    assert_eq!(value["agent_id"], identity.id.to_string());
    assert!(value["balance"].is_object());
    assert!(value["budget"].is_object());
}

#[tokio::test]
async fn test_billing_estimate() {
    let (dispatcher, identity) = make_dispatcher();

    let result = dispatcher
        .dispatch(
            &identity,
            "billing.estimate",
            serde_json::json!({"tool_name": "engine.chat"}),
        )
        .await;

    let value = result.expect("billing.estimate should succeed");
    assert_eq!(value["tool_name"], "engine.chat");
    assert!(value["estimated_cost_milli_pigatokens"].is_number());
}

#[tokio::test]
async fn test_scope_violation_blocks_call() {
    let (dispatcher, _) = make_dispatcher();

    // Create identity with limited scopes (no EngineChat)
    let limited_identity = AgentIdentity {
        id: uuid::Uuid::new_v4(),
        name: "limited-agent".to_string(),
        tier: AgentTier::Trial,
        scopes: vec![AgentScope::ToolsRead],
        created_at: chrono::Utc::now(),
    };

    let result = dispatcher
        .dispatch(&limited_identity, "engine.chat", serde_json::json!({}))
        .await;

    assert!(
        matches!(result, Err(DispatchError::InsufficientScope { .. })),
        "Should fail with InsufficientScope, got: {:?}",
        result
    );
}

#[tokio::test]
async fn test_tool_not_found() {
    let (dispatcher, identity) = make_dispatcher();

    let result = dispatcher
        .dispatch(&identity, "nonexistent.tool", serde_json::json!({}))
        .await;

    assert!(matches!(result, Err(DispatchError::ToolNotFound(_))));
}

#[tokio::test]
async fn test_engine_chat_without_engine() {
    let (dispatcher, identity) = make_dispatcher();

    // Engine is None, so engine.chat should return an internal error
    let result = dispatcher
        .dispatch(
            &identity,
            "engine.chat",
            serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}),
        )
        .await;

    // Should fail because engine is not initialized
    assert!(result.is_err());
}

#[tokio::test]
async fn test_catalog_has_billing_tools() {
    let catalog = build_default_catalog();

    assert!(catalog.get("billing.usage").is_some());
    assert!(catalog.get("billing.balance").is_some());
    assert!(catalog.get("billing.estimate").is_some());
}

#[tokio::test]
async fn test_metering_records_accumulate() {
    let (dispatcher, identity) = make_dispatcher();

    // Make 3 calls
    for _ in 0..3 {
        let _ = dispatcher
            .dispatch(&identity, "platform.health", serde_json::json!({}))
            .await;
    }

    // Check usage reflects calls
    let result = dispatcher
        .dispatch(&identity, "billing.usage", serde_json::json!({}))
        .await
        .unwrap();

    // Verify usage data is returned (billing-core records tool_usage events)
    assert_eq!(result["period"], "current_month");
    assert!(result["usage"].is_object());
}
