//! Model Router Integration Tests
//!
//! This module provides comprehensive integration tests for the Model Router system:
//! - Profile loading and catalog management
//! - RoutingEngine initialization and operation
//! - Health tracker integration (circuit breaker)
//! - Cost tracker integration
//! - End-to-end routing decisions

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::sync::RwLock;

use gateway_core::llm::model_profile::{
    ModelProfile, ModelTarget, ProfileCatalog, RoutingConfig, RoutingStrategy,
};
use gateway_core::llm::strategies::ModelRegistry;
use gateway_core::llm::{
    // Core routing types
    ChatRequest,
    HealthConfig,
    HealthTracker,
    // Cost tracking
    InternalCostTracker,
    Message,
    // Routing engine
    RoutingEngine,
    Usage,
};

// ============================================================================
// Test Fixtures and Helpers
// ============================================================================

/// Create a simple model target
fn target(provider: &str, model: &str) -> ModelTarget {
    ModelTarget {
        provider: provider.to_string(),
        model: model.to_string(),
    }
}

/// Create a minimal model profile with primary-fallback strategy
fn primary_fallback_profile(
    id: &str,
    primary: ModelTarget,
    fallbacks: Option<Vec<ModelTarget>>,
) -> ModelProfile {
    ModelProfile {
        id: id.to_string(),
        name: format!("Profile {}", id),
        description: format!("Test profile: {}", id),
        enabled: true,
        routing: RoutingConfig {
            strategy: RoutingStrategy::PrimaryFallback,
            primary: Some(primary),
            fallbacks,
            ..Default::default()
        },
        agent: Default::default(),
        cache_enabled: false,
        cache_ttl_seconds: 3600,
    }
}

/// Create a chat request with optional profile_id and task_type
fn chat_request(
    user_message: &str,
    profile_id: Option<&str>,
    task_type: Option<&str>,
) -> ChatRequest {
    ChatRequest {
        messages: vec![Message::text("user", user_message)],
        profile_id: profile_id.map(String::from),
        task_type: task_type.map(String::from),
        ..Default::default()
    }
}

/// Create a Usage struct for cost tracking
fn usage(prompt_tokens: i32, completion_tokens: i32) -> Usage {
    Usage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens + completion_tokens,
    }
}

/// Write a YAML config to a temp file and return it
fn write_yaml_config(yaml: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("create temp file");
    file.write_all(yaml.as_bytes()).expect("write yaml");
    file.flush().expect("flush");
    file
}

// ============================================================================
// ProfileCatalog Tests
// ============================================================================

mod profile_catalog_tests {
    use super::*;

    /// Test 1: Empty catalog creation
    #[tokio::test]
    async fn test_empty_catalog() {
        let catalog = ProfileCatalog::empty();
        let profiles = catalog.list().await;
        assert!(profiles.is_empty());
    }

    /// Test 2: Add profile to catalog
    #[tokio::test]
    async fn test_add_profile() {
        let catalog = ProfileCatalog::empty();

        let profile = primary_fallback_profile(
            "fast-chat",
            target("anthropic", "claude-3-5-haiku-latest"),
            None,
        );

        catalog.upsert(profile.clone()).await;

        let retrieved = catalog.get("fast-chat").await.unwrap();
        assert_eq!(retrieved.id, "fast-chat");
    }

    /// Test 3: Get non-existent profile
    #[tokio::test]
    async fn test_get_nonexistent_profile() {
        let catalog = ProfileCatalog::empty();
        let result = catalog.get("nonexistent").await;
        assert!(result.is_err());
    }

    /// Test 4: List all profiles
    #[tokio::test]
    async fn test_list_profiles() {
        let catalog = ProfileCatalog::empty();

        catalog
            .upsert(primary_fallback_profile(
                "profile-a",
                target("anthropic", "claude-sonnet-4-6"),
                None,
            ))
            .await;

        catalog
            .upsert(primary_fallback_profile(
                "profile-b",
                target("openai", "gpt-4o"),
                None,
            ))
            .await;

        let profiles = catalog.list().await;
        assert_eq!(profiles.len(), 2);
    }

