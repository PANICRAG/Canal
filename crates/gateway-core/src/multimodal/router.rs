//! Multimodal routing strategy.
//!
//! Routes requests to different models based on detected content modality.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::llm::router::ChatRequest;
use crate::llm::strategies::{
    ModelProfile, ModelRegistry, ModelTarget, RouteDecision, RoutingStrategyHandler,
};
use crate::llm::Result;

use super::{ContentModality, MultimodalDetector};

/// Configuration for multimodal routing within a model profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultimodalRoutingConfig {
    /// Target for text-only requests (e.g., reasoning model).
    pub text_target: ModelTarget,
    /// Target for vision-only or hybrid requests (e.g., vision model).
    pub vision_target: ModelTarget,
    /// Fallback target that handles both modalities.
    pub hybrid_target: ModelTarget,
}

/// Routes requests to different models based on detected content modality.
///
/// Uses a `MultimodalDetector` to classify the request, then selects
/// the appropriate model target from the configuration.
pub struct MultimodalRoutingStrategy {
    detector: MultimodalDetector,
    config: MultimodalRoutingConfig,
}

impl MultimodalRoutingStrategy {
    /// Create a new multimodal routing strategy.
    pub fn new(config: MultimodalRoutingConfig) -> Self {
        Self {
            detector: MultimodalDetector::with_defaults(),
            config,
        }
    }

    /// Create with a custom detector.
    pub fn with_detector(config: MultimodalRoutingConfig, detector: MultimodalDetector) -> Self {
        Self { detector, config }
    }
}

