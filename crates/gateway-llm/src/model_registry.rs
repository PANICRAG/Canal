//! Model Registry
//!
//! Maps model identifiers to provider names via exact matches and prefix-based
//! lookups, and tracks per-provider capability metadata.

use std::collections::HashMap;

use crate::model_profile::ProviderCapabilities;

/// Registry that resolves model identifiers to provider names and stores
/// per-provider capability metadata.
#[derive(Debug, Clone)]
pub struct ModelRegistry {
    /// Exact model_id to provider mapping.
    exact: HashMap<String, String>,
    /// Prefix to provider mapping (checked when no exact match is found).
    /// Sorted by descending prefix length at query time so the longest
    /// matching prefix wins.
    prefixes: Vec<(String, String)>,
    /// Per-provider capability descriptors.
    capabilities: HashMap<String, ProviderCapabilities>,
}

impl ModelRegistry {
    /// Create an empty registry with no mappings.
    pub fn new() -> Self {
        Self {
            exact: HashMap::new(),
            prefixes: Vec::new(),
            capabilities: HashMap::new(),
        }
    }

    /// Create a registry pre-populated with well-known model prefixes and
    /// provider capabilities.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();

        // -- prefix mappings --------------------------------------------------
        registry.register_prefix("claude-", "anthropic");
        registry.register_prefix("gemini-", "google");
        registry.register_prefix("qwen", "qwen");
        registry.register_prefix("gpt-", "openai");
        registry.register_prefix("o1", "openai");
        registry.register_prefix("o3", "openai");

        // -- provider capabilities --------------------------------------------
        registry.register_capabilities(
            "anthropic",
            ProviderCapabilities {
                tool_calling: true,
                streaming: true,
                vision: true,
                max_context_tokens: 200_000,
            },
        );

        // R3-M: Updated capabilities to match current provider features
        registry.register_capabilities(
            "google",
            ProviderCapabilities {
                tool_calling: true,
                streaming: true,
                vision: true,
                max_context_tokens: 1_000_000,
            },
        );

        registry.register_capabilities(
            "qwen",
            ProviderCapabilities {
                tool_calling: true,
                streaming: true,
                vision: true,
                max_context_tokens: 256_000,
            },
        );

        registry.register_capabilities(
            "openai",
            ProviderCapabilities {
                tool_calling: true,
                streaming: true,
                vision: true,
                max_context_tokens: 128_000,
            },
        );

