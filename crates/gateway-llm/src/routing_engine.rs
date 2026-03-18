//! Routing Engine for the LLM Model Router.
//!
//! The `RoutingEngine` orchestrates all routing strategies and manages the
//! lifecycle of routing decisions. It integrates with the profile catalog,
//! health tracker, cost tracker, and model registry to provide intelligent
//! routing with automatic fallback handling.
//!
//! ## Overview
//!
//! ```text
//!   ┌──────────────────────────────────────────────────────────────────────┐
//!   │  RoutingEngine                                                        │
//!   │                                                                       │
//!   │  ┌─────────────────┐  ┌───────────────┐  ┌──────────────────────────┐│
//!   │  │ ProfileCatalog  │  │ HealthTracker │  │ InternalCostTracker      ││
//!   │  │ (model profiles)│  │ (circuit brk) │  │ (usage tracking)         ││
//!   │  └────────┬────────┘  └───────┬───────┘  └─────────────────────────-┘│
//!   │           │                   │                                      │
//!   │           ▼                   ▼                                      │
//!   │  ┌────────────────────────────────────────────────────────────────┐ │
//!   │  │                    Strategy Selection                          │ │
//!   │  │  cascade_tiers → CascadeStrategy                               │ │
//!   │  │  ab_test_variants → AbTestStrategy                             │ │
//!   │  │  task_type_rules → TaskTypeRulesStrategy                       │ │
//!   │  │  otherwise → PrimaryFallbackStrategy                           │ │
//!   │  └────────────────────────────────────────────────────────────────┘ │
//!   │                                                                       │
//!   │  ┌────────────────────────────────────────────────────────────────┐ │
//!   │  │                    ModelRegistry                               │ │
//!   │  │  (provider capabilities lookup)                                │ │
//!   │  └────────────────────────────────────────────────────────────────┘ │
//!   └──────────────────────────────────────────────────────────────────────┘
//! ```

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::cost_tracker::InternalCostTracker;
use crate::error::Result;
use crate::health::HealthTracker;
use crate::model_profile::ProfileCatalog;
use crate::router::{ChatRequest, LlmRouter};
use crate::strategies::{
    ab_test::AbTestStrategy, cascade::CascadeStrategy, primary_fallback::PrimaryFallbackStrategy,
    router_agent::RouterAgentStrategy, task_type_rules::TaskTypeRulesStrategy, ModelProfile,
    ModelRegistry, ModelTarget, MultimodalRoutingConfig, RouteDecision, RoutingStrategyHandler,
};

// ============================================================================
// Inline Multimodal Strategy
// ============================================================================

/// Inline multimodal routing strategy for the routing engine.
///
/// Detects content modality by inspecting message content blocks and
/// routes to the appropriate target (text, vision, or hybrid).
struct InlineMultimodalStrategy {
    config: MultimodalRoutingConfig,
}

#[async_trait::async_trait]
impl RoutingStrategyHandler for InlineMultimodalStrategy {
    async fn resolve(
        &self,
        _profile: &ModelProfile,
        request: &ChatRequest,
        _registry: &ModelRegistry,
    ) -> Result<RouteDecision> {
        use crate::router::ContentBlock;

        let has_image = request.messages.iter().any(|m| {
            m.content_blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. }))
        });

        let has_text = request.messages.iter().any(|m| {
            !m.content.is_empty()
                || m.content_blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { .. }))
        });

        let (target, fallback, reason) = if has_image && has_text {
            (
                self.config.hybrid_target.clone(),
                Some(self.config.vision_target.clone()),
                "multimodal:hybrid",
            )
        } else if has_image {
            (
                self.config.vision_target.clone(),
                Some(self.config.hybrid_target.clone()),
                "multimodal:vision_only",
            )
        } else {
            (
                self.config.text_target.clone(),
                Some(self.config.hybrid_target.clone()),
                "multimodal:text_only",
            )
        };

        Ok(RouteDecision {
            target,
            reason: reason.into(),
            fallback,
        })
    }
}

// ============================================================================
// RoutingEngine
// ============================================================================

/// The main routing engine that orchestrates all routing strategies.
///
/// This struct coordinates profile resolution, strategy selection, health
/// checking, and fallback handling to produce optimal routing decisions.
pub struct RoutingEngine {
    /// The catalog of model profiles.
    profile_catalog: Arc<RwLock<ProfileCatalog>>,
    /// Health tracker for circuit breaker functionality.
    health_tracker: Arc<HealthTracker>,
    /// Cost tracker for usage monitoring.
    cost_tracker: Arc<InternalCostTracker>,
    /// Registry of provider capabilities.
    registry: Arc<ModelRegistry>,
    /// Optional LLM router for RouterAgentStrategy (dynamic AI routing).
    llm_router: Option<Arc<RwLock<LlmRouter>>>,
    /// Default classifier model for RouterAgentStrategy.
    classifier_model: String,
}

