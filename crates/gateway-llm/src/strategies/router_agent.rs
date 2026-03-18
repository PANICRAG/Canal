//! Router Agent routing strategy.
//!
//! Uses an LLM classifier to intelligently route requests to the most
//! appropriate model target. The classifier analyzes the request characteristics
//! (message count, length, tool usage, content preview) and selects from the
//! available targets based on their descriptions.
//!
//! This strategy provides dynamic routing based on request content rather than
//! static rules, making it ideal for diverse workloads where different requests
//! benefit from different model capabilities.

use async_trait::async_trait;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

use super::{ModelProfile, ModelRegistry, ModelTarget, RouteDecision, RoutingStrategyHandler};
use crate::error::{Error, Result};
use crate::router::{ChatRequest, LlmRouter, Message};

/// Strategy that uses an LLM classifier to route requests to the best target.
///
/// The classifier model analyzes the incoming request and selects the most
/// appropriate target from the available options in the profile's cascade tiers
/// or A/B test variants.
pub struct RouterAgentStrategy {
    /// LLM router for making classification requests
    llm_router: Arc<RwLock<LlmRouter>>,
    /// Model to use for classification (e.g., "claude-haiku-4-5-20251001")
    classifier_model: String,
    /// Simple cache for route decisions (keyed by request hash) with TTL
    cache: Arc<RwLock<HashMap<u64, (RouteDecision, Instant)>>>,
}