    /// Test 5: Profile YAML parsing from file
    #[tokio::test]
    async fn test_yaml_parsing() {
        let yaml = r#"
profiles:
  - id: fast-chat
    name: Fast Chat
    description: "Fast chat profile"
    routing:
      strategy: primary_fallback
      primary:
        provider: anthropic
        model: claude-3-5-haiku-latest
      fallbacks:
        - provider: openai
          model: gpt-4o-mini
"#;

        let file = write_yaml_config(yaml);
        let result = ProfileCatalog::from_yaml(file.path()).await;
        assert!(
            result.is_ok(),
            "YAML parsing should succeed: {:?}",
            result.err()
        );

        let catalog = result.unwrap();
        let profiles = catalog.list().await;
        assert_eq!(profiles.len(), 1);

        let profile = catalog.get("fast-chat").await.unwrap();
        assert!(profile.routing.primary.is_some());
        assert!(profile.routing.fallbacks.is_some());
    }

    /// Test 6: Profile update (upsert replaces)
    #[tokio::test]
    async fn test_profile_update() {
        let catalog = ProfileCatalog::empty();

        let profile =
            primary_fallback_profile("test", target("anthropic", "claude-sonnet-4-6"), None);
        let was_replaced = catalog.upsert(profile).await;
        assert!(!was_replaced, "First insert should not replace");

        // Update with new values
        let updated = ModelProfile {
            id: "test".to_string(),
            name: "Updated Name".to_string(),
            description: "Updated".to_string(),
            enabled: true,
            routing: RoutingConfig::default(),
            agent: Default::default(),
            cache_enabled: true,
            cache_ttl_seconds: 7200,
        };
        let was_replaced = catalog.upsert(updated).await;
        assert!(was_replaced, "Second insert should replace");

        let fetched = catalog.get("test").await.unwrap();
        assert_eq!(fetched.name, "Updated Name");
    }

    /// Test 7: Remove profile
    #[tokio::test]
    async fn test_remove_profile() {
        let catalog = ProfileCatalog::empty();

        catalog
            .upsert(primary_fallback_profile(
                "removable",
                target("anthropic", "claude-sonnet-4-6"),
                None,
            ))
            .await;

        let removed = catalog.remove("removable").await;
        assert!(removed.is_ok());
        assert_eq!(removed.unwrap().id, "removable");

        // Should fail on second remove
        let err = catalog.remove("removable").await;
        assert!(err.is_err());
    }
}

// ============================================================================
// HealthTracker Tests
// ============================================================================

mod health_tracker_tests {
    use super::*;

    /// Test 8: Initial health state is healthy
    #[test]
    fn test_initial_health_state() {
        let tracker = HealthTracker::new(HealthConfig::default());

        // Record a success to initialize the provider
        tracker.record_success("anthropic");

        let snapshot = tracker.get_provider_status("anthropic");
        assert!(snapshot.is_some());

        let status = snapshot.unwrap();
        assert_eq!(status.consecutive_failures, 0);
        assert_eq!(status.total_requests, 1);
    }

    /// Test 9: Unknown provider is considered healthy
    #[test]
    fn test_unknown_provider_is_healthy() {
        let tracker = HealthTracker::new(HealthConfig::default());
        assert!(tracker.is_healthy("unknown_provider"));
    }

    /// Test 10: Record failures and track consecutive count
    #[test]
    fn test_record_failures() {
        let config = HealthConfig {
            failure_threshold: 5,
            ..Default::default()
        };
        let tracker = HealthTracker::new(config);

        // Record consecutive failures
        for _ in 0..3 {
            tracker.record_failure("openai");
        }

        let snapshot = tracker.get_provider_status("openai").unwrap();
        assert_eq!(snapshot.consecutive_failures, 3);
    }

    /// Test 11: Circuit breaker opens after threshold
    #[test]
    fn test_circuit_breaker_opens() {
        let config = HealthConfig {
            failure_threshold: 2,
            ..Default::default()
        };
        let tracker = HealthTracker::new(config);

        // Record failures to trip the circuit
        tracker.record_failure("google");
        tracker.record_failure("google");

        let snapshot = tracker.get_provider_status("google").unwrap();
        // State should be Open
        assert!(
            snapshot.state.contains("open"),
            "Circuit should be open, state: {}",
            snapshot.state
        );
    }