impl RoutingEngine {
    /// Create a new `RoutingEngine` with the provided dependencies.
    ///
    /// # Arguments
    ///
    /// * `profile_catalog` - The catalog of model profiles.
    /// * `health_tracker` - Health tracker for circuit breaker functionality.
    /// * `cost_tracker` - Cost tracker for usage monitoring.
    /// * `registry` - Registry of provider capabilities.
    pub fn new(
        profile_catalog: Arc<RwLock<ProfileCatalog>>,
        health_tracker: Arc<HealthTracker>,
        cost_tracker: Arc<InternalCostTracker>,
        registry: Arc<ModelRegistry>,
    ) -> Self {
        Self {
            profile_catalog,
            health_tracker,
            cost_tracker,
            registry,
            llm_router: None,
            classifier_model: "qwen-turbo".to_string(), // Default fast classifier
        }
    }

    /// Set the LLM router for RouterAgentStrategy (dynamic AI-based routing).
    ///
    /// When set, profiles using the `router_agent` strategy will use this router
    /// to classify incoming requests and select the optimal target model.
    ///
    /// # Arguments
    ///
    /// * `router` - The LLM router to use for classification.
    /// * `classifier_model` - The model to use for classification (e.g., "qwen-turbo").
    pub fn with_llm_router(
        mut self,
        router: Arc<RwLock<LlmRouter>>,
        classifier_model: String,
    ) -> Self {
        self.llm_router = Some(router);
        self.classifier_model = classifier_model;
        self
    }

    /// Set the LLM router after construction.
    pub fn set_llm_router(&mut self, router: Arc<RwLock<LlmRouter>>, classifier_model: String) {
        self.llm_router = Some(router);
        self.classifier_model = classifier_model;
    }

    /// Route a request using the default profile.
    ///
    /// This is equivalent to calling `route_with_profile("default", request)`.
    ///
    /// # Arguments
    ///
    /// * `request` - The chat request to route.
    ///
    /// # Returns
    ///
    /// A `RouteDecision` containing the target provider/model and fallback info.
    pub async fn route(&self, request: &ChatRequest) -> Result<RouteDecision> {
        self.route_with_profile("default", request).await
    }

    /// Route a request using a specific profile.
    ///
    /// The routing process:
    /// 1. Load the profile from the catalog.
    /// 2. Select the appropriate strategy based on profile configuration.
    /// 3. Resolve the target via the strategy.
    /// 4. Check if the target is healthy; if not, try the fallback.
    /// 5. Return the final routing decision.
    ///
    /// # Arguments
    ///
    /// * `profile_id` - The ID of the profile to use for routing.
    /// * `request` - The chat request to route.
    ///
    /// # Returns
    ///
    /// A `RouteDecision` containing the target provider/model and fallback info.
    pub async fn route_with_profile(
        &self,
        profile_id: &str,
        request: &ChatRequest,
    ) -> Result<RouteDecision> {
        // Step 1: Load the profile from the catalog.
        let profile = self.get_profile(profile_id).await?;

        // Step 2: Select the appropriate strategy based on profile configuration.
        let strategy = self.select_strategy(&profile);

        // Step 3: Resolve the target via the strategy.
        let mut decision = strategy.resolve(&profile, request, &self.registry).await?;

        tracing::debug!(
            profile_id = profile_id,
            target_provider = %decision.target.provider,
            target_model = %decision.target.model,
            reason = %decision.reason,
            "Initial routing decision"
        );

        // Step 4: Check if the target is healthy; if not, try the fallback.
        if !self.check_target_health(&decision.target) {
            tracing::warn!(
                provider = %decision.target.provider,
                model = %decision.target.model,
                "Primary target unhealthy, attempting fallback"
            );

            if let Some(fallback) = &decision.fallback {
                if self.check_target_health(fallback) {
                    let original_target = decision.target.clone();
                    decision.target = fallback.clone();
                    decision.reason = format!(
                        "fallback:primary_unhealthy:{}:{}",
                        original_target.provider, original_target.model
                    );
                    decision.fallback = None; // No further fallback available

                    tracing::info!(
                        fallback_provider = %decision.target.provider,
                        fallback_model = %decision.target.model,
                        "Switched to fallback target"
                    );
                } else {
                    tracing::warn!(
                        fallback_provider = %fallback.provider,
                        fallback_model = %fallback.model,
                        "Fallback target also unhealthy, proceeding with primary"
                    );
                    // Proceed with the original target despite being unhealthy.
                    // The circuit breaker may allow probe requests (HalfOpen state).
                }
            }
        }

        tracing::info!(
            profile_id = profile_id,
            target_provider = %decision.target.provider,
            target_model = %decision.target.model,
            reason = %decision.reason,
            has_fallback = decision.fallback.is_some(),
            "Final routing decision"
        );

        Ok(decision)
    }

