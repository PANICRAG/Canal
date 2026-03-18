//! A/B test routing strategy.
//!
//! Performs weighted random selection across a set of A/B test variants defined
//! in the model profile. Each variant carries a relative weight; the strategy
//! normalises weights into a cumulative distribution and picks a variant using a
//! simple deterministic-enough random value derived from `SystemTime` nanos.
//!
//! This avoids pulling in an external RNG crate while still providing
//! reasonable distribution for routing purposes. For production-grade
//! experimentation you would swap the entropy source for a proper PRNG or
//! hash-based assignment keyed on a user/session ID.

use async_trait::async_trait;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::{ModelProfile, ModelRegistry, RouteDecision, RoutingStrategyHandler};
use crate::error::{Error, Result};
use crate::router::ChatRequest;

/// Strategy that routes requests to A/B test variants by weighted random
/// selection.
pub struct AbTestStrategy;

impl AbTestStrategy {
    /// R3-M: Generate a deterministic hash from request content.
    ///
    /// Uses the first user message as a seed so the same user/message
    /// consistently maps to the same A/B variant. Falls back to profile
    /// name hash when no messages are available.
    fn deterministic_hash(request: &ChatRequest, profile: &ModelProfile) -> u64 {
        let mut hasher = DefaultHasher::new();
        // Hash the first user message for per-request consistency
        if let Some(msg) = request.messages.first() {
            msg.role.hash(&mut hasher);
            msg.content.hash(&mut hasher);
        }
        // Include profile name to differentiate across profiles
        profile.name.hash(&mut hasher);
        hasher.finish()
    }
}

#[async_trait]
impl RoutingStrategyHandler for AbTestStrategy {
    async fn resolve(
        &self,
        profile: &ModelProfile,
        request: &ChatRequest,
        _registry: &ModelRegistry,
    ) -> Result<RouteDecision> {
        let variants = &profile.routing.ab_test_variants;

        if variants.is_empty() {
            return Err(Error::Llm(
                "No A/B test variants configured in profile".into(),
            ));
        }

        // Single variant — deterministic shortcut.
        if variants.len() == 1 {
            let v = &variants[0];
            return Ok(RouteDecision {
                target: v.target.clone(),
                reason: format!("ab_test:{}", v.name),
                fallback: profile.routing.fallback.clone(),
            });
        }

        // Compute total weight.
        let total_weight: u64 = variants.iter().map(|v| v.weight as u64).sum();

        if total_weight == 0 {
            return Err(Error::Llm(
                "A/B test variants have zero total weight".into(),
            ));
        }

        // R3-M: Pick a deterministic point in [0, total_weight) using request content hash
        let roll = Self::deterministic_hash(request, profile) % total_weight;

        let mut cumulative: u64 = 0;
        for variant in variants {
            cumulative += variant.weight as u64;
            if roll < cumulative {
                tracing::info!(
                    variant = %variant.name,
                    weight = variant.weight,
                    roll = roll,
                    total_weight = total_weight,
                    "A/B test variant selected"
                );
                return Ok(RouteDecision {
                    target: variant.target.clone(),
                    reason: format!("ab_test:{}", variant.name),
                    fallback: profile.routing.fallback.clone(),
                });
            }
        }

        // Should be unreachable, but handle gracefully — pick the last variant.
        let last = variants.last().unwrap();
        Ok(RouteDecision {
            target: last.target.clone(),
            reason: format!("ab_test:{}:overflow", last.name),
            fallback: profile.routing.fallback.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{ChatRequest, Message};
    use crate::strategies::{
        AbTestVariant, ModelProfile, ModelRegistry, ModelTarget, RoutingConfig,
    };

    fn target_a() -> ModelTarget {
        ModelTarget {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
        }
    }

    fn target_b() -> ModelTarget {
        ModelTarget {
            provider: "openai".into(),
            model: "gpt-4o".into(),
        }
    }

    fn make_profile(variants: Vec<AbTestVariant>) -> ModelProfile {
        ModelProfile {
            name: "ab-test-profile".into(),
            description: String::new(),
            routing: RoutingConfig {
                primary: Some(target_a()),
                fallback: Some(target_b()),
                ab_test_variants: variants,
                ..Default::default()
            },
        }
    }

    fn plain_request() -> ChatRequest {
        ChatRequest {
            messages: vec![Message::text("user", "test")],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_single_variant() {
        let profile = make_profile(vec![AbTestVariant {
            name: "control".into(),
            target: target_a(),
            weight: 1,
        }]);
        let registry = ModelRegistry::new();

        let decision = AbTestStrategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, target_a());
        assert_eq!(decision.reason, "ab_test:control");
    }

    #[tokio::test]
    async fn test_multiple_variants_returns_valid_target() {
        let profile = make_profile(vec![
            AbTestVariant {
                name: "control".into(),
                target: target_a(),
                weight: 50,
            },
            AbTestVariant {
                name: "treatment".into(),
                target: target_b(),
                weight: 50,
            },
        ]);
        let registry = ModelRegistry::new();

        // Run several times — every result should be one of the two targets.
        for _ in 0..20 {
            let decision = AbTestStrategy
                .resolve(&profile, &plain_request(), &registry)
                .await
                .unwrap();

            assert!(
                decision.target == target_a() || decision.target == target_b(),
                "Unexpected target: {:?}",
                decision.target
            );
            assert!(
                decision.reason.starts_with("ab_test:"),
                "Unexpected reason: {}",
                decision.reason
            );
        }
    }

    #[tokio::test]
    async fn test_error_on_empty_variants() {
        let profile = make_profile(vec![]);
        let registry = ModelRegistry::new();

        let result = AbTestStrategy
            .resolve(&profile, &plain_request(), &registry)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_error_on_zero_total_weight() {
        let profile = make_profile(vec![
            AbTestVariant {
                name: "a".into(),
                target: target_a(),
                weight: 0,
            },
            AbTestVariant {
                name: "b".into(),
                target: target_b(),
                weight: 0,
            },
        ]);
        let registry = ModelRegistry::new();

        let result = AbTestStrategy
            .resolve(&profile, &plain_request(), &registry)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_fallback_propagated() {
        let profile = make_profile(vec![AbTestVariant {
            name: "only".into(),
            target: target_a(),
            weight: 1,
        }]);
        let registry = ModelRegistry::new();

        let decision = AbTestStrategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        // The profile has a fallback configured — it should be passed through.
        assert_eq!(decision.fallback, Some(target_b()));
    }
}