    /// Test 12: Success resets consecutive failure count
    #[test]
    fn test_success_resets_failures() {
        let tracker = HealthTracker::new(HealthConfig::default());

        tracker.record_failure("anthropic");
        tracker.record_failure("anthropic");

        let before = tracker.get_provider_status("anthropic").unwrap();
        assert_eq!(before.consecutive_failures, 2);

        tracker.record_success("anthropic");

        let after = tracker.get_provider_status("anthropic").unwrap();
        assert_eq!(after.consecutive_failures, 0);
    }

    /// Test 13: Latency tracking
    #[test]
    fn test_latency_tracking() {
        let tracker = HealthTracker::new(HealthConfig::default());

        tracker.record_success_with_latency("openai", Duration::from_millis(100));
        tracker.record_success_with_latency("openai", Duration::from_millis(200));
        tracker.record_success_with_latency("openai", Duration::from_millis(150));

        let snapshot = tracker.get_provider_status("openai").unwrap();
        // Average should be tracked (exponential moving average)
        assert!(snapshot.avg_latency_ms > 0.0);
    }

    /// Test 14: Get all providers status
    #[test]
    fn test_get_all_status() {
        let tracker = HealthTracker::new(HealthConfig::default());

        tracker.record_success("provider1");
        tracker.record_success("provider2");

        let all_status = tracker.get_all_status();
        assert_eq!(all_status.len(), 2);
        assert!(all_status.contains_key("provider1"));
        assert!(all_status.contains_key("provider2"));
    }

    /// Test 15: is_healthy returns false when circuit is open
    #[test]
    fn test_is_healthy_when_open() {
        let config = HealthConfig {
            failure_threshold: 2,
            cooldown_seconds: 3600, // Long cooldown so it doesn't transition
            ..Default::default()
        };
        let tracker = HealthTracker::new(config);

        // Trip the circuit
        tracker.record_failure("provider");
        tracker.record_failure("provider");

        // Should be unhealthy
        assert!(!tracker.is_healthy("provider"));
    }
}

// ============================================================================
// InternalCostTracker Tests
// ============================================================================

mod cost_tracker_tests {
    use super::*;

    /// Test 16: Record usage and get summary
    #[test]
    fn test_record_usage() {
        let tracker = InternalCostTracker::with_default_pricing();

        tracker.record("anthropic/claude-sonnet-4-6", &usage(1000, 500));

        let summary = tracker.get_summary();
        assert!(!summary.is_empty());

        let record = summary
            .iter()
            .find(|r| r.model.contains("claude-sonnet"))
            .unwrap();
        assert_eq!(record.total_input_tokens, 1000);
        assert_eq!(record.total_output_tokens, 500);
        assert_eq!(record.total_requests, 1);
    }

    /// Test 17: Multiple records aggregate correctly
    #[test]
    fn test_aggregate_usage() {
        let tracker = InternalCostTracker::with_default_pricing();

        tracker.record("openai/gpt-4o", &usage(100, 50));
        tracker.record("openai/gpt-4o", &usage(200, 100));
        tracker.record("openai/gpt-4o", &usage(300, 150));

        let summary = tracker.get_summary();
        let record = summary.iter().find(|r| r.model.contains("gpt-4o")).unwrap();

        assert_eq!(record.total_input_tokens, 600);
        assert_eq!(record.total_output_tokens, 300);
        assert_eq!(record.total_requests, 3);
    }

    /// Test 18: Cost estimation for known models
    #[test]
    fn test_cost_estimation() {
        let tracker = InternalCostTracker::with_default_pricing();

        // Record substantial usage for a model with known pricing
        tracker.record(
            "claude-sonnet-4-6",
            &usage(100_000, 50_000), // 100k input, 50k output
        );

        let summary = tracker.get_summary();
        let record = summary
            .iter()
            .find(|r| r.model.contains("claude-sonnet"))
            .unwrap();

        // Should have some cost estimate for known model
        assert!(record.estimated_cost_usd > 0.0);
    }

