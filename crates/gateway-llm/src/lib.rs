//! LLM routing crate
//!
//! Provides LLM routing, provider abstraction, model profiles,
//! health tracking, cost tracking, and routing strategies.
//!
//! Extracted from `gateway-core::llm` as a standalone crate for
//! faster compilation and independent versioning.

pub mod cost_tracker;
pub mod error;
pub mod health;
pub mod model_profile;
pub mod model_registry;
pub mod providers;
pub mod router;
pub mod routing_engine;
pub mod strategies;

pub use error::{Error, Result};

pub use router::{
    ChatRequest, ChatResponse, LlmConfig, LlmProvider, LlmRouter, Message, StreamChunk,
    StreamResponse, Usage,
};
pub use routing_engine::RoutingEngine;

// Re-export commonly used types from submodules
pub use cost_tracker::{InternalCostTracker, ModelUsageRecord};
pub use health::{CircuitState, HealthConfig, HealthTracker, ProviderHealthSnapshot};
pub use model_profile::{ProfileCatalog, ProfileTemplate};
pub use strategies::{
    AbTestVariant, CascadeTier, ModelCapabilities, ModelProfile, ModelRegistry, ModelTarget,
    MultimodalRoutingConfig, RouteDecision, RoutingConfig, RoutingStrategyHandler, TaskTypeRule,
};

/// Register LLM providers from environment variables into the given router.
///
/// Checks for API keys in env vars and registers the corresponding providers.
/// Returns a list of registered provider names.
///
/// Supported providers:
/// - `ANTHROPIC_API_KEY` → Anthropic (Claude)
/// - `GOOGLE_AI_API_KEY` → Google AI (Gemini)
/// - `OPENAI_API_KEY` → OpenAI (GPT)
/// - `QWEN_API_KEY` → Qwen (DashScope), set as default if present
pub fn register_providers_from_env(router: &mut LlmRouter) -> Vec<String> {
    use std::sync::Arc;
    let mut registered = Vec::new();

    // Anthropic
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        router.register_provider("anthropic", Arc::new(providers::AnthropicProvider::new()));
        registered.push("anthropic".to_string());
        tracing::info!("Registered Anthropic provider");
    }

    // Google AI (Gemini)
    if std::env::var("GOOGLE_AI_API_KEY").is_ok() {
        router.register_provider("google", Arc::new(providers::GoogleAIProvider::new()));
        registered.push("google".to_string());
        tracing::info!("Registered Google AI (Gemini) provider");
    }

    // OpenAI
    if std::env::var("OPENAI_API_KEY").is_ok() {
        router.register_provider("openai", Arc::new(providers::OpenAIProvider::new()));
        registered.push("openai".to_string());
        tracing::info!("Registered OpenAI provider");
    }

    // OpenRouter — multi-provider gateway
    if std::env::var("OPENROUTER_API_KEY").is_ok() {
        router.register_provider("openrouter", Arc::new(providers::OpenRouterProvider::new()));
        registered.push("openrouter".to_string());
        tracing::info!("Registered OpenRouter provider");
    }

    // Qwen (Alibaba Cloud DashScope) — uses OpenAI-compatible API
    if let Ok(qwen_key) = std::env::var("QWEN_API_KEY") {
        let qwen_config = providers::OpenAIConfig {
            api_key: qwen_key,
            base_url: std::env::var("QWEN_BASE_URL").unwrap_or_else(|_| {
                "https://dashscope-intl.aliyuncs.com/compatible-mode".to_string()
            }),
            default_model: std::env::var("QWEN_DEFAULT_MODEL")
                .unwrap_or_else(|_| "qwen3-max-2026-01-23".to_string()),
            organization: None,
            name: "qwen".to_string(),
        };
        router.register_provider(
            "qwen",
            Arc::new(providers::OpenAIProvider::with_config(qwen_config)),
        );
        router.set_default_provider("qwen");
        registered.push("qwen".to_string());
        tracing::info!("Registered Qwen provider (DashScope) as default");
    }

    registered
}