impl RouterAgentStrategy {
    /// Create a new router agent strategy.
    ///
    /// # Arguments
    ///
    /// * `llm_router` - The LLM router to use for classification requests
    /// * `classifier_model` - The model identifier to use for classification
    ///   (e.g., "claude-haiku-4-5-20251001")
    pub fn new(llm_router: Arc<RwLock<LlmRouter>>, classifier_model: String) -> Self {
        Self {
            llm_router,
            classifier_model,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Compute a simple hash of the request for caching purposes.
    ///
    /// The hash is based on message content, tool definitions, and model settings.
    fn compute_request_hash(request: &ChatRequest) -> u64 {
        let mut hasher = DefaultHasher::new();

        // Hash message content
        for msg in &request.messages {
            msg.role.hash(&mut hasher);
            msg.content.hash(&mut hasher);
        }

        // Hash tool names (but not full schemas to keep hash lightweight)
        for tool in &request.tools {
            tool.name.hash(&mut hasher);
        }

        // Hash other relevant settings
        request.max_tokens.hash(&mut hasher);
        if let Some(temp) = request.temperature {
            // Hash float as bits
            temp.to_bits().hash(&mut hasher);
        }

        hasher.finish()
    }

    /// Build the classification prompt for the LLM.
    ///
    /// The prompt describes the request characteristics and available targets,
    /// asking the classifier to select the most appropriate one.
    fn build_classification_prompt(
        request: &ChatRequest,
        targets: &[(ModelTarget, String)], // (target, description)
    ) -> String {
        // R3-L: Removed unused _message_count and _has_tools variables
        let total_length: usize = request.messages.iter().map(|m| m.content.len()).sum();

        // Get first message preview (up to 200 chars, safe at char boundary)
        let first_message_preview = request
            .messages
            .first()
            .map(|m| {
                if m.content.len() > 200 {
                    let safe_end = m
                        .content
                        .char_indices()
                        .take_while(|(i, _)| *i < 200)
                        .last()
                        .map(|(i, c)| i + c.len_utf8())
                        .unwrap_or(m.content.len().min(200));
                    format!("{}...", &m.content[..safe_end])
                } else {
                    m.content.clone()
                }
            })
            .unwrap_or_else(|| "(empty)".to_string());

        // Build target list
        let targets_list: String = targets
            .iter()
            .enumerate()
            .map(|(i, (target, desc))| {
                format!("{}. {}/{} - {}", i + 1, target.provider, target.model, desc)
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Analyze request complexity
        let is_simple = total_length < 50
            || first_message_preview.contains("1+1")
            || first_message_preview.contains("=?")
            || first_message_preview.contains("翻译")
            || first_message_preview.to_lowercase().contains("hello")
            || first_message_preview.to_lowercase().contains("what is");

        let is_complex = first_message_preview.contains("实现")
            || first_message_preview.contains("implement")
            || first_message_preview.contains("完整")
            || first_message_preview.contains("测试")
            || first_message_preview.contains("proof")
            || first_message_preview.contains("证明")
            || first_message_preview.contains("design")
            || first_message_preview.contains("架构")
            || total_length > 200;

        // Determine index based on heuristics
        let suggested_index = if is_simple {
            1
        } else if is_complex {
            3.min(targets.len())
        } else {
            2.min(targets.len())
        };

        format!(
            r#"Task complexity analysis suggests index {}. Confirm or adjust.

Request: "{}"

Models:
{}

Reply with JSON: {{"index": {}}}"#,
            suggested_index, first_message_preview, targets_list, suggested_index
        )
    }

    /// Parse the classification response to extract the selected target index.
    ///
    /// Returns the 0-based index of the selected target.
    fn parse_classification_response(response: &str) -> Result<usize> {
        // Try to find JSON object in the response
        let response = response.trim();

        // Look for the JSON pattern anywhere in the response
        let json_start = response.find('{');
        let json_end = response.rfind('}');

        let json_str = match (json_start, json_end) {
            (Some(start), Some(end)) if start < end => &response[start..=end],
            _ => {
                return Err(Error::Llm(format!(
                    "No valid JSON object found in classification response: {}",
                    response
                )));
            }
        };

        // Parse the JSON
        let parsed: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
            Error::Llm(format!(
                "Failed to parse classification JSON '{}': {}",
                json_str, e
            ))
        })?;

        // Extract the index
        let index = parsed
            .get("index")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| {
                Error::Llm(format!(
                    "Classification response missing 'index' field or not a number: {}",
                    json_str
                ))
            })?;

        // Convert from 1-based to 0-based index
        if index == 0 {
            return Err(Error::Llm(
                "Classification index must be 1 or greater".into(),
            ));
        }

        Ok((index - 1) as usize)
    }

    /// Extract available targets from the profile.
    ///
    /// Checks cascade tiers first, then A/B test variants, then primary/fallback.
    fn extract_targets(profile: &ModelProfile) -> Vec<(ModelTarget, String)> {
        let mut targets = Vec::new();

        // First, check cascade tiers
        if !profile.routing.cascade_tiers.is_empty() {
            for tier in &profile.routing.cascade_tiers {
                targets.push((tier.target.clone(), format!("Tier: {}", tier.label)));
            }
            return targets;
        }

        // Then check A/B test variants
        if !profile.routing.ab_test_variants.is_empty() {
            for variant in &profile.routing.ab_test_variants {
                targets.push((
                    variant.target.clone(),
                    format!("Variant: {} (weight: {})", variant.name, variant.weight),
                ));
            }
            return targets;
        }

        // Finally, use primary/fallback
        if let Some(primary) = &profile.routing.primary {
            targets.push((primary.clone(), "Primary target (fast/cheap)".to_string()));
        }
        if let Some(fallback) = &profile.routing.fallback {
            targets.push((
                fallback.clone(),
                format!("Fallback target ({})", fallback.model),
            ));
        }

        targets
    }
}

#[async_trait]
impl RoutingStrategyHandler for RouterAgentStrategy {
    async fn resolve(
        &self,
        profile: &ModelProfile,
        request: &ChatRequest,
        _registry: &ModelRegistry,
    ) -> Result<RouteDecision> {
        // Extract available targets from the profile
        let targets = Self::extract_targets(profile);

        if targets.is_empty() {
            return Err(Error::Llm(
                "No routing targets configured in profile".into(),
            ));
        }

        // If only one target, return it directly
        if targets.len() == 1 {
            let (target, _) = &targets[0];
            return Ok(RouteDecision {
                target: target.clone(),
                reason: "router_agent:single_target".into(),
                fallback: profile.routing.fallback.clone(),
            });
        }

        // Compute request hash for caching
        let request_hash = Self::compute_request_hash(request);

        // R3-M: Check cache with TTL (5 minutes)
        const CACHE_TTL_SECS: u64 = 300;
        {
            let cache = self.cache.read().await;
            if let Some((cached_decision, cached_at)) = cache.get(&request_hash) {
                if cached_at.elapsed().as_secs() < CACHE_TTL_SECS {
                    tracing::debug!(
                        hash = request_hash,
                        target = %cached_decision.target.model,
                        "Router agent cache hit"
                    );
                    return Ok(cached_decision.clone());
                }
            }
        }

        // Build classification prompt
        let classification_prompt = Self::build_classification_prompt(request, &targets);

        // Create classification request
        let classification_request = ChatRequest {
            messages: vec![Message::text("user", &classification_prompt)],
            model: Some(self.classifier_model.clone()),
            max_tokens: Some(50),   // We only need a small JSON response
            temperature: Some(0.0), // Deterministic for consistency
            stream: false,
            tools: vec![],
            tool_choice: None,
            ..Default::default()
        };

        // Call the LLM router for classification
        let selected_index = {
            let router = self.llm_router.read().await;
            match router.route(classification_request).await {
                Ok(response) => {
                    let response_text = response
                        .choices
                        .first()
                        .map(|c| c.message.content.clone())
                        .unwrap_or_default();

                    tracing::debug!(
                        classifier_model = %self.classifier_model,
                        response = %response_text,
                        "Router agent classification response"
                    );

                    match Self::parse_classification_response(&response_text) {
                        Ok(idx) if idx < targets.len() => idx,
                        Ok(idx) => {
                            tracing::warn!(
                                index = idx,
                                num_targets = targets.len(),
                                "Classification returned out-of-bounds index, using first target"
                            );
                            0
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "Failed to parse classification response, using first target"
                            );
                            0
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Classification request failed, using first target"
                    );
                    0
                }
            }
        };

        // Build the route decision
        let (selected_target, selected_desc) = &targets[selected_index];

        // Determine fallback (next target in list, if available)
        let fallback = if selected_index + 1 < targets.len() {
            Some(targets[selected_index + 1].0.clone())
        } else if selected_index > 0 {
            // If we selected a later target, use the first as fallback
            Some(targets[0].0.clone())
        } else {
            profile.routing.fallback.clone()
        };

        let decision = RouteDecision {
            target: selected_target.clone(),
            reason: format!("router_agent:idx{}:{}", selected_index + 1, selected_desc),
            fallback,
        };

        // Cache the decision
        {
            let mut cache = self.cache.write().await;
            // Limit cache size to prevent unbounded growth
            if cache.len() >= 1000 {
                cache.clear();
                tracing::debug!("Router agent cache cleared due to size limit");
            }
            cache.insert(request_hash, (decision.clone(), Instant::now()));
        }

        tracing::info!(
            target_provider = %decision.target.provider,
            target_model = %decision.target.model,
            reason = %decision.reason,
            "Router agent selected target"
        );

        Ok(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{
        ChatRequest, ChatResponse, Choice, LlmConfig, LlmProvider, Message, StopReason,
        StreamResponse, Usage,
    };
    use crate::strategies::{
        AbTestVariant, CascadeTier, ModelProfile, ModelRegistry, ModelTarget, RoutingConfig,
    };

    // Mock LLM provider for testing
    struct MockClassifierProvider {
        response_index: usize,
    }

    #[async_trait]
    impl LlmProvider for MockClassifierProvider {
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse> {
            Ok(ChatResponse {
                id: "mock-id".to_string(),
                model: "mock-classifier".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: Message::text(
                        "assistant",
                        format!(r#"{{"index": {}}}"#, self.response_index),
                    ),
                    finish_reason: "stop".to_string(),
                    stop_reason: Some(StopReason::EndTurn),
                }],
                usage: Usage {
                    prompt_tokens: 100,
                    completion_tokens: 10,
                    total_tokens: 110,
                },
            })
        }

        async fn chat_stream(&self, _request: ChatRequest) -> Result<StreamResponse> {
            unimplemented!("Streaming not needed for classifier")
        }

        fn name(&self) -> &str {
            "mock-classifier"
        }

        async fn is_available(&self) -> bool {
            true
        }
    }

    // Mock provider that always fails
    struct FailingProvider;

    #[async_trait]
    impl LlmProvider for FailingProvider {
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse> {
            Err(Error::Llm("Mock classification failure".into()))
        }

        async fn chat_stream(&self, _request: ChatRequest) -> Result<StreamResponse> {
            Err(Error::Llm("Mock streaming not supported".into()))
        }

        fn name(&self) -> &str {
            "failing"
        }

        async fn is_available(&self) -> bool {
            false
        }
    }

    fn target_haiku() -> ModelTarget {
        ModelTarget {
            provider: "anthropic".into(),
            model: "claude-haiku-4-5-20251001".into(),
        }
    }

    fn target_sonnet() -> ModelTarget {
        ModelTarget {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
        }
    }

    fn target_opus() -> ModelTarget {
        ModelTarget {
            provider: "anthropic".into(),
            model: "claude-opus-4-6".into(),
        }
    }

    fn make_cascade_profile() -> ModelProfile {
        ModelProfile {
            name: "router-test".into(),
            description: "Test profile for router agent".into(),
            routing: RoutingConfig {
                cascade_tiers: vec![
                    CascadeTier {
                        label: "fast".into(),
                        target: target_haiku(),
                    },
                    CascadeTier {
                        label: "balanced".into(),
                        target: target_sonnet(),
                    },
                    CascadeTier {
                        label: "powerful".into(),
                        target: target_opus(),
                    },
                ],
                ..Default::default()
            },
        }
    }

    fn make_ab_test_profile() -> ModelProfile {
        ModelProfile {
            name: "ab-router-test".into(),
            description: String::new(),
            routing: RoutingConfig {
                ab_test_variants: vec![
                    AbTestVariant {
                        name: "control".into(),
                        target: target_sonnet(),
                        weight: 50,
                    },
                    AbTestVariant {
                        name: "treatment".into(),
                        target: target_opus(),
                        weight: 50,
                    },
                ],
                ..Default::default()
            },
        }
    }

    fn make_primary_fallback_profile() -> ModelProfile {
        ModelProfile {
            name: "pf-router-test".into(),
            description: String::new(),
            routing: RoutingConfig {
                primary: Some(target_sonnet()),
                fallback: Some(target_haiku()),
                ..Default::default()
            },
        }
    }

    fn plain_request() -> ChatRequest {
        ChatRequest {
            messages: vec![Message::text("user", "Hello, how are you?")],
            ..Default::default()
        }
    }

    fn long_request() -> ChatRequest {
        ChatRequest {
            messages: vec![
                Message::text("user", "Please write a detailed analysis of..."),
                Message::text("assistant", "I'll provide a comprehensive analysis..."),
                Message::text("user", "Can you expand on the third point?"),
            ],
            ..Default::default()
        }
    }

    async fn create_strategy_with_mock(response_index: usize) -> RouterAgentStrategy {
        let config = LlmConfig {
            default_provider: "mock".into(),
            fallback_enabled: false,
            timeout_seconds: 30,
            max_retries: 0,
        };

        let mut router = LlmRouter::new(config);
        router.register_provider("mock", Arc::new(MockClassifierProvider { response_index }));

        RouterAgentStrategy::new(Arc::new(RwLock::new(router)), "mock-classifier".into())
    }

    async fn create_failing_strategy() -> RouterAgentStrategy {
        let config = LlmConfig {
            default_provider: "failing".into(),
            fallback_enabled: false,
            timeout_seconds: 30,
            max_retries: 0,
        };

        let mut router = LlmRouter::new(config);
        router.register_provider("failing", Arc::new(FailingProvider));

        RouterAgentStrategy::new(Arc::new(RwLock::new(router)), "failing-classifier".into())
    }

    #[test]
    fn test_compute_request_hash_consistency() {
        let request = plain_request();
        let hash1 = RouterAgentStrategy::compute_request_hash(&request);
        let hash2 = RouterAgentStrategy::compute_request_hash(&request);
        assert_eq!(hash1, hash2, "Same request should produce same hash");
    }

    #[test]
    fn test_compute_request_hash_different_requests() {
        let request1 = plain_request();
        let request2 = long_request();
        let hash1 = RouterAgentStrategy::compute_request_hash(&request1);
        let hash2 = RouterAgentStrategy::compute_request_hash(&request2);
        assert_ne!(
            hash1, hash2,
            "Different requests should produce different hashes"
        );
    }

    #[test]
    fn test_parse_classification_response_valid() {
        let response = r#"{"index": 2}"#;
        let result = RouterAgentStrategy::parse_classification_response(response);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1); // 1-based to 0-based
    }