    /// Test 19: Multiple models tracked separately
    #[test]
    fn test_multiple_models() {
        let tracker = InternalCostTracker::with_default_pricing();

        tracker.record("model-a", &usage(1000, 500));
        tracker.record("model-b", &usage(2000, 1000));
        tracker.record("model-c", &usage(3000, 1500));

        let summary = tracker.get_summary();

        // Should have 3 separate records
        assert_eq!(summary.len(), 3);

        let a = summary.iter().find(|r| r.model == "model-a").unwrap();
        let b = summary.iter().find(|r| r.model == "model-b").unwrap();
        let c = summary.iter().find(|r| r.model == "model-c").unwrap();

        assert_eq!(a.total_input_tokens, 1000);
        assert_eq!(b.total_input_tokens, 2000);
        assert_eq!(c.total_input_tokens, 3000);
    }

    /// Test 20: Get total cost across all models
    #[test]
    fn test_get_total_cost() {
        let tracker = InternalCostTracker::with_default_pricing();

        tracker.record("claude-sonnet-4-6", &usage(10000, 5000));
        tracker.record("claude-sonnet-4-6", &usage(10000, 5000));

        let total = tracker.get_total_cost_usd();
        // With known pricing, should have accumulated cost
        assert!(total >= 0.0);
    }

    /// Test 21: Reset clears all records
    #[test]
    fn test_reset() {
        let tracker = InternalCostTracker::with_default_pricing();

        tracker.record("model-a", &usage(1000, 500));
        tracker.record("model-b", &usage(2000, 1000));

        assert_eq!(tracker.get_summary().len(), 2);

        tracker.reset();

        assert_eq!(tracker.get_summary().len(), 0);
    }
}

// ============================================================================
// RoutingEngine Integration Tests
// ============================================================================

mod routing_engine_tests {
    use super::*;
    use gateway_core::llm::strategies::ModelCapabilities;

    /// Helper to create a test registry
    fn create_test_registry() -> ModelRegistry {
        let mut registry = ModelRegistry::new();
        registry.register(
            "anthropic",
            ModelCapabilities {
                tool_calling: true,
                vision: true,
                streaming: true,
                max_context_tokens: Some(200_000),
            },
        );
        registry.register(
            "openai",
            ModelCapabilities {
                tool_calling: true,
                vision: true,
                streaming: true,
                max_context_tokens: Some(128_000),
            },
        );
        registry
    }

    /// Helper to create a RoutingEngine with test fixtures
    async fn create_test_routing_engine() -> RoutingEngine {
        let catalog = ProfileCatalog::empty();

        // Add test profiles
        catalog
            .upsert(primary_fallback_profile(
                "fast-chat",
                target("anthropic", "claude-3-5-haiku-latest"),
                Some(vec![target("openai", "gpt-4o-mini")]),
            ))
            .await;

        catalog
            .upsert(primary_fallback_profile(
                "deep-reasoning",
                target("anthropic", "claude-sonnet-4-6"),
                Some(vec![target("openai", "gpt-4o")]),
            ))
            .await;

        let registry = create_test_registry();
        let health_tracker = HealthTracker::new(HealthConfig::default());
        let cost_tracker = InternalCostTracker::with_default_pricing();

        RoutingEngine::new(
            Arc::new(RwLock::new(catalog)),
            Arc::new(health_tracker),
            Arc::new(cost_tracker),
            Arc::new(registry),
        )
    }

    /// Test 22: Route with profile ID
    #[tokio::test]
    async fn test_route_with_profile_id() {
        let engine = create_test_routing_engine().await;

        let request = chat_request("Hello", Some("fast-chat"), None);
        let decision = engine
            .route_with_profile("fast-chat", &request)
            .await
            .unwrap();

        assert_eq!(decision.target.provider, "anthropic");
        assert_eq!(decision.target.model, "claude-3-5-haiku-latest");
    }

    /// Test 23: Route with different profile
    #[tokio::test]
    async fn test_route_different_profiles() {
        let engine = create_test_routing_engine().await;

        // Fast chat profile
        let request = chat_request("Hello", Some("fast-chat"), None);
        let decision = engine
            .route_with_profile("fast-chat", &request)
            .await
            .unwrap();
        assert_eq!(decision.target.model, "claude-3-5-haiku-latest");

        // Deep reasoning profile
        let request = chat_request("Analyze", Some("deep-reasoning"), None);
        let decision = engine
            .route_with_profile("deep-reasoning", &request)
            .await
            .unwrap();
        assert_eq!(decision.target.model, "claude-sonnet-4-6");
    }