    /// Select the appropriate routing strategy based on the profile configuration.
    ///
    /// Strategy selection logic:
    /// 1. If RouterAgent is explicitly configured and llm_router is available -> `RouterAgentStrategy`
    /// 2. If `cascade_tiers` is non-empty -> `CascadeStrategy`
    /// 3. If `ab_test_variants` is non-empty -> `AbTestStrategy`
    /// 4. If `task_type_rules` is non-empty -> `TaskTypeRulesStrategy`
    /// 5. Otherwise -> `PrimaryFallbackStrategy`
    ///
    /// # Arguments
    ///
    /// * `profile` - The model profile to select a strategy for.
    ///
    /// # Returns
    ///
    /// A boxed `RoutingStrategyHandler` implementation.
    pub fn select_strategy(&self, profile: &ModelProfile) -> Box<dyn RoutingStrategyHandler> {
        let routing = &profile.routing;

        // Priority 0: RouterAgent strategy (dynamic AI-based routing)
        // Check if we have an LLM router available and the profile has multiple targets
        if let Some(ref llm_router) = self.llm_router {
            let has_multiple_targets = !routing.cascade_tiers.is_empty()
                || !routing.ab_test_variants.is_empty()
                || (routing.primary.is_some() && routing.fallback.is_some());

            // Use RouterAgent if profile name contains "dynamic" or "router-agent"
            let is_dynamic = profile.name.to_lowercase().contains("dynamic")
                || profile.name.to_lowercase().contains("router-agent")
                || profile.name.to_lowercase().contains("ai-router");

            if is_dynamic && has_multiple_targets {
                tracing::debug!(
                    profile_name = %profile.name,
                    classifier_model = %self.classifier_model,
                    "Selected RouterAgentStrategy (dynamic AI routing)"
                );
                return Box::new(RouterAgentStrategy::new(
                    llm_router.clone(),
                    self.classifier_model.clone(),
                ));
            }
        }

        // Priority 1: Multimodal strategy (content-modality-aware routing)
        if let Some(ref mm_config) = routing.multimodal {
            tracing::debug!(
                profile_name = %profile.name,
                "Selected InlineMultimodalStrategy"
            );
            return Box::new(InlineMultimodalStrategy {
                config: mm_config.clone(),
            });
        }

        // Priority 2: Cascade strategy
        if !routing.cascade_tiers.is_empty() {
            tracing::debug!(
                profile_name = %profile.name,
                num_tiers = routing.cascade_tiers.len(),
                "Selected CascadeStrategy"
            );
            return Box::new(CascadeStrategy);
        }

        // Priority 2: A/B test strategy
        if !routing.ab_test_variants.is_empty() {
            tracing::debug!(
                profile_name = %profile.name,
                num_variants = routing.ab_test_variants.len(),
                "Selected AbTestStrategy"
            );
            return Box::new(AbTestStrategy);
        }

        // Priority 3: Task-type rules strategy
        if !routing.task_type_rules.is_empty() {
            tracing::debug!(
                profile_name = %profile.name,
                num_rules = routing.task_type_rules.len(),
                "Selected TaskTypeRulesStrategy"
            );
            return Box::new(TaskTypeRulesStrategy);
        }

        // Default: Primary/Fallback strategy
        tracing::debug!(
            profile_name = %profile.name,
            "Selected PrimaryFallbackStrategy"
        );
        Box::new(PrimaryFallbackStrategy)
    }

    /// Check if a target provider is healthy.
    ///
    /// Uses the health tracker's circuit breaker to determine if the provider
    /// should receive traffic.
    ///
    /// # Arguments
    ///
    /// * `target` - The model target to check.
    ///
    /// # Returns
    ///
    /// `true` if the provider is healthy, `false` otherwise.
    pub fn check_target_health(&self, target: &ModelTarget) -> bool {
        self.health_tracker.is_healthy(&target.provider)
    }

    /// Record a successful request to a target.
    ///
    /// Updates the health tracker with success information and latency data.
    ///
    /// # Arguments
    ///
    /// * `target` - The target that was called.
    /// * `latency` - The latency of the successful call.
    pub fn record_success(&self, target: &ModelTarget, latency: Duration) {
        self.health_tracker
            .record_success_with_latency(&target.provider, latency);

        tracing::trace!(
            provider = %target.provider,
            model = %target.model,
            latency_ms = latency.as_millis(),
            "Recorded successful request"
        );
    }

    /// Record a failed request to a target.
    ///
    /// Updates the health tracker with failure information. This may trigger
    /// circuit breaker state transitions.
    ///
    /// # Arguments
    ///
    /// * `target` - The target that failed.
    pub fn record_failure(&self, target: &ModelTarget) {
        self.health_tracker.record_failure(&target.provider);

        tracing::debug!(
            provider = %target.provider,
            model = %target.model,
            "Recorded failed request"
        );
    }

    /// Get a reference to the cost tracker.
    ///
    /// This allows callers to record usage and query costs.
    pub fn cost_tracker(&self) -> &Arc<InternalCostTracker> {
        &self.cost_tracker
    }

    /// Get a reference to the health tracker.
    ///
    /// This allows callers to query health status.
    pub fn health_tracker(&self) -> &Arc<HealthTracker> {
        &self.health_tracker
    }

    /// Get a reference to the model registry.
    ///
    /// This allows callers to query provider capabilities.
    pub fn registry(&self) -> &Arc<ModelRegistry> {
        &self.registry
    }

