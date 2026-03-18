//! LLM Provider implementations

pub mod anthropic;
pub mod google;
pub mod openai;
pub mod openrouter;

pub use anthropic::AnthropicProvider;
pub use google::GoogleAIProvider;
pub use openai::{OpenAIConfig, OpenAIProvider};
pub use openrouter::{OpenRouterConfig, OpenRouterProvider};

/// R3-H12: Shared HTTP client for all LLM providers.
/// Reusing a single `reqwest::Client` shares the connection pool, TLS sessions,
/// and DNS cache across providers instead of each provider maintaining its own.
pub(crate) fn shared_http_client() -> reqwest::Client {
    use std::sync::LazyLock;
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .pool_max_idle_per_host(10)
            .build()
            .expect("failed to create shared HTTP client")
    });
    CLIENT.clone()
}