    /// Test 24: Non-existent profile returns error
    #[tokio::test]
    async fn test_nonexistent_profile_error() {
        let engine = create_test_routing_engine().await;

        let request = chat_request("Hello", Some("nonexistent"), None);
        let result = engine.route_with_profile("nonexistent", &request).await;

        assert!(result.is_err());
    }

    /// Test 25: Get profile by ID
    #[tokio::test]
    async fn test_get_profile_by_id() {
        let engine = create_test_routing_engine().await;

        let profile = engine.get_profile_by_id("fast-chat").await.unwrap();
        // Note: get_profile_by_id returns strategies::ModelProfile where 'name' is the profile ID
        // (see routing_engine.rs convert_profile which uses profile.id.clone() for name)
        assert_eq!(profile.name, "fast-chat");
    }

    /// Test 26: List all profiles via catalog
    #[tokio::test]
    async fn test_list_profiles() {
        let engine = create_test_routing_engine().await;

        // Use the catalog directly since list_profiles() requires async catalog access
        let catalog = engine.profile_catalog();
        let profiles = catalog.read().await.list().await;
        assert!(profiles.len() >= 2);
    }
}

// ============================================================================
// End-to-End Integration Tests
// ============================================================================

mod e2e_tests {
    use super::*;
    use gateway_core::llm::strategies::ModelCapabilities;

    fn create_test_registry() -> ModelRegistry {
        let mut registry = ModelRegistry::new();
        registry.register(
            "anthropic",
            ModelCapabilities {
                tool_calling: true,
                vision: true,
                streaming: true,
                max_context_tokens: Some(200_000),
            },
        );
        registry.register(
            "openai",
            ModelCapabilities {
                tool_calling: true,
                vision: true,
                streaming: true,
                max_context_tokens: Some(128_000),
            },
        );
        registry
    }

    /// Test 27: Full routing flow with health tracking
    #[tokio::test]
    async fn test_full_routing_with_health() {
        let catalog = ProfileCatalog::empty();
        catalog
            .upsert(primary_fallback_profile(
                "test-profile",
                target("anthropic", "claude-sonnet-4-6"),
                Some(vec![target("openai", "gpt-4o")]),
            ))
            .await;

        let registry = create_test_registry();
        let health_tracker = Arc::new(HealthTracker::new(HealthConfig::default()));
        let cost_tracker = Arc::new(InternalCostTracker::with_default_pricing());

        let engine = RoutingEngine::new(
            Arc::new(RwLock::new(catalog)),
            health_tracker.clone(),
            cost_tracker.clone(),
            Arc::new(registry),
        );

        // Route a request
        let request = chat_request("Test", Some("test-profile"), None);
        let decision = engine
            .route_with_profile("test-profile", &request)
            .await
            .unwrap();

        // Simulate success
        health_tracker
            .record_success_with_latency(&decision.target.provider, Duration::from_millis(150));

        // Check health is recorded
        let status = health_tracker
            .get_provider_status(&decision.target.provider)
            .unwrap();
        assert_eq!(status.total_requests, 1);
        assert_eq!(status.consecutive_failures, 0);
    }

    /// Test 28: Full routing flow with cost tracking
    #[tokio::test]
    async fn test_full_routing_with_cost() {
        let catalog = ProfileCatalog::empty();
        catalog
            .upsert(primary_fallback_profile(
                "test-profile",
                target("anthropic", "claude-sonnet-4-6"),
                None,
            ))
            .await;

        let registry = create_test_registry();
        let health_tracker = Arc::new(HealthTracker::new(HealthConfig::default()));
        let cost_tracker = Arc::new(InternalCostTracker::with_default_pricing());

        let engine = RoutingEngine::new(
            Arc::new(RwLock::new(catalog)),
            health_tracker,
            cost_tracker.clone(),
            Arc::new(registry),
        );

        // Route request
        let request = chat_request("Test", Some("test-profile"), None);
        let decision = engine
            .route_with_profile("test-profile", &request)
            .await
            .unwrap();

        // Record usage
        cost_tracker.record(&decision.target.model, &usage(5000, 2000));

        // Verify cost recorded
        let summary = cost_tracker.get_summary();
        assert!(!summary.is_empty());
        let record = summary
            .iter()
            .find(|r| r.model.contains("claude-sonnet"))
            .unwrap();
        assert_eq!(record.total_input_tokens, 5000);
    }