    /// Get a reference to the profile catalog.
    ///
    /// This allows callers to access the raw profile catalog for CRUD operations.
    pub fn profile_catalog(&self) -> &Arc<RwLock<ProfileCatalog>> {
        &self.profile_catalog
    }

    /// List all profiles as (id, ModelProfile) pairs.
    ///
    /// R3-H5: Now delegates to profile_catalog instead of returning empty vec.
    pub async fn list_profiles(&self) -> Vec<(String, crate::model_profile::ModelProfile)> {
        let catalog = self.profile_catalog.read().await;
        catalog
            .list()
            .await
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect()
    }

    /// List all templates as (id, ProfileTemplate) pairs.
    ///
    /// R3-H5: Now delegates to profile_catalog instead of returning empty vec.
    pub async fn list_templates(&self) -> Vec<(String, crate::model_profile::ProfileTemplate)> {
        let catalog = self.profile_catalog.read().await;
        catalog
            .list_templates()
            .await
            .into_iter()
            .map(|t| (t.template_id.clone(), t))
            .collect()
    }

    /// Get all health snapshots for all tracked providers.
    ///
    /// This is a convenience method for health status API.
    pub async fn get_all_health_snapshots(
        &self,
    ) -> std::collections::HashMap<String, crate::health::ProviderHealthSnapshot> {
        self.health_tracker.get_all_status()
    }

    /// Get a profile by ID (public wrapper around the internal method).
    ///
    /// Returns the converted ModelProfile if found.
    pub async fn get_profile_by_id(&self, profile_id: &str) -> Result<ModelProfile> {
        self.get_profile(profile_id).await
    }

    /// Get usage records from the cost tracker.
    ///
    /// This is a convenience method for cost tracking API.
    pub fn get_usage_records(&self) -> Vec<crate::cost_tracker::ModelUsageRecord> {
        self.cost_tracker.get_summary()
    }

    // -- Private helpers ------------------------------------------------------

    /// Get a profile from the catalog, converting from the model_profile types
    /// to the strategies types.
    async fn get_profile(&self, profile_id: &str) -> Result<ModelProfile> {
        let catalog = self.profile_catalog.read().await;
        let profile = catalog.get(profile_id).await?;

        // Convert model_profile::ModelProfile to strategies::ModelProfile
        let strategies_profile = self.convert_profile(&profile);

        Ok(strategies_profile)
    }

