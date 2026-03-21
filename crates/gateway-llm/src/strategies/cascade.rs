//! Cascade routing strategy.
//!
//! Routes requests through an ordered list of tiers. The first tier is the
//! initial target and the second tier (if present) becomes the automatic
//! fallback. This strategy is useful when you want a clear priority chain
//! — e.g., try the fastest/cheapest model first, then fall back to a more
//! capable (but more expensive) model.

use async_trait::async_trait;

use super::{ModelProfile, ModelRegistry, RouteDecision, RoutingStrategyHandler};
use crate::error::{Error, Result};
use crate::router::ChatRequest;

/// Strategy that picks the first cascade tier as the target and the second
/// tier as the fallback.
pub struct CascadeStrategy;

#[async_trait]
impl RoutingStrategyHandler for CascadeStrategy {
    async fn resolve(
        &self,
        profile: &ModelProfile,
        _request: &ChatRequest,
        _registry: &ModelRegistry,
    ) -> Result<RouteDecision> {
        let tiers = &profile.routing.cascade_tiers;

        if tiers.is_empty() {
            return Err(Error::Llm("No cascade tiers configured in profile".into()));
        }

        let primary_tier = &tiers[0];

        // The fallback is the second tier, if one exists.
        let fallback_tier = tiers.get(1);

        tracing::info!(
            tier = %primary_tier.label,
            provider = %primary_tier.target.provider,
            model = %primary_tier.target.model,
            num_tiers = tiers.len(),
            "Cascade strategy selected tier"
        );

        Ok(RouteDecision {
            target: primary_tier.target.clone(),
            reason: format!("cascade:{}", primary_tier.label),
            fallback: fallback_tier.map(|t| t.target.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{ChatRequest, Message};
    use crate::strategies::{CascadeTier, ModelProfile, ModelRegistry, ModelTarget, RoutingConfig};

    fn fast_target() -> ModelTarget {
        ModelTarget {
            provider: "anthropic".into(),
            model: "claude-haiku".into(),
        }
    }

    fn strong_target() -> ModelTarget {
        ModelTarget {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
        }
    }

    fn premium_target() -> ModelTarget {
        ModelTarget {
            provider: "anthropic".into(),
            model: "claude-opus-4-6".into(),
        }
    }

    fn make_profile(tiers: Vec<CascadeTier>) -> ModelProfile {
        ModelProfile {
            name: "cascade-profile".into(),
            description: String::new(),
            routing: RoutingConfig {
                cascade_tiers: tiers,
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
    async fn test_single_tier() {
        let profile = make_profile(vec![CascadeTier {
            label: "fast".into(),
            target: fast_target(),
        }]);
        let registry = ModelRegistry::new();

        let decision = CascadeStrategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, fast_target());
        assert_eq!(decision.reason, "cascade:fast");
        assert!(decision.fallback.is_none());
    }

    #[tokio::test]
    async fn test_two_tiers() {
        let profile = make_profile(vec![
            CascadeTier {
                label: "fast".into(),
                target: fast_target(),
            },
            CascadeTier {
                label: "strong".into(),
                target: strong_target(),
            },
        ]);
        let registry = ModelRegistry::new();

        let decision = CascadeStrategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, fast_target());
        assert_eq!(decision.reason, "cascade:fast");
        assert_eq!(decision.fallback, Some(strong_target()));
    }

    #[tokio::test]
    async fn test_three_tiers_only_uses_first_two() {
        let profile = make_profile(vec![
            CascadeTier {
                label: "fast".into(),
                target: fast_target(),
            },
            CascadeTier {
                label: "strong".into(),
                target: strong_target(),
            },
            CascadeTier {
                label: "premium".into(),
                target: premium_target(),
            },
        ]);
        let registry = ModelRegistry::new();

        let decision = CascadeStrategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        // The third tier is not used by the basic cascade; the router could
        // implement deeper cascade logic on top of the strategy.
        assert_eq!(decision.target, fast_target());
        assert_eq!(decision.fallback, Some(strong_target()));
    }

    #[tokio::test]
    async fn test_error_on_empty_tiers() {
        let profile = make_profile(vec![]);
        let registry = ModelRegistry::new();

        let result = CascadeStrategy
            .resolve(&profile, &plain_request(), &registry)
            .await;

        assert!(result.is_err());
    }
}