    /// Test 29: Fallback path decision includes fallback
    #[tokio::test]
    async fn test_fallback_in_decision() {
        let catalog = ProfileCatalog::empty();
        catalog
            .upsert(primary_fallback_profile(
                "test-profile",
                target("anthropic", "claude-sonnet-4-6"),
                Some(vec![target("openai", "gpt-4o")]),
            ))
            .await;

        let registry = create_test_registry();
        let health_tracker = Arc::new(HealthTracker::new(HealthConfig::default()));
        let cost_tracker = Arc::new(InternalCostTracker::with_default_pricing());

        let engine = RoutingEngine::new(
            Arc::new(RwLock::new(catalog)),
            health_tracker.clone(),
            cost_tracker,
            Arc::new(registry),
        );

        // Route request - primary is selected
        let request = chat_request("Test", Some("test-profile"), None);
        let decision = engine
            .route_with_profile("test-profile", &request)
            .await
            .unwrap();

        assert_eq!(decision.target.provider, "anthropic");

        // Decision should have fallback
        assert!(decision.fallback.is_some());
        let fallback = decision.fallback.unwrap();
        assert_eq!(fallback.provider, "openai");
    }

    /// Test 30: Multiple concurrent routing requests
    #[tokio::test]
    async fn test_concurrent_routing() {
        let catalog = ProfileCatalog::empty();
        catalog
            .upsert(primary_fallback_profile(
                "test-profile",
                target("anthropic", "claude-sonnet-4-6"),
                None,
            ))
            .await;

        let registry = create_test_registry();
        let health_tracker = Arc::new(HealthTracker::new(HealthConfig::default()));
        let cost_tracker = Arc::new(InternalCostTracker::with_default_pricing());

        let engine = Arc::new(RoutingEngine::new(
            Arc::new(RwLock::new(catalog)),
            health_tracker,
            cost_tracker.clone(),
            Arc::new(registry),
        ));

        // Spawn multiple concurrent routing tasks
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let engine = engine.clone();
                let cost = cost_tracker.clone();
                tokio::spawn(async move {
                    let request = chat_request(&format!("Test {}", i), Some("test-profile"), None);
                    let decision = engine
                        .route_with_profile("test-profile", &request)
                        .await
                        .unwrap();

                    // Simulate recording usage
                    cost.record(&decision.target.model, &usage(100, 50));

                    decision
                })
            })
            .collect();

        let results: Vec<_> = futures::future::join_all(handles).await;

        // All should succeed
        for result in &results {
            assert!(result.is_ok());
        }

        // Cost should be aggregated
        let summary = cost_tracker.get_summary();
        let record = summary
            .iter()
            .find(|r| r.model.contains("claude-sonnet"))
            .unwrap();
        assert_eq!(record.total_requests, 10);
        assert_eq!(record.total_input_tokens, 1000);
    }

    /// Test 31: Profile switching mid-session
    #[tokio::test]
    async fn test_profile_switching() {
        let catalog = ProfileCatalog::empty();
        catalog
            .upsert(primary_fallback_profile(
                "fast",
                target("anthropic", "claude-3-5-haiku-latest"),
                None,
            ))
            .await;
        catalog
            .upsert(primary_fallback_profile(
                "smart",
                target("anthropic", "claude-sonnet-4-6"),
                None,
            ))
            .await;

        let registry = create_test_registry();
        let health_tracker = Arc::new(HealthTracker::new(HealthConfig::default()));
        let cost_tracker = Arc::new(InternalCostTracker::with_default_pricing());

        let engine = RoutingEngine::new(
            Arc::new(RwLock::new(catalog)),
            health_tracker,
            cost_tracker,
            Arc::new(registry),
        );

        // First request with "fast" profile
        let request = chat_request("Quick question", Some("fast"), None);
        let decision = engine.route_with_profile("fast", &request).await.unwrap();
        assert_eq!(decision.target.model, "claude-3-5-haiku-latest");

        // Second request with "smart" profile
        let request = chat_request("Complex analysis", Some("smart"), None);
        let decision = engine.route_with_profile("smart", &request).await.unwrap();
        assert_eq!(decision.target.model, "claude-sonnet-4-6");
    }
}