    /// Convert a profile from model_profile types to strategies types.
    ///
    /// This is necessary because the two modules define slightly different
    /// versions of the same types for modularity reasons.
    fn convert_profile(&self, profile: &crate::model_profile::ModelProfile) -> ModelProfile {
        use crate::strategies::{
            AbTestVariant, CascadeTier, ModelTarget as StratTarget, RoutingConfig, TaskTypeRule,
        };

        // Convert primary target
        let primary = profile.routing.primary.as_ref().map(|t| StratTarget {
            provider: t.provider.clone(),
            model: t.model.clone(),
        });

        // Convert fallback (first in the list if present)
        let fallback = profile
            .routing
            .fallbacks
            .as_ref()
            .and_then(|fallbacks| fallbacks.first())
            .map(|t| StratTarget {
                provider: t.provider.clone(),
                model: t.model.clone(),
            });

        // Convert task type rules
        let task_type_rules: Vec<TaskTypeRule> = profile
            .routing
            .task_type_rules
            .as_ref()
            .map(|rules| {
                rules
                    .iter()
                    .map(|r| TaskTypeRule {
                        task_type: r.task_pattern.clone(),
                        preferred: StratTarget {
                            provider: r.target.provider.clone(),
                            model: r.target.model.clone(),
                        },
                        fallback: None, // The model_profile TaskTypeRule doesn't have per-rule fallback
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Convert A/B test variants
        // Note: model_profile uses f64 weights (e.g., 0.5 for 50%), but strategies
        // uses u32 weights. We scale by 100 to preserve proportions.
        let ab_test_variants: Vec<AbTestVariant> = profile
            .routing
            .ab_test_variants
            .as_ref()
            .map(|variants| {
                variants
                    .iter()
                    .map(|v| AbTestVariant {
                        name: v.name.clone(),
                        target: StratTarget {
                            provider: v.target.provider.clone(),
                            model: v.target.model.clone(),
                        },
                        // Scale f64 weight to u32 (e.g., 0.5 -> 50, 0.3 -> 30)
                        weight: (v.weight * 100.0).round() as u32,
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Convert cascade tiers
        let cascade_tiers: Vec<CascadeTier> = if let Some(cascade) = &profile.routing.cascade {
            // Use explicit cascade configuration
            cascade
                .tiers
                .iter()
                .map(|t| CascadeTier {
                    label: t.label.clone(),
                    target: StratTarget {
                        provider: t.target.provider.clone(),
                        model: t.target.model.clone(),
                    },
                })
                .collect()
        } else if profile.routing.strategy == crate::model_profile::RoutingStrategy::RouterAgent {
            // For RouterAgent strategy, convert primary + fallbacks to cascade tiers
            // so the AI classifier has multiple targets to choose from
            let mut tiers = Vec::new();

            // Add primary as first tier (fast/cheap - for simple queries)
            if let Some(p) = &primary {
                tiers.push(CascadeTier {
                    label: "Fast/cheap - USE FOR: simple questions, translations, formatting, factual lookups".to_string(),
                    target: p.clone(),
                });
            }

            // Add all fallbacks as subsequent tiers
            if let Some(fallbacks) = &profile.routing.fallbacks {
                for (i, fb) in fallbacks.iter().enumerate() {
                    let label = match i {
                        0 => "Standard - USE FOR: explanations, comparisons, summaries, moderate analysis".to_string(),
                        1 => "Powerful - USE FOR: complex coding, deep reasoning, creative writing, math proofs".to_string(),
                        _ => format!("Fallback tier {} - USE FOR: backup option", i + 2),
                    };
                    tiers.push(CascadeTier {
                        label,
                        target: StratTarget {
                            provider: fb.provider.clone(),
                            model: fb.model.clone(),
                        },
                    });
                }
            }

            tiers
        } else {
            Vec::new()
        };

        // Convert multimodal config
        let multimodal = profile.routing.multimodal.as_ref().map(|mm| {
            crate::strategies::MultimodalRoutingConfig {
                text_target: StratTarget {
                    provider: mm.text_target.provider.clone(),
                    model: mm.text_target.model.clone(),
                },
                vision_target: StratTarget {
                    provider: mm.vision_target.provider.clone(),
                    model: mm.vision_target.model.clone(),
                },
                hybrid_target: StratTarget {
                    provider: mm.hybrid_target.provider.clone(),
                    model: mm.hybrid_target.model.clone(),
                },
            }
        });

        ModelProfile {
            name: profile.id.clone(),
            description: profile.description.clone(),
            routing: RoutingConfig {
                primary,
                fallback,
                task_type_rules,
                ab_test_variants,
                cascade_tiers,
                multimodal,
            },
        }
    }
}

impl std::fmt::Debug for RoutingEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutingEngine")
            .field("profile_catalog", &"Arc<RwLock<ProfileCatalog>>")
            .field("health_tracker", &"Arc<HealthTracker>")
            .field("cost_tracker", &"Arc<InternalCostTracker>")
            .field("registry", &"Arc<ModelRegistry>")
            .finish()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::HealthConfig;
    use crate::model_profile::{
        AbTestVariant as ProfileAbTestVariant, CascadeConfig, CascadeTier as ProfileCascadeTier,
        ModelProfile as ProfileModelProfile, ModelTarget as ProfileModelTarget,
        RoutingConfig as ProfileRoutingConfig, RoutingStrategy,
        TaskTypeRule as ProfileTaskTypeRule,
    };
    use crate::router::Message;
    use crate::strategies::ModelCapabilities;

    // -- Test Helpers ---------------------------------------------------------

    fn create_test_profile_catalog() -> ProfileCatalog {
        ProfileCatalog::empty()
    }

    fn create_test_health_tracker() -> Arc<HealthTracker> {
        Arc::new(HealthTracker::new(HealthConfig {
            failure_threshold: 3,
            cooldown_seconds: 0, // Immediate cooldown for testing
            success_to_recover: 2,
        }))
    }

    fn create_test_cost_tracker() -> Arc<InternalCostTracker> {
        Arc::new(InternalCostTracker::with_default_pricing())
    }

    fn create_test_registry() -> Arc<ModelRegistry> {
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
        registry.register(
            "google",
            ModelCapabilities {
                tool_calling: true,
                vision: true,
                streaming: true,
                max_context_tokens: Some(1_000_000),
            },
        );
        Arc::new(registry)
    }

    fn primary_target() -> ProfileModelTarget {
        ProfileModelTarget {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
        }
    }

    fn fallback_target() -> ProfileModelTarget {
        ProfileModelTarget {
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
        }
    }

    fn google_target() -> ProfileModelTarget {
        ProfileModelTarget {
            provider: "google".to_string(),
            model: "gemini-2.0-flash".to_string(),
        }
    }

    fn simple_profile(id: &str) -> ProfileModelProfile {
        ProfileModelProfile {
            id: id.to_string(),
            name: format!("Test Profile {}", id),
            description: "A test profile".to_string(),
            enabled: true,
            routing: ProfileRoutingConfig {
                strategy: RoutingStrategy::PrimaryFallback,
                primary: Some(primary_target()),
                fallbacks: Some(vec![fallback_target()]),
                ..ProfileRoutingConfig::default()
            },
            agent: Default::default(),
            cache_enabled: false,
            cache_ttl_seconds: 3600,
        }
    }

    fn cascade_profile(id: &str) -> ProfileModelProfile {
        ProfileModelProfile {
            id: id.to_string(),
            name: format!("Cascade Profile {}", id),
            description: "A cascade test profile".to_string(),
            enabled: true,
            routing: ProfileRoutingConfig {
                strategy: RoutingStrategy::Cascade,
                cascade: Some(CascadeConfig {
                    tiers: vec![
                        ProfileCascadeTier {
                            label: "fast".to_string(),
                            target: google_target(),
                            max_tokens: Some(1024),
                            quality_threshold: Some(0.7),
                        },
                        ProfileCascadeTier {
                            label: "strong".to_string(),
                            target: primary_target(),
                            max_tokens: None,
                            quality_threshold: None,
                        },
                    ],
                    max_escalations: 2,
                }),
                ..ProfileRoutingConfig::default()
            },
            agent: Default::default(),
            cache_enabled: false,
            cache_ttl_seconds: 3600,
        }
    }

    fn ab_test_profile(id: &str) -> ProfileModelProfile {
        ProfileModelProfile {
            id: id.to_string(),
            name: format!("A/B Test Profile {}", id),
            description: "An A/B test profile".to_string(),
            enabled: true,
            routing: ProfileRoutingConfig {
                strategy: RoutingStrategy::AbTest,
                primary: Some(primary_target()),
                fallbacks: Some(vec![fallback_target()]),
                ab_test_variants: Some(vec![
                    ProfileAbTestVariant {
                        name: "control".to_string(),
                        target: primary_target(),
                        weight: 0.5,
                    },
                    ProfileAbTestVariant {
                        name: "treatment".to_string(),
                        target: fallback_target(),
                        weight: 0.5,
                    },
                ]),
                ..ProfileRoutingConfig::default()
            },
            agent: Default::default(),
            cache_enabled: false,
            cache_ttl_seconds: 3600,
        }
    }

    fn task_type_profile(id: &str) -> ProfileModelProfile {
        ProfileModelProfile {
            id: id.to_string(),
            name: format!("Task Type Profile {}", id),
            description: "A task-type routing profile".to_string(),
            enabled: true,
            routing: ProfileRoutingConfig {
                strategy: RoutingStrategy::TaskTypeRules,
                primary: Some(primary_target()),
                fallbacks: Some(vec![fallback_target()]),
                task_type_rules: Some(vec![
                    ProfileTaskTypeRule {
                        task_pattern: "code".to_string(),
                        target: primary_target(),
                        priority: 0,
                    },
                    ProfileTaskTypeRule {
                        task_pattern: "analysis".to_string(),
                        target: fallback_target(),
                        priority: 1,
                    },
                ]),
                ..ProfileRoutingConfig::default()
            },
            agent: Default::default(),
            cache_enabled: false,
            cache_ttl_seconds: 3600,
        }
    }

    fn plain_request() -> ChatRequest {
        ChatRequest {
            messages: vec![Message::text("user", "Hello, world!")],
            ..Default::default()
        }
    }

    async fn create_engine_with_profile(profile: ProfileModelProfile) -> RoutingEngine {
        let catalog = create_test_profile_catalog();
        catalog.upsert(profile).await;
        let catalog = Arc::new(RwLock::new(catalog));

        RoutingEngine {
            profile_catalog: catalog,
            health_tracker: create_test_health_tracker(),
            cost_tracker: create_test_cost_tracker(),
            registry: create_test_registry(),
            llm_router: None,
            classifier_model: "test-classifier".to_string(),
        }
    }

    // -- Strategy Selection Tests ---------------------------------------------

    #[tokio::test]
    async fn test_select_strategy_primary_fallback() {
        let profile = simple_profile("test");
        let engine = create_engine_with_profile(profile.clone()).await;
        let converted = engine.convert_profile(&profile);

        let strategy = engine.select_strategy(&converted);
        // We can't directly compare strategy types, but we can verify the
        // strategy produces the expected result.
        let request = plain_request();
        let decision = strategy
            .resolve(&converted, &request, &engine.registry)
            .await
            .unwrap();

        assert_eq!(decision.target.provider, "anthropic");
        assert_eq!(decision.reason, "primary");
    }

    #[tokio::test]
    async fn test_select_strategy_cascade() {
        let profile = cascade_profile("cascade-test");
        let engine = create_engine_with_profile(profile.clone()).await;
        let converted = engine.convert_profile(&profile);

        let strategy = engine.select_strategy(&converted);
        let request = plain_request();
        let decision = strategy
            .resolve(&converted, &request, &engine.registry)
            .await
            .unwrap();

        assert_eq!(decision.target.provider, "google");
        assert!(decision.reason.starts_with("cascade:"));
    }

    #[tokio::test]
    async fn test_select_strategy_ab_test() {
        let profile = ab_test_profile("ab-test");
        let engine = create_engine_with_profile(profile.clone()).await;
        let converted = engine.convert_profile(&profile);

        let strategy = engine.select_strategy(&converted);
        let request = plain_request();
        let decision = strategy
            .resolve(&converted, &request, &engine.registry)
            .await
            .unwrap();

        // Should be one of the two variants
        assert!(
            decision.target.provider == "anthropic" || decision.target.provider == "openai",
            "Unexpected provider: {}",
            decision.target.provider
        );
        assert!(
            decision.reason.starts_with("ab_test:"),
            "Unexpected reason: {}",
            decision.reason
        );
    }

    #[tokio::test]
    async fn test_select_strategy_task_type_rules() {
        let profile = task_type_profile("task-type");
        let engine = create_engine_with_profile(profile.clone()).await;
        let converted = engine.convert_profile(&profile);

        let strategy = engine.select_strategy(&converted);
        let request = ChatRequest {
            messages: vec![Message::text("user", "test")],
            model: Some("task:code".to_string()),
            ..Default::default()
        };
        let decision = strategy
            .resolve(&converted, &request, &engine.registry)
            .await
            .unwrap();

        assert_eq!(decision.target.provider, "anthropic");
        assert_eq!(decision.reason, "task_type_rule:code");
    }

    // -- Route Tests ----------------------------------------------------------

    #[tokio::test]
    async fn test_route_with_profile_success() {
        let profile = simple_profile("default");
        let engine = create_engine_with_profile(profile).await;

        let request = plain_request();
        let decision = engine
            .route_with_profile("default", &request)
            .await
            .unwrap();

        assert_eq!(decision.target.provider, "anthropic");
        assert_eq!(decision.target.model, "claude-sonnet-4-6");
    }

    #[tokio::test]
    async fn test_route_default_profile() {
        let profile = simple_profile("default");
        let engine = create_engine_with_profile(profile).await;

        let request = plain_request();
        let decision = engine.route(&request).await.unwrap();

        assert_eq!(decision.target.provider, "anthropic");
    }

    #[tokio::test]
    async fn test_route_profile_not_found() {
        let profile = simple_profile("default");
        let engine = create_engine_with_profile(profile).await;

        let request = plain_request();
        let result = engine.route_with_profile("nonexistent", &request).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // -- Health Check Tests ---------------------------------------------------

    #[tokio::test]
    async fn test_check_target_health_healthy() {
        let profile = simple_profile("default");
        let engine = create_engine_with_profile(profile).await;

        let target = ModelTarget {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
        };

        assert!(engine.check_target_health(&target));
    }

    #[tokio::test]
    async fn test_check_target_health_after_failures() {
        let catalog = create_test_profile_catalog();
        catalog.upsert(simple_profile("default")).await;
        let catalog = Arc::new(RwLock::new(catalog));

        // Use a health tracker with longer cooldown for this test
        let health_tracker = Arc::new(HealthTracker::new(HealthConfig {
            failure_threshold: 3,
            cooldown_seconds: 9999, // Long cooldown
            success_to_recover: 2,
        }));

        let engine = RoutingEngine {
            profile_catalog: catalog,
            health_tracker,
            cost_tracker: create_test_cost_tracker(),
            registry: create_test_registry(),
            llm_router: None,
            classifier_model: "test-classifier".to_string(),
        };

        let target = ModelTarget {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
        };

        // Record failures to trip the circuit breaker
        for _ in 0..3 {
            engine.record_failure(&target);
        }

        // Now the provider should be unhealthy
        assert!(!engine.check_target_health(&target));
    }

    #[tokio::test]
    async fn test_fallback_on_unhealthy_primary() {
        let catalog = create_test_profile_catalog();
        catalog.upsert(simple_profile("default")).await;
        let catalog = Arc::new(RwLock::new(catalog));

        // Use a health tracker with longer cooldown
        let health_tracker = Arc::new(HealthTracker::new(HealthConfig {
            failure_threshold: 3,
            cooldown_seconds: 9999,
            success_to_recover: 2,
        }));

        let engine = RoutingEngine {
            profile_catalog: catalog,
            health_tracker: health_tracker.clone(),
            cost_tracker: create_test_cost_tracker(),
            registry: create_test_registry(),
            llm_router: None,
            classifier_model: "test-classifier".to_string(),
        };

        // Trip the circuit breaker for anthropic
        for _ in 0..3 {
            health_tracker.record_failure("anthropic");
        }

        let request = plain_request();
        let decision = engine
            .route_with_profile("default", &request)
            .await
            .unwrap();

        // Should have fallen back to openai
        assert_eq!(decision.target.provider, "openai");
        assert!(decision.reason.contains("fallback:primary_unhealthy"));
    }

    // -- Success/Failure Recording Tests --------------------------------------

    #[tokio::test]
    async fn test_record_success_updates_health() {
        let profile = simple_profile("default");
        let engine = create_engine_with_profile(profile).await;

        let target = ModelTarget {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
        };

        engine.record_success(&target, Duration::from_millis(100));

        let status = engine
            .health_tracker()
            .get_provider_status("anthropic")
            .unwrap();
        assert_eq!(status.total_requests, 1);
        assert!((status.avg_latency_ms - 100.0).abs() < 1.0);
    }

    #[tokio::test]
    async fn test_record_failure_updates_health() {
        let profile = simple_profile("default");
        let engine = create_engine_with_profile(profile).await;

        let target = ModelTarget {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
        };

        engine.record_failure(&target);

        let status = engine
            .health_tracker()
            .get_provider_status("anthropic")
            .unwrap();
        assert_eq!(status.consecutive_failures, 1);
    }

    // -- Profile Conversion Tests ---------------------------------------------

    #[tokio::test]
    async fn test_convert_profile_primary_fallback() {
        let profile = simple_profile("convert-test");
        let engine = create_engine_with_profile(profile.clone()).await;

        let converted = engine.convert_profile(&profile);

        assert_eq!(converted.name, "convert-test");
        assert!(converted.routing.primary.is_some());
        assert!(converted.routing.fallback.is_some());
        assert_eq!(
            converted.routing.primary.as_ref().unwrap().provider,
            "anthropic"
        );
        assert_eq!(
            converted.routing.fallback.as_ref().unwrap().provider,
            "openai"
        );
    }

    #[tokio::test]
    async fn test_convert_profile_cascade() {
        let profile = cascade_profile("cascade-convert");
        let engine = create_engine_with_profile(profile.clone()).await;

        let converted = engine.convert_profile(&profile);

        assert_eq!(converted.routing.cascade_tiers.len(), 2);
        assert_eq!(converted.routing.cascade_tiers[0].label, "fast");
        assert_eq!(converted.routing.cascade_tiers[0].target.provider, "google");
        assert_eq!(converted.routing.cascade_tiers[1].label, "strong");
    }

    #[tokio::test]
    async fn test_convert_profile_ab_test() {
        let profile = ab_test_profile("ab-convert");
        let engine = create_engine_with_profile(profile.clone()).await;

        let converted = engine.convert_profile(&profile);

        assert_eq!(converted.routing.ab_test_variants.len(), 2);
        assert_eq!(converted.routing.ab_test_variants[0].name, "control");
        assert_eq!(converted.routing.ab_test_variants[1].name, "treatment");
    }

    #[tokio::test]
    async fn test_convert_profile_task_type_rules() {
        let profile = task_type_profile("task-convert");
        let engine = create_engine_with_profile(profile.clone()).await;

        let converted = engine.convert_profile(&profile);

        assert_eq!(converted.routing.task_type_rules.len(), 2);
        assert_eq!(converted.routing.task_type_rules[0].task_type, "code");
        assert_eq!(converted.routing.task_type_rules[1].task_type, "analysis");
    }

    // -- Accessor Tests -------------------------------------------------------

    #[tokio::test]
    async fn test_accessor_methods() {
        let profile = simple_profile("default");
        let engine = create_engine_with_profile(profile).await;

        // Verify accessors return valid references
        let _ = engine.cost_tracker();
        let _ = engine.health_tracker();
        let _ = engine.registry();
    }

    // -- Debug Impl Test ------------------------------------------------------

    #[tokio::test]
    async fn test_debug_impl() {
        let profile = simple_profile("default");
        let engine = create_engine_with_profile(profile).await;

        let debug_str = format!("{:?}", engine);
        assert!(debug_str.contains("RoutingEngine"));
        assert!(debug_str.contains("profile_catalog"));
        assert!(debug_str.contains("health_tracker"));
    }

    // -- Edge Cases -----------------------------------------------------------

    #[tokio::test]
    async fn test_both_primary_and_fallback_unhealthy() {
        let catalog = create_test_profile_catalog();
        catalog.upsert(simple_profile("default")).await;
        let catalog = Arc::new(RwLock::new(catalog));

        let health_tracker = Arc::new(HealthTracker::new(HealthConfig {
            failure_threshold: 3,
            cooldown_seconds: 9999,
            success_to_recover: 2,
        }));

        let engine = RoutingEngine::new(
            catalog,
            health_tracker.clone(),
            create_test_cost_tracker(),
            create_test_registry(),
        );

        // Trip circuit breakers for both providers
        for _ in 0..3 {
            health_tracker.record_failure("anthropic");
            health_tracker.record_failure("openai");
        }

        let request = plain_request();
        let decision = engine
            .route_with_profile("default", &request)
            .await
            .unwrap();

        // Should still return primary (circuit breaker may allow probe requests
        // or we proceed despite being unhealthy)
        // The reason should still be "primary" since we couldn't switch to fallback
        assert_eq!(decision.target.provider, "anthropic");
    }

    #[tokio::test]
    async fn test_cascade_with_unhealthy_first_tier() {
        let catalog = create_test_profile_catalog();
        catalog.upsert(cascade_profile("cascade")).await;
        let catalog = Arc::new(RwLock::new(catalog));

        let health_tracker = Arc::new(HealthTracker::new(HealthConfig {
            failure_threshold: 3,
            cooldown_seconds: 9999,
            success_to_recover: 2,
        }));

        let engine = RoutingEngine::new(
            catalog,
            health_tracker.clone(),
            create_test_cost_tracker(),
            create_test_registry(),
        );

        // Trip circuit breaker for google (first cascade tier)
        for _ in 0..3 {
            health_tracker.record_failure("google");
        }

        let request = plain_request();
        let decision = engine
            .route_with_profile("cascade", &request)
            .await
            .unwrap();

        // Should fall back to second tier (anthropic)
        assert_eq!(decision.target.provider, "anthropic");
        assert!(decision.reason.contains("fallback:primary_unhealthy"));
    }
}