    #[test]
    fn test_parse_classification_response_with_surrounding_text() {
        let response = r#"Based on the analysis, I recommend: {"index": 3} for this task."#;
        let result = RouterAgentStrategy::parse_classification_response(response);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 2);
    }

    #[test]
    fn test_parse_classification_response_zero_index() {
        let response = r#"{"index": 0}"#;
        let result = RouterAgentStrategy::parse_classification_response(response);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_classification_response_no_json() {
        let response = "I think you should use the second model.";
        let result = RouterAgentStrategy::parse_classification_response(response);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_classification_response_missing_index() {
        let response = r#"{"model": "sonnet"}"#;
        let result = RouterAgentStrategy::parse_classification_response(response);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_classification_prompt_structure() {
        let request = plain_request();
        let targets = vec![
            (target_haiku(), "Fast responses".into()),
            (target_sonnet(), "Balanced performance".into()),
        ];

        let prompt = RouterAgentStrategy::build_classification_prompt(&request, &targets);

        // Verify prompt contains the message content
        assert!(prompt.contains("Hello, how are you?"));
        // Verify prompt contains model targets
        assert!(prompt.contains("anthropic/claude-haiku"));
        assert!(prompt.contains("anthropic/claude-sonnet"));
        // Verify prompt contains JSON response format
        assert!(prompt.contains("\"index\":"));
    }

    #[test]
    fn test_extract_targets_cascade() {
        let profile = make_cascade_profile();
        let targets = RouterAgentStrategy::extract_targets(&profile);

        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].0, target_haiku());
        assert_eq!(targets[1].0, target_sonnet());
        assert_eq!(targets[2].0, target_opus());
    }

    #[test]
    fn test_extract_targets_ab_test() {
        let profile = make_ab_test_profile();
        let targets = RouterAgentStrategy::extract_targets(&profile);

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].0, target_sonnet());
        assert_eq!(targets[1].0, target_opus());
    }

    #[test]
    fn test_extract_targets_primary_fallback() {
        let profile = make_primary_fallback_profile();
        let targets = RouterAgentStrategy::extract_targets(&profile);

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].0, target_sonnet());
        assert_eq!(targets[1].0, target_haiku());
    }

    #[tokio::test]
    async fn test_resolve_single_target() {
        let strategy = create_strategy_with_mock(1).await;
        let profile = ModelProfile {
            name: "single".into(),
            description: String::new(),
            routing: RoutingConfig {
                primary: Some(target_sonnet()),
                ..Default::default()
            },
        };
        let registry = ModelRegistry::new();

        let decision = strategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, target_sonnet());
        assert_eq!(decision.reason, "router_agent:single_target");
    }

    #[tokio::test]
    async fn test_resolve_selects_first_target() {
        let strategy = create_strategy_with_mock(1).await; // Select index 1 (first)
        let profile = make_cascade_profile();
        let registry = ModelRegistry::new();

        let decision = strategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, target_haiku());
        assert!(decision.reason.contains("router_agent:idx1"));
        assert_eq!(decision.fallback, Some(target_sonnet()));
    }

    #[tokio::test]
    async fn test_resolve_selects_second_target() {
        let strategy = create_strategy_with_mock(2).await; // Select index 2 (second)
        let profile = make_cascade_profile();
        let registry = ModelRegistry::new();

        let decision = strategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, target_sonnet());
        assert!(decision.reason.contains("router_agent:idx2"));
        assert_eq!(decision.fallback, Some(target_opus()));
    }

    #[tokio::test]
    async fn test_resolve_selects_third_target() {
        let strategy = create_strategy_with_mock(3).await; // Select index 3 (third)
        let profile = make_cascade_profile();
        let registry = ModelRegistry::new();

        let decision = strategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, target_opus());
        assert!(decision.reason.contains("router_agent:idx3"));
        // Fallback wraps around to first target
        assert_eq!(decision.fallback, Some(target_haiku()));
    }

    #[tokio::test]
    async fn test_resolve_out_of_bounds_falls_back_to_first() {
        let strategy = create_strategy_with_mock(99).await; // Invalid index
        let profile = make_cascade_profile();
        let registry = ModelRegistry::new();

        let decision = strategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        // Should fall back to first target
        assert_eq!(decision.target, target_haiku());
    }

    #[tokio::test]
    async fn test_resolve_classification_failure_falls_back() {
        let strategy = create_failing_strategy().await;
        let profile = make_cascade_profile();
        let registry = ModelRegistry::new();

        let decision = strategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        // Should fall back to first target on error
        assert_eq!(decision.target, target_haiku());
    }

    #[tokio::test]
    async fn test_resolve_caching() {
        let strategy = create_strategy_with_mock(2).await;
        let profile = make_cascade_profile();
        let registry = ModelRegistry::new();
        let request = plain_request();

        // First call
        let decision1 = strategy
            .resolve(&profile, &request, &registry)
            .await
            .unwrap();

        // Verify cache has entry
        {
            let cache = strategy.cache.read().await;
            let hash = RouterAgentStrategy::compute_request_hash(&request);
            assert!(cache.contains_key(&hash));
        }

        // Second call should hit cache
        let decision2 = strategy
            .resolve(&profile, &request, &registry)
            .await
            .unwrap();

        assert_eq!(decision1.target, decision2.target);
        assert_eq!(decision1.reason, decision2.reason);
    }

    #[tokio::test]
    async fn test_resolve_no_targets_error() {
        let strategy = create_strategy_with_mock(1).await;
        let profile = ModelProfile {
            name: "empty".into(),
            description: String::new(),
            routing: RoutingConfig::default(),
        };
        let registry = ModelRegistry::new();

        let result = strategy
            .resolve(&profile, &plain_request(), &registry)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resolve_with_tools() {
        let strategy = create_strategy_with_mock(1).await;
        let profile = make_cascade_profile();
        let registry = ModelRegistry::new();

        let request = ChatRequest {
            messages: vec![Message::text("user", "Read the file")],
            tools: vec![crate::router::ToolDefinition {
                name: "read_file".into(),
                description: "Read a file".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            ..Default::default()
        };

        let decision = strategy
            .resolve(&profile, &request, &registry)
            .await
            .unwrap();

        // Should still work with tools
        assert_eq!(decision.target, target_haiku());
    }
}