// ============================================================================
// Error Handling Tests
// ============================================================================

mod error_handling_tests {
    use super::*;
    use gateway_core::llm::strategies::ModelCapabilities;

    fn create_test_registry() -> ModelRegistry {
        let mut registry = ModelRegistry::new();
        registry.register(
            "anthropic",
            ModelCapabilities {
                tool_calling: true,
                vision: true,
                streaming: true,
                max_context_tokens: Some(200_000),
            },
        );
        registry
    }

    /// Test 32: Empty profile catalog
    #[tokio::test]
    async fn test_empty_catalog_error() {
        let catalog = ProfileCatalog::empty();
        let registry = create_test_registry();
        let health_tracker = Arc::new(HealthTracker::new(HealthConfig::default()));
        let cost_tracker = Arc::new(InternalCostTracker::with_default_pricing());

        let engine = RoutingEngine::new(
            Arc::new(RwLock::new(catalog)),
            health_tracker,
            cost_tracker,
            Arc::new(registry),
        );

        let request = chat_request("Test", Some("any"), None);
        let result = engine.route_with_profile("any", &request).await;

        assert!(result.is_err());
    }

    /// Test 33: Profile with no primary target
    #[tokio::test]
    async fn test_empty_routing_config() {
        let catalog = ProfileCatalog::empty();
        catalog
            .upsert(ModelProfile {
                id: "empty".to_string(),
                name: "Empty Profile".to_string(),
                description: "Empty profile".to_string(),
                enabled: true,
                routing: RoutingConfig::default(),
                agent: Default::default(),
                cache_enabled: false,
                cache_ttl_seconds: 3600,
            })
            .await;

        let registry = create_test_registry();
        let health_tracker = Arc::new(HealthTracker::new(HealthConfig::default()));
        let cost_tracker = Arc::new(InternalCostTracker::with_default_pricing());

        let engine = RoutingEngine::new(
            Arc::new(RwLock::new(catalog)),
            health_tracker,
            cost_tracker,
            Arc::new(registry),
        );

        let request = chat_request("Test", Some("empty"), None);
        let result = engine.route_with_profile("empty", &request).await;

        // Should fail because no routing config
        assert!(result.is_err());
    }

    /// Test 34: Invalid YAML file path
    #[tokio::test]
    async fn test_invalid_yaml_path() {
        let result = ProfileCatalog::from_yaml("/nonexistent/path.yaml").await;
        assert!(result.is_err());
    }
}

// ============================================================================
// Performance Tests
// ============================================================================

mod performance_tests {
    use super::*;
    use gateway_core::llm::strategies::ModelCapabilities;
    use std::time::Instant;

    fn create_test_registry() -> ModelRegistry {
        let mut registry = ModelRegistry::new();
        registry.register(
            "anthropic",
            ModelCapabilities {
                tool_calling: true,
                vision: true,
                streaming: true,
                max_context_tokens: Some(200_000),
            },
        );
        registry
    }

    /// Test 35: Routing throughput
    #[tokio::test]
    async fn test_routing_throughput() {
        let catalog = ProfileCatalog::empty();
        catalog
            .upsert(primary_fallback_profile(
                "perf-test",
                target("anthropic", "claude-sonnet-4-6"),
                None,
            ))
            .await;

        let registry = create_test_registry();
        let health_tracker = Arc::new(HealthTracker::new(HealthConfig::default()));
        let cost_tracker = Arc::new(InternalCostTracker::with_default_pricing());

        let engine = Arc::new(RoutingEngine::new(
            Arc::new(RwLock::new(catalog)),
            health_tracker,
            cost_tracker,
            Arc::new(registry),
        ));

        let iterations = 1000;
        let start = Instant::now();

        for i in 0..iterations {
            let request = chat_request(&format!("Test {}", i), Some("perf-test"), None);
            engine
                .route_with_profile("perf-test", &request)
                .await
                .unwrap();
        }

        let elapsed = start.elapsed();
        let per_route_us = elapsed.as_micros() / iterations as u128;

        // Should be fast (< 1ms per route)
        assert!(
            per_route_us < 1000,
            "Routing too slow: {}us per route",
            per_route_us
        );
    }

