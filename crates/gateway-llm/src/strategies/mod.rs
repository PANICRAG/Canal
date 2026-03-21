//! Routing strategy module for the LLM Model Router.
//!
//! This module defines the `RoutingStrategyHandler` trait and supporting types
//! that allow the Model Router to resolve which LLM provider/model to route a
//! request to. Each strategy encapsulates a different routing policy:
//!
//! - **PrimaryFallback** — Use the primary target unless it lacks a required
//!   capability (e.g., tool calling), in which case fall back.
//! - **TaskTypeRules** — Match the request's `task_type` against a set of rules
//!   to pick the best provider/model for the job.
//! - **AbTest** — Weighted random selection across A/B test variants.
//! - **Cascade** — Ordered tier list; the first tier is the initial target and
//!   the second tier is the automatic fallback.

pub mod ab_test;
pub mod cascade;
pub mod primary_fallback;
pub mod router_agent;
pub mod task_type_rules;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Result;
use crate::router::ChatRequest;

// ============================================================================
// Core types used by all routing strategies
// ============================================================================

/// A concrete provider + model pair that a request can be routed to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelTarget {
    /// Provider name (e.g., "anthropic", "openai", "google").
    pub provider: String,
    /// Model identifier within the provider (e.g., "claude-sonnet-4-6").
    pub model: String,
}

/// Capabilities advertised by a provider/model combination.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Whether the model supports tool/function calling.
    pub tool_calling: bool,
    /// Whether the model supports vision (image) inputs.
    pub vision: bool,
    /// Whether the model supports streaming responses.
    pub streaming: bool,
    /// Maximum context window in tokens.
    pub max_context_tokens: Option<u32>,
}

/// A rule that maps a `task_type` string to a preferred model target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTypeRule {
    /// The task type this rule applies to (e.g., "code", "analysis", "chat").
    pub task_type: String,
    /// The preferred target when this rule matches.
    pub preferred: ModelTarget,
    /// Optional fallback if the preferred target is unavailable.
    pub fallback: Option<ModelTarget>,
}

/// An A/B test variant with an associated weight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbTestVariant {
    /// Human-readable variant name (e.g., "control", "treatment_a").
    pub name: String,
    /// The model target for this variant.
    pub target: ModelTarget,
    /// Relative weight (the sum of all variant weights determines the
    /// probability distribution).
    pub weight: u32,
}

/// Cascade tier — an ordered preference level in a cascade strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CascadeTier {
    /// Human-readable tier label (e.g., "primary", "secondary").
    pub label: String,
    /// The model target for this tier.
    pub target: ModelTarget,
}

/// Configuration for multimodal routing within the strategy layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultimodalRoutingConfig {
    /// Target for text-only requests.
    pub text_target: ModelTarget,
    /// Target for vision/image requests.
    pub vision_target: ModelTarget,
    /// Target for hybrid (text + image) requests.
    pub hybrid_target: ModelTarget,
}

/// Routing configuration embedded in a `ModelProfile`.
///
/// Each profile can specify one or more routing strategies and their
/// configuration. The router evaluates these to produce a `RouteDecision`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// Primary model target.
    pub primary: Option<ModelTarget>,
    /// Fallback model target when the primary cannot service a request.
    pub fallback: Option<ModelTarget>,
    /// Task-type based routing rules.
    #[serde(default)]
    pub task_type_rules: Vec<TaskTypeRule>,
    /// A/B test variants (empty means A/B testing is disabled).
    #[serde(default)]
    pub ab_test_variants: Vec<AbTestVariant>,
    /// Cascade tiers in priority order.
    #[serde(default)]
    pub cascade_tiers: Vec<CascadeTier>,
    /// Multimodal routing configuration.
    #[serde(default)]
    pub multimodal: Option<MultimodalRoutingConfig>,
}

/// A model profile aggregates metadata and routing configuration for a
/// logical "model slot" (e.g., "fast-chat", "deep-reasoning").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    /// Unique name for this profile.
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Routing configuration.
    #[serde(default)]
    pub routing: RoutingConfig,
}

/// Registry of known providers and their capabilities.
///
/// The router consults the registry when a strategy needs to check whether
/// a provider supports a particular capability (e.g., tool calling).
#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    capabilities: HashMap<String, ModelCapabilities>,
}

impl ModelRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            capabilities: HashMap::new(),
        }
    }

    /// Register capabilities for a provider.
    pub fn register(&mut self, provider: impl Into<String>, caps: ModelCapabilities) {
        self.capabilities.insert(provider.into(), caps);
    }

    /// Look up capabilities for a provider.
    pub fn get_capabilities(&self, provider: &str) -> Option<&ModelCapabilities> {
        self.capabilities.get(provider)
    }

    /// Check whether a provider is registered.
    pub fn has_provider(&self, provider: &str) -> bool {
        self.capabilities.contains_key(provider)
    }
}

// ============================================================================
// Route decision & strategy trait
// ============================================================================

/// The result of a routing strategy evaluation.
#[derive(Debug, Clone)]
pub struct RouteDecision {
    /// The model target the request should be routed to.
    pub target: ModelTarget,
    /// A short machine-readable reason string (e.g., "primary",
    /// "fallback:primary_no_tool_calling", "ab_test:treatment_a").
    pub reason: String,
    /// An optional fallback target the router may try if the primary target
    /// fails at the provider level.
    pub fallback: Option<ModelTarget>,
}

/// Trait implemented by each routing strategy.
///
/// The Model Router holds one or more `RoutingStrategyHandler` implementations
/// and evaluates them to determine where a request should go.
#[async_trait]
pub trait RoutingStrategyHandler: Send + Sync {
    /// Evaluate the strategy and return a `RouteDecision`.
    ///
    /// # Arguments
    ///
    /// * `profile`  — The model profile whose routing config should be used.
    /// * `request`  — The incoming chat request (may influence the decision,
    ///                e.g., whether tools are present).
    /// * `registry` — The model registry for capability lookups.
    async fn resolve(
        &self,
        profile: &ModelProfile,
        request: &ChatRequest,
        registry: &ModelRegistry,
    ) -> Result<RouteDecision>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_target_equality() {
        let a = ModelTarget {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
        };
        let b = ModelTarget {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_model_registry_lookup() {
        let mut reg = ModelRegistry::new();
        reg.register(
            "anthropic",
            ModelCapabilities {
                tool_calling: true,
                vision: true,
                streaming: true,
                max_context_tokens: Some(200_000),
            },
        );
        assert!(reg.has_provider("anthropic"));
        assert!(!reg.has_provider("openai"));

        let caps = reg.get_capabilities("anthropic").unwrap();
        assert!(caps.tool_calling);
        assert!(caps.vision);
    }

    #[test]
    fn test_routing_config_defaults() {
        let cfg = RoutingConfig::default();
        assert!(cfg.primary.is_none());
        assert!(cfg.fallback.is_none());
        assert!(cfg.task_type_rules.is_empty());
        assert!(cfg.ab_test_variants.is_empty());
        assert!(cfg.cascade_tiers.is_empty());
    }
}