        registry
    }

    /// Register an exact model_id to provider mapping.
    pub fn register(&mut self, model_id: impl Into<String>, provider: impl Into<String>) {
        self.exact.insert(model_id.into(), provider.into());
    }

    /// Register a prefix to provider mapping.
    pub fn register_prefix(&mut self, prefix: impl Into<String>, provider: impl Into<String>) {
        self.prefixes.push((prefix.into(), provider.into()));
    }

    /// Resolve a model identifier to a provider name.
    ///
    /// Resolution order:
    /// 1. Exact match in the `exact` map.
    /// 2. Longest matching prefix in the `prefixes` list.
    ///
    /// Returns `None` if no mapping is found.
    pub fn resolve(&self, model_id: &str) -> Option<&str> {
        // 1. exact match
        if let Some(provider) = self.exact.get(model_id) {
            return Some(provider.as_str());
        }

        // 2. longest prefix match
        let mut best: Option<&str> = None;
        let mut best_len: usize = 0;

        for (prefix, provider) in &self.prefixes {
            if model_id.starts_with(prefix.as_str()) && prefix.len() > best_len {
                best_len = prefix.len();
                best = Some(provider.as_str());
            }
        }

        best
    }

    /// Register (or replace) capability metadata for a provider.
    pub fn register_capabilities(
        &mut self,
        provider: impl Into<String>,
        caps: ProviderCapabilities,
    ) {
        self.capabilities.insert(provider.into(), caps);
    }

    /// Retrieve capability metadata for a provider.
    pub fn get_capabilities(&self, provider: &str) -> Option<&ProviderCapabilities> {
        self.capabilities.get(provider)
    }

    /// Return the names of all providers whose capabilities satisfy the given
    /// requirements.
    ///
    /// A provider is included when:
    /// - `needs_tool_calling` is `true` **and** the provider supports tool calling, **or**
    ///   `needs_tool_calling` is `false`.
    /// - `needs_vision` is `true` **and** the provider supports vision, **or**
    ///   `needs_vision` is `false`.
    pub fn providers_with_capability(
        &self,
        needs_tool_calling: bool,
        needs_vision: bool,
    ) -> Vec<String> {
        self.capabilities
            .iter()
            .filter(|(_, caps)| {
                (!needs_tool_calling || caps.tool_calling) && (!needs_vision || caps.vision)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- resolve tests --------------------------------------------------------

    #[test]
    fn test_resolve_exact_match() {
        let mut registry = ModelRegistry::new();
        registry.register("my-custom-model", "custom-provider");

        assert_eq!(registry.resolve("my-custom-model"), Some("custom-provider"));
        assert_eq!(registry.resolve("unknown"), None);
    }

    #[test]
    fn test_resolve_exact_takes_priority_over_prefix() {
        let mut registry = ModelRegistry::new();
        registry.register_prefix("claude-", "anthropic");
        registry.register("claude-special", "custom-provider");

        // Exact match should win even though the prefix also matches.
        assert_eq!(registry.resolve("claude-special"), Some("custom-provider"));
        // Other claude models still resolve via prefix.
        assert_eq!(registry.resolve("claude-3-opus"), Some("anthropic"));
    }

    #[test]
    fn test_resolve_prefix_match_defaults() {
        let registry = ModelRegistry::with_defaults();

        assert_eq!(registry.resolve("claude-3-opus"), Some("anthropic"));
        assert_eq!(registry.resolve("claude-3-5-sonnet"), Some("anthropic"));
        assert_eq!(registry.resolve("gemini-1.5-pro"), Some("google"));
        assert_eq!(registry.resolve("gpt-4o"), Some("openai"));
        assert_eq!(registry.resolve("o1-preview"), Some("openai"));
        assert_eq!(registry.resolve("o3-mini"), Some("openai"));
        assert_eq!(registry.resolve("qwen2.5-72b"), Some("qwen"));
    }

    #[test]
    fn test_resolve_longest_prefix_wins() {
        let mut registry = ModelRegistry::new();
        registry.register_prefix("gpt-", "openai");
        registry.register_prefix("gpt-4", "openai-v4");

        // "gpt-4" is a longer prefix than "gpt-" so it should win.
        assert_eq!(registry.resolve("gpt-4o"), Some("openai-v4"));
        // "gpt-3.5-turbo" only matches "gpt-".
        assert_eq!(registry.resolve("gpt-3.5-turbo"), Some("openai"));
    }

    #[test]
    fn test_resolve_no_match() {
        let registry = ModelRegistry::with_defaults();
        assert_eq!(registry.resolve("llama-3-70b"), None);
    }

    // -- capabilities tests ---------------------------------------------------

    #[test]
    fn test_get_capabilities() {
        let registry = ModelRegistry::with_defaults();

        let anthropic = registry.get_capabilities("anthropic").unwrap();
        assert!(anthropic.tool_calling);
        assert!(anthropic.streaming);
        assert!(anthropic.vision);
        assert_eq!(anthropic.max_context_tokens, 200_000);

        // R3-M: Updated to match current provider capabilities
        let google = registry.get_capabilities("google").unwrap();
        assert!(google.tool_calling);
        assert!(google.streaming);
        assert!(google.vision);
        assert_eq!(google.max_context_tokens, 1_000_000);

        let qwen = registry.get_capabilities("qwen").unwrap();
        assert!(qwen.tool_calling);
        assert!(qwen.streaming);
        assert!(qwen.vision);
        assert_eq!(qwen.max_context_tokens, 256_000);

        let openai = registry.get_capabilities("openai").unwrap();
        assert!(openai.tool_calling);
        assert!(openai.streaming);
        assert!(openai.vision);
        assert_eq!(openai.max_context_tokens, 128_000);

        assert!(registry.get_capabilities("nonexistent").is_none());
    }

    #[test]
    fn test_register_capabilities_overwrites() {
        let mut registry = ModelRegistry::with_defaults();

        registry.register_capabilities(
            "anthropic",
            ProviderCapabilities {
                tool_calling: false,
                streaming: false,
                vision: false,
                max_context_tokens: 42,
            },
        );

        let caps = registry.get_capabilities("anthropic").unwrap();
        assert!(!caps.tool_calling);
        assert_eq!(caps.max_context_tokens, 42);
    }

    // -- providers_with_capability tests --------------------------------------

    #[test]
    fn test_providers_with_capability_tool_calling() {
        let registry = ModelRegistry::with_defaults();

        let mut providers = registry.providers_with_capability(true, false);
        providers.sort();
        // R3-M: All providers now support tool_calling
        assert_eq!(providers, vec!["anthropic", "google", "openai", "qwen"]);
    }

    #[test]
    fn test_providers_with_capability_vision() {
        let registry = ModelRegistry::with_defaults();

        let mut providers = registry.providers_with_capability(false, true);
        providers.sort();
        // R3-M: All providers now support vision
        assert_eq!(providers, vec!["anthropic", "google", "openai", "qwen"]);
    }

    #[test]
    fn test_providers_with_capability_both() {
        let registry = ModelRegistry::with_defaults();

        let mut providers = registry.providers_with_capability(true, true);
        providers.sort();
        // R3-M: All providers now support both tool_calling and vision
        assert_eq!(providers, vec!["anthropic", "google", "openai", "qwen"]);
    }

    #[test]
    fn test_providers_with_capability_neither() {
        let registry = ModelRegistry::with_defaults();

        let mut providers = registry.providers_with_capability(false, false);
        providers.sort();
        // All providers match when no requirements are specified
        assert_eq!(providers, vec!["anthropic", "google", "openai", "qwen"]);
    }

    // -- Default trait --------------------------------------------------------

    #[test]
    fn test_default_is_with_defaults() {
        let registry = ModelRegistry::default();
        // Should behave identically to with_defaults()
        assert_eq!(registry.resolve("claude-3-opus"), Some("anthropic"));
        assert!(registry.get_capabilities("anthropic").is_some());
    }
}
