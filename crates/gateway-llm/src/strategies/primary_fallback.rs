//! Primary/Fallback routing strategy.
//!
//! Routes to the profile's primary target by default. If the request requires
//! a capability that the primary target does not support (e.g., tool calling),
//! the strategy transparently redirects to the configured fallback target.

use async_trait::async_trait;

use super::{ModelProfile, ModelRegistry, RouteDecision, RoutingStrategyHandler};
use crate::error::{Error, Result};
use crate::router::ChatRequest;

/// Check if a ChatRequest contains any image content blocks.
fn request_has_images(request: &ChatRequest) -> bool {
    use crate::router::ContentBlock;
    request.messages.iter().any(|msg| {
        msg.content_blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::Image { .. }))
    })
}

/// Strategy that routes to the primary target with automatic capability-aware
/// fallback.
pub struct PrimaryFallbackStrategy;

#[async_trait]
impl RoutingStrategyHandler for PrimaryFallbackStrategy {
    async fn resolve(
        &self,
        profile: &ModelProfile,
        request: &ChatRequest,
        registry: &ModelRegistry,
    ) -> Result<RouteDecision> {
        let primary = profile
            .routing
            .primary
            .clone()
            .ok_or_else(|| Error::Llm("No primary target configured in profile".into()))?;

        // ---- Capability gate: tool calling ----
        // If the request includes tool definitions we need a provider that
        // advertises tool_calling support.
        if !request.tools.is_empty() {
            if let Some(caps) = registry.get_capabilities(&primary.provider) {
                if !caps.tool_calling {
                    tracing::info!(
                        primary_provider = %primary.provider,
                        "Primary provider lacks tool_calling — checking fallback"
                    );
                    if let Some(fallback) = &profile.routing.fallback {
                        // Verify the fallback actually supports tools before
                        // blindly routing there.
                        let fb_ok = registry
                            .get_capabilities(&fallback.provider)
                            .map_or(true, |c| c.tool_calling);

                        if fb_ok {
                            return Ok(RouteDecision {
                                target: fallback.clone(),
                                reason: "fallback:primary_no_tool_calling".into(),
                                fallback: None,
                            });
                        }
                        // If even the fallback lacks tools, fall through to
                        // the primary and let the provider return an error.
                        tracing::warn!(
                            "Fallback provider also lacks tool_calling — proceeding with primary"
                        );
                    }
                }
            }
        }

        // ---- Capability gate: vision (CP27.4) ----
        // If the request contains image content blocks, route to a vision-capable
        // provider.
        if request_has_images(request) {
            if let Some(caps) = registry.get_capabilities(&primary.provider) {
                if !caps.vision {
                    tracing::info!(
                        primary_provider = %primary.provider,
                        "Primary provider lacks vision — checking fallback"
                    );
                    if let Some(fallback) = &profile.routing.fallback {
                        let fb_ok = registry
                            .get_capabilities(&fallback.provider)
                            .map_or(true, |c| c.vision);
                        if fb_ok {
                            return Ok(RouteDecision {
                                target: fallback.clone(),
                                reason: "fallback:primary_no_vision".into(),
                                fallback: None,
                            });
                        }
                        tracing::warn!(
                            "Fallback provider also lacks vision — proceeding with primary"
                        );
                    }
                }
            }
        }

        Ok(RouteDecision {
            target: primary,
            reason: "primary".into(),
            fallback: profile.routing.fallback.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{ChatRequest, Message, ToolDefinition};
    use crate::strategies::{
        ModelCapabilities, ModelProfile, ModelRegistry, ModelTarget, RoutingConfig,
    };

    fn make_profile(primary: ModelTarget, fallback: Option<ModelTarget>) -> ModelProfile {
        ModelProfile {
            name: "test-profile".into(),
            description: String::new(),
            routing: RoutingConfig {
                primary: Some(primary),
                fallback,
                ..Default::default()
            },
        }
    }

    fn tool_request() -> ChatRequest {
        ChatRequest {
            messages: vec![Message::text("user", "Do something")],
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "Read a file".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            ..Default::default()
        }
    }

    fn plain_request() -> ChatRequest {
        ChatRequest {
            messages: vec![Message::text("user", "Hello")],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_routes_to_primary_for_plain_request() {
        let primary = ModelTarget {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
        };
        let profile = make_profile(primary.clone(), None);
        let registry = ModelRegistry::new();

        let decision = PrimaryFallbackStrategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, primary);
        assert_eq!(decision.reason, "primary");
    }

    #[tokio::test]
    async fn test_falls_back_when_primary_lacks_tools() {
        let primary = ModelTarget {
            provider: "no_tools_provider".into(),
            model: "basic-v1".into(),
        };
        let fallback = ModelTarget {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
        };

        let profile = make_profile(primary.clone(), Some(fallback.clone()));

        let mut registry = ModelRegistry::new();
        registry.register(
            "no_tools_provider",
            ModelCapabilities {
                tool_calling: false,
                ..Default::default()
            },
        );
        registry.register(
            "anthropic",
            ModelCapabilities {
                tool_calling: true,
                ..Default::default()
            },
        );

        let decision = PrimaryFallbackStrategy
            .resolve(&profile, &tool_request(), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, fallback);
        assert_eq!(decision.reason, "fallback:primary_no_tool_calling");
    }

    #[tokio::test]
    async fn test_stays_on_primary_when_tools_supported() {
        let primary = ModelTarget {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
        };

        let profile = make_profile(primary.clone(), None);

        let mut registry = ModelRegistry::new();
        registry.register(
            "anthropic",
            ModelCapabilities {
                tool_calling: true,
                ..Default::default()
            },
        );

        let decision = PrimaryFallbackStrategy
            .resolve(&profile, &tool_request(), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, primary);
        assert_eq!(decision.reason, "primary");
    }

    #[tokio::test]
    async fn test_error_when_no_primary() {
        let profile = ModelProfile {
            name: "empty".into(),
            description: String::new(),
            routing: RoutingConfig::default(),
        };
        let registry = ModelRegistry::new();
        let result = PrimaryFallbackStrategy
            .resolve(&profile, &plain_request(), &registry)
            .await;
        assert!(result.is_err());
    }

    // ---- Vision routing tests (CP27.4) ----

    fn vision_request() -> ChatRequest {
        use crate::router::ContentBlock;
        ChatRequest {
            messages: vec![Message {
                role: "user".into(),
                content: "What's in this image?".into(),
                content_blocks: vec![ContentBlock::Image {
                    source_type: "base64".into(),
                    media_type: "image/png".into(),
                    data: "iVBORw0KGgo=".into(),
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_routing_selects_vision_model() {
        let primary = ModelTarget {
            provider: "text_only".into(),
            model: "text-v1".into(),
        };
        let fallback = ModelTarget {
            provider: "vision_provider".into(),
            model: "vision-v1".into(),
        };

        let profile = make_profile(primary, Some(fallback.clone()));

        let mut registry = ModelRegistry::new();
        registry.register(
            "text_only",
            ModelCapabilities {
                tool_calling: true,
                vision: false,
                ..Default::default()
            },
        );
        registry.register(
            "vision_provider",
            ModelCapabilities {
                tool_calling: true,
                vision: true,
                ..Default::default()
            },
        );

        let decision = PrimaryFallbackStrategy
            .resolve(&profile, &vision_request(), &registry)
            .await
            .unwrap();

        assert_eq!(decision.target, fallback);
        assert_eq!(decision.reason, "fallback:primary_no_vision");
    }

    #[tokio::test]
    async fn test_routing_rejects_text_only() {
        // When no fallback configured and primary lacks vision, still routes to primary
        let primary = ModelTarget {
            provider: "text_only".into(),
            model: "text-v1".into(),
        };

        let profile = make_profile(primary.clone(), None);

        let mut registry = ModelRegistry::new();
        registry.register(
            "text_only",
            ModelCapabilities {
                vision: false,
                ..Default::default()
            },
        );

        let decision = PrimaryFallbackStrategy
            .resolve(&profile, &vision_request(), &registry)
            .await
            .unwrap();

        // Falls through to primary since no fallback configured
        assert_eq!(decision.target, primary);
        assert_eq!(decision.reason, "primary");
    }

    #[tokio::test]
    async fn test_routing_text_unchanged() {
        // Plain text request — vision gate should not trigger
        let primary = ModelTarget {
            provider: "text_only".into(),
            model: "text-v1".into(),
        };
        let fallback = ModelTarget {
            provider: "vision_provider".into(),
            model: "vision-v1".into(),
        };

        let profile = make_profile(primary.clone(), Some(fallback));

        let mut registry = ModelRegistry::new();
        registry.register(
            "text_only",
            ModelCapabilities {
                vision: false,
                ..Default::default()
            },
        );
        registry.register(
            "vision_provider",
            ModelCapabilities {
                vision: true,
                ..Default::default()
            },
        );

        let decision = PrimaryFallbackStrategy
            .resolve(&profile, &plain_request(), &registry)
            .await
            .unwrap();

        // Plain text → stays on primary, vision gate not triggered
        assert_eq!(decision.target, primary);
        assert_eq!(decision.reason, "primary");
    }
}