#[async_trait]
impl RoutingStrategyHandler for MultimodalRoutingStrategy {
    #[tracing::instrument(
        skip(self, _profile, request, _registry),
        fields(modality, target_model)
    )]
    async fn resolve(
        &self,
        _profile: &ModelProfile,
        request: &ChatRequest,
        _registry: &ModelRegistry,
    ) -> Result<RouteDecision> {
        let modality = self.detector.detect(request);

        let (target, fallback, reason) = match modality {
            ContentModality::TextOnly => (
                self.config.text_target.clone(),
                Some(self.config.hybrid_target.clone()),
                "multimodal:text_only",
            ),
            ContentModality::VisionOnly => (
                self.config.vision_target.clone(),
                Some(self.config.hybrid_target.clone()),
                "multimodal:vision_only",
            ),
            ContentModality::Hybrid => (
                self.config.hybrid_target.clone(),
                Some(self.config.vision_target.clone()),
                "multimodal:hybrid",
            ),
        };

        tracing::Span::current().record("modality", tracing::field::debug(&modality));
        tracing::Span::current().record("target_model", target.model.as_str());

        Ok(RouteDecision {
            target,
            reason: reason.into(),
            fallback,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::router::{ContentBlock, Message};

    fn test_config() -> MultimodalRoutingConfig {
        MultimodalRoutingConfig {
            text_target: ModelTarget {
                provider: "openai".into(),
                model: "gpt-4o".into(),
            },
            vision_target: ModelTarget {
                provider: "openai".into(),
                model: "gpt-4o-vision".into(),
            },
            hybrid_target: ModelTarget {
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
            },
        }
    }

    fn text_request(text: &str) -> ChatRequest {
        ChatRequest {
            messages: vec![Message::text("user", text)],
            ..Default::default()
        }
    }

    fn profile() -> ModelProfile {
        ModelProfile {
            name: "test".into(),
            description: "test profile".into(),
            routing: Default::default(),
        }
    }

    #[tokio::test]
    async fn test_routes_text_to_text_model() {
        let strategy = MultimodalRoutingStrategy::new(test_config());
        let decision = strategy
            .resolve(
                &profile(),
                &text_request("Hello world"),
                &ModelRegistry::new(),
            )
            .await
            .unwrap();

        assert_eq!(decision.target.model, "gpt-4o");
        assert_eq!(decision.reason, "multimodal:text_only");
        assert!(decision.fallback.is_some());
    }

    #[tokio::test]
    async fn test_routes_vision_keyword_to_hybrid() {
        let strategy = MultimodalRoutingStrategy::new(test_config());
        let decision = strategy
            .resolve(
                &profile(),
                &text_request("Take a screenshot"),
                &ModelRegistry::new(),
            )
            .await
            .unwrap();

        assert_eq!(decision.target.model, "claude-sonnet-4-6");
        assert_eq!(decision.reason, "multimodal:hybrid");
    }

    #[tokio::test]
    async fn test_routes_image_to_vision_model() {
        let strategy = MultimodalRoutingStrategy::new(test_config());
        let req = ChatRequest {
            messages: vec![Message {
                role: "user".into(),
                content: String::new(),
                content_blocks: vec![ContentBlock::Image {
                    source_type: "base64".into(),
                    media_type: "image/png".into(),
                    data: "test".into(),
                }],
            }],
            ..Default::default()
        };

        let decision = strategy
            .resolve(&profile(), &req, &ModelRegistry::new())
            .await
            .unwrap();

        assert_eq!(decision.target.model, "gpt-4o-vision");
        assert_eq!(decision.reason, "multimodal:vision_only");
    }

    #[tokio::test]
    async fn test_routes_browser_task_type_to_hybrid() {
        let strategy = MultimodalRoutingStrategy::new(test_config());
        let mut req = text_request("Click the button");
        req.task_type = Some("browser".into());

        let decision = strategy
            .resolve(&profile(), &req, &ModelRegistry::new())
            .await
            .unwrap();

        assert_eq!(decision.target.model, "claude-sonnet-4-6");
        assert_eq!(decision.reason, "multimodal:hybrid");
    }

    #[tokio::test]
    async fn test_fallback_always_provided() {
        let strategy = MultimodalRoutingStrategy::new(test_config());

        for req in [text_request("Hello"), text_request("Take a screenshot")] {
            let decision = strategy
                .resolve(&profile(), &req, &ModelRegistry::new())
                .await
                .unwrap();
            assert!(decision.fallback.is_some(), "Fallback should always be set");
        }
    }

    #[tokio::test]
    async fn test_text_fallback_is_hybrid_target() {
        let strategy = MultimodalRoutingStrategy::new(test_config());
        let decision = strategy
            .resolve(&profile(), &text_request("Hello"), &ModelRegistry::new())
            .await
            .unwrap();

        let fallback = decision.fallback.unwrap();
        assert_eq!(fallback.model, "claude-sonnet-4-6");
        assert_eq!(fallback.provider, "anthropic");
    }

    #[tokio::test]
    async fn test_hybrid_fallback_is_vision_target() {
        let strategy = MultimodalRoutingStrategy::new(test_config());
        let decision = strategy
            .resolve(
                &profile(),
                &text_request("Take a screenshot"),
                &ModelRegistry::new(),
            )
            .await
            .unwrap();

        let fallback = decision.fallback.unwrap();
        assert_eq!(fallback.model, "gpt-4o-vision");
        assert_eq!(fallback.provider, "openai");
    }

    #[tokio::test]
    async fn test_custom_detector() {
        let detector = MultimodalDetector::empty();
        let strategy = MultimodalRoutingStrategy::with_detector(test_config(), detector);

        // "screenshot" would normally trigger hybrid, but empty detector returns text_only
        let decision = strategy
            .resolve(
                &profile(),
                &text_request("Take a screenshot"),
                &ModelRegistry::new(),
            )
            .await
            .unwrap();

        assert_eq!(decision.target.model, "gpt-4o");
        assert_eq!(decision.reason, "multimodal:text_only");
    }

    #[tokio::test]
    async fn test_image_plus_text_routes_to_hybrid() {
        let strategy = MultimodalRoutingStrategy::new(test_config());
        let req = ChatRequest {
            messages: vec![Message {
                role: "user".into(),
                content: "What is in this image?".into(),
                content_blocks: vec![ContentBlock::Image {
                    source_type: "base64".into(),
                    media_type: "image/png".into(),
                    data: "test".into(),
                }],
            }],
            ..Default::default()
        };

        let decision = strategy
            .resolve(&profile(), &req, &ModelRegistry::new())
            .await
            .unwrap();

        assert_eq!(decision.target.model, "claude-sonnet-4-6");
        assert_eq!(decision.reason, "multimodal:hybrid");
    }

    #[tokio::test]
    async fn test_config_serialization() {
        let config = test_config();
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: MultimodalRoutingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.text_target.model, "gpt-4o");
        assert_eq!(deserialized.vision_target.model, "gpt-4o-vision");
        assert_eq!(deserialized.hybrid_target.model, "claude-sonnet-4-6");
    }
}