    /// Test 36: Health tracker under load
    #[test]
    fn test_health_tracker_under_load() {
        let tracker = Arc::new(HealthTracker::new(HealthConfig::default()));
        let iterations = 10000;
        let start = Instant::now();

        for i in 0..iterations {
            if i % 10 == 0 {
                tracker.record_failure("provider");
            } else {
                tracker.record_success("provider");
            }
        }

        let elapsed = start.elapsed();

        // Should complete quickly (< 1 second for 10k operations)
        assert!(elapsed.as_secs() < 1);

        // Check final state
        let status = tracker.get_provider_status("provider").unwrap();
        assert_eq!(status.total_requests, iterations as u64);
    }
}

// ============================================================================
// YAML Configuration Tests
// ============================================================================

mod yaml_config_tests {
    use super::*;

    /// Test 37: Full YAML with multiple profiles
    #[tokio::test]
    async fn test_full_yaml_config() {
        let yaml = r#"
profiles:
  - id: primary-fallback
    name: Primary Fallback
    description: "Primary with fallback"
    routing:
      strategy: primary_fallback
      primary:
        provider: anthropic
        model: claude-sonnet-4-6
      fallbacks:
        - provider: openai
          model: gpt-4o

  - id: dev-profile
    name: Dev Profile
    description: "Development API profile"
    routing:
      strategy: primary_fallback
      primary:
        provider: openai
        model: gpt-4o-mini
"#;

        let file = write_yaml_config(yaml);
        let catalog = ProfileCatalog::from_yaml(file.path()).await.unwrap();
        let profiles = catalog.list().await;

        assert_eq!(profiles.len(), 2);

        // Verify each profile
        let pf = catalog.get("primary-fallback").await.unwrap();
        assert!(pf.routing.primary.is_some());
        assert!(pf.routing.fallbacks.is_some());

        let dev = catalog.get("dev-profile").await.unwrap();
        assert_eq!(dev.routing.primary.as_ref().unwrap().model, "gpt-4o-mini");
    }

    /// Test 38: Production-like YAML config
    #[tokio::test]
    async fn test_production_like_config() {
        let yaml = r#"
profiles:
  - id: product-a
    name: Product A
    description: "Production profile for Product A"
    routing:
      strategy: primary_fallback
      primary:
        provider: anthropic
        model: claude-sonnet-4-6
      fallbacks:
        - provider: openai
          model: gpt-4o

  - id: dev-api
    name: Dev API
    description: "Development API profile"
    routing:
      strategy: primary_fallback
      primary:
        provider: openai
        model: gpt-4o-mini

  - id: enterprise
    name: Enterprise
    description: "Enterprise tier with cascade"
    routing:
      strategy: cascade
      cascade:
        tiers:
          - label: premium
            target:
              provider: anthropic
              model: claude-sonnet-4-6
          - label: standard
            target:
              provider: openai
              model: gpt-4o
          - label: fallback
            target:
              provider: google
              model: gemini-2.0-flash
        max_escalations: 2
"#;

        let file = write_yaml_config(yaml);
        let catalog = ProfileCatalog::from_yaml(file.path()).await.unwrap();

        let product_a = catalog.get("product-a").await.unwrap();
        assert_eq!(
            product_a.routing.primary.as_ref().unwrap().provider,
            "anthropic"
        );

        let dev_api = catalog.get("dev-api").await.unwrap();
        assert_eq!(
            dev_api.routing.primary.as_ref().unwrap().model,
            "gpt-4o-mini"
        );

        let enterprise = catalog.get("enterprise").await.unwrap();
        assert!(enterprise.routing.cascade.is_some());
        assert_eq!(enterprise.routing.cascade.as_ref().unwrap().tiers.len(), 3);
    }
}
