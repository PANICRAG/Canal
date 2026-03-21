//! Model Profile and Routing Strategy configuration.
//!
//! This module provides a catalog-based approach to managing LLM model profiles,
//! each of which defines a routing strategy, agent configuration, and caching
//! behavior. Profiles can be loaded from YAML, managed at runtime via a
//! thread-safe catalog, and instantiated from reusable templates.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing;

use crate::error::{Error, Result};

// ============================================================================
// Routing Strategy
// ============================================================================

/// The strategy used to route requests across LLM providers and models.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    /// Route to a primary model; fall back to alternatives on failure.
    PrimaryFallback,
    /// Select a model based on the inferred task type of the request.
    TaskTypeRules,
    /// Delegate routing decisions to a lightweight router LLM.
    RouterAgent,
    /// Split traffic across model variants for experimentation.
    AbTest,
    /// Try models in a tiered cascade, escalating on insufficient quality.
    Cascade,
    /// Route to different models based on detected content modality
    /// (text-only, vision, or hybrid).
    Multimodal,
}

impl Default for RoutingStrategy {
    fn default() -> Self {
        Self::PrimaryFallback
    }
}

// ============================================================================
// Model Target & Provider Capabilities
// ============================================================================

/// A concrete model endpoint on a specific provider.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ModelTarget {
    /// Provider identifier (e.g. "anthropic", "openai", "google").
    pub provider: String,
    /// Model identifier (e.g. "claude-opus-4-5-20251101", "gpt-4o").
    pub model: String,
}

/// Declared capabilities for a provider/model combination.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderCapabilities {
    /// Whether the model supports tool/function calling.
    #[serde(default)]
    pub tool_calling: bool,
    /// Whether the model supports streaming responses.
    #[serde(default)]
    pub streaming: bool,
    /// Whether the model supports vision/image inputs.
    #[serde(default)]
    pub vision: bool,
    /// Maximum context window size in tokens.
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,
}

fn default_max_context_tokens() -> usize {
    128_000
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self {
            tool_calling: false,
            streaming: false,
            vision: false,
            max_context_tokens: default_max_context_tokens(),
        }
    }
}

// ============================================================================
// Strategy-Specific Configuration Types
// ============================================================================

/// A rule that maps a task type pattern to a model target.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TaskTypeRule {
    /// Regex or exact-match pattern for the task type label.
    pub task_pattern: String,
    /// The model to route matching tasks to.
    pub target: ModelTarget,
    /// Optional priority (lower wins). Defaults to 0.
    #[serde(default)]
    pub priority: u32,
}

/// Configuration for the router-agent strategy.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RouterAgentConfig {
    /// The model used as the routing agent itself.
    pub router_model: ModelTarget,
    /// System prompt injected into the routing agent.
    #[serde(default)]
    pub system_prompt: String,
    /// Descriptions of candidate models the agent can choose from.
    #[serde(default)]
    pub candidates: Vec<ModelDescription>,
    /// Maximum latency budget (ms) for the routing decision.
    #[serde(default = "default_router_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_router_timeout_ms() -> u64 {
    3000
}

/// Human-readable description of a model, used by the router agent.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelDescription {
    /// The model target.
    pub target: ModelTarget,
    /// Free-text description of the model's strengths.
    #[serde(default)]
    pub description: String,
    /// Optional capability tags (e.g. "code", "creative", "fast").
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A single variant in an A/B test split.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AbTestVariant {
    /// Human-readable variant name (e.g. "control", "treatment_a").
    pub name: String,
    /// The model target for this variant.
    pub target: ModelTarget,
    /// Traffic weight (all weights are normalised at runtime).
    pub weight: f64,
}

/// A single tier in a cascade strategy.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CascadeTier {
    /// Tier label (e.g. "fast", "balanced", "powerful").
    pub label: String,
    /// The model target for this tier.
    pub target: ModelTarget,
    /// Maximum tokens the model is allowed to generate at this tier.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Quality threshold (0.0-1.0) below which the cascade escalates.
    #[serde(default)]
    pub quality_threshold: Option<f64>,
}

/// Configuration for the multimodal routing strategy.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MultimodalConfig {
    /// Target for text-only requests (e.g., reasoning model).
    pub text_target: ModelTarget,
    /// Target for vision/image requests (e.g., vision-capable model).
    pub vision_target: ModelTarget,
    /// Target for hybrid requests (text + image).
    pub hybrid_target: ModelTarget,
}

/// Full configuration for the cascade routing strategy.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CascadeConfig {
    /// Ordered list of tiers, tried from first to last.
    pub tiers: Vec<CascadeTier>,
    /// Maximum number of tier escalations before giving up.
    #[serde(default = "default_max_escalations")]
    pub max_escalations: u32,
}

fn default_max_escalations() -> u32 {
    3
}

// ============================================================================
// Routing Configuration
// ============================================================================

/// Top-level routing configuration embedded in a `ModelProfile`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RoutingConfig {
    /// The routing strategy to apply.
    #[serde(default)]
    pub strategy: RoutingStrategy,

    // -- PrimaryFallback fields --
    /// Primary model target (used by PrimaryFallback and as a general default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<ModelTarget>,
    /// Ordered fallback targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallbacks: Option<Vec<ModelTarget>>,

    // -- TaskTypeRules fields --
    /// Rules for the TaskTypeRules strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type_rules: Option<Vec<TaskTypeRule>>,
    /// Default target when no task-type rule matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type_default: Option<ModelTarget>,

    // -- RouterAgent fields --
    /// Configuration for the RouterAgent strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_agent: Option<RouterAgentConfig>,

    // -- AbTest fields --
    /// Variants for the AbTest strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ab_test_variants: Option<Vec<AbTestVariant>>,

    // -- Cascade fields --
    /// Configuration for the Cascade strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cascade: Option<CascadeConfig>,

    // -- Multimodal fields --
    /// Configuration for the Multimodal strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multimodal: Option<MultimodalConfig>,

    // -- Common --
    /// If true, callers may override the routed model via an explicit field.
    #[serde(default)]
    pub allow_explicit_model: bool,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            strategy: RoutingStrategy::default(),
            primary: None,
            fallbacks: None,
            task_type_rules: None,
            task_type_default: None,
            router_agent: None,
            ab_test_variants: None,
            cascade: None,
            multimodal: None,
            allow_explicit_model: false,
        }
    }
}

// ============================================================================
// Agent Configuration
// ============================================================================

/// Per-profile agent behaviour configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentConfig {
    /// System prompt prepended to every conversation for this profile.
    #[serde(default)]
    pub system_prompt: String,
    /// Allow-list of tool names the agent is permitted to invoke.
    /// An empty list means all tools are allowed.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Maximum agent loop turns before forcefully stopping.
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    /// Per-request timeout in seconds.
    #[serde(default = "default_task_timeout_seconds")]
    pub task_timeout_seconds: u64,
    /// A38: Override instance-level semantic memory setting for this profile.
    /// `None` inherits instance default, `Some(true/false)` overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_memory: Option<bool>,
}

fn default_max_turns() -> u32 {
    25
}

fn default_task_timeout_seconds() -> u64 {
    300
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            allowed_tools: Vec::new(),
            max_turns: default_max_turns(),
            task_timeout_seconds: default_task_timeout_seconds(),
            semantic_memory: None,
        }
    }
}

// ============================================================================
// Model Profile
// ============================================================================

/// A complete model profile that bundles routing, agent, and caching settings
/// under a single named identity.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelProfile {
    /// Unique identifier for this profile (e.g. "code-assistant-v2").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Free-text description of what this profile is intended for.
    #[serde(default)]
    pub description: String,
    /// Whether the profile is active and eligible for use.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Routing configuration.
    #[serde(default)]
    pub routing: RoutingConfig,
    /// Agent behaviour configuration.
    #[serde(default)]
    pub agent: AgentConfig,
    /// Whether response caching is enabled for this profile.
    #[serde(default)]
    pub cache_enabled: bool,
    /// Cache time-to-live in seconds (only meaningful when caching is on).
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_seconds: u64,
}

fn default_enabled() -> bool {
    true
}

fn default_cache_ttl() -> u64 {
    3600
}

// ============================================================================
// Profile Template
// ============================================================================

/// A reusable template from which new `ModelProfile` instances can be stamped.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProfileTemplate {
    /// Unique template identifier.
    pub template_id: String,
    /// Human-readable name of the template.
    pub name: String,
    /// Description of the template's purpose.
    #[serde(default)]
    pub description: String,
    /// Category tag (e.g. "coding", "chat", "analysis").
    #[serde(default)]
    pub category: String,
    /// The base profile that will be cloned when this template is instantiated.
    pub base_profile: ModelProfile,
}

// ============================================================================
// YAML Serialization Wrapper
// ============================================================================

/// Internal wrapper used for (de)serialising the YAML config file.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct ProfilesConfig {
    #[serde(default)]
    profiles: Vec<ModelProfile>,
    #[serde(default)]
    templates: Vec<ProfileTemplate>,
}

// ============================================================================
// Profile Catalog
// ============================================================================

/// Thread-safe, runtime-mutable catalog of model profiles and templates.
///
/// Profiles and templates can be loaded from a YAML file, queried, mutated,
/// and persisted back to disk.
pub struct ProfileCatalog {
    profiles: Arc<RwLock<HashMap<String, ModelProfile>>>,
    templates: Arc<RwLock<HashMap<String, ProfileTemplate>>>,
    config_path: Option<PathBuf>,
}

impl std::fmt::Debug for ProfileCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileCatalog")
            .field("config_path", &self.config_path)
            .finish()
    }
}

impl ProfileCatalog {
    // -- Constructors --------------------------------------------------------

    /// Load profiles and templates from a YAML file.
    pub async fn from_yaml(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
            Error::Config(format!(
                "Failed to read profile config at {}: {}",
                path.display(),
                e
            ))
        })?;

        let config: ProfilesConfig = serde_yaml::from_str(&content).map_err(|e| {
            Error::Config(format!(
                "Failed to parse profile config at {}: {}",
                path.display(),
                e
            ))
        })?;

        let mut profiles = HashMap::new();
        for profile in config.profiles {
            tracing::info!(id = %profile.id, name = %profile.name, "Loaded model profile");
            profiles.insert(profile.id.clone(), profile);
        }

        let mut templates = HashMap::new();
        for template in config.templates {
            tracing::info!(
                template_id = %template.template_id,
                name = %template.name,
                "Loaded profile template"
            );
            templates.insert(template.template_id.clone(), template);
        }

        Ok(Self {
            profiles: Arc::new(RwLock::new(profiles)),
            templates: Arc::new(RwLock::new(templates)),
            config_path: Some(path),
        })
    }

    /// Create an empty catalog with no backing file.
    pub fn empty() -> Self {
        Self {
            profiles: Arc::new(RwLock::new(HashMap::new())),
            templates: Arc::new(RwLock::new(HashMap::new())),
            config_path: None,
        }
    }

    // -- Profile CRUD --------------------------------------------------------

    /// Retrieve a profile by its `id`.
    pub async fn get(&self, id: &str) -> Result<ModelProfile> {
        let profiles = self.profiles.read().await;
        profiles
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("Model profile not found: {}", id)))
    }

    /// List all profiles currently in the catalog.
    pub async fn list(&self) -> Vec<ModelProfile> {
        let profiles = self.profiles.read().await;
        let mut list: Vec<ModelProfile> = profiles.values().cloned().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }

    /// Insert or update a profile. Returns `true` if an existing profile was
    /// replaced, `false` if a new profile was inserted.
    pub async fn upsert(&self, profile: ModelProfile) -> bool {
        let mut profiles = self.profiles.write().await;
        let replaced = profiles.contains_key(&profile.id);
        tracing::info!(
            id = %profile.id,
            replaced = replaced,
            "Upserted model profile"
        );
        profiles.insert(profile.id.clone(), profile);
        replaced
    }

    /// Remove a profile by its `id`. Returns the removed profile, or an error
    /// if it does not exist.
    pub async fn remove(&self, id: &str) -> Result<ModelProfile> {
        let mut profiles = self.profiles.write().await;
        profiles
            .remove(id)
            .ok_or_else(|| Error::NotFound(format!("Model profile not found: {}", id)))
    }

    // -- Reload & Persist ----------------------------------------------------

    /// Reload profiles and templates from the original YAML path.
    ///
    /// This is a full replace: profiles that existed in memory but not in the
    /// file will be removed.
    pub async fn reload(&self) -> Result<()> {
        let path = self
            .config_path
            .as_ref()
            .ok_or_else(|| Error::Config("No config path set for reload".to_string()))?;

        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            Error::Config(format!(
                "Failed to read profile config at {}: {}",
                path.display(),
                e
            ))
        })?;

        let config: ProfilesConfig = serde_yaml::from_str(&content).map_err(|e| {
            Error::Config(format!(
                "Failed to parse profile config at {}: {}",
                path.display(),
                e
            ))
        })?;

        {
            let mut profiles = self.profiles.write().await;
            profiles.clear();
            for profile in config.profiles {
                profiles.insert(profile.id.clone(), profile);
            }
        }

        {
            let mut templates = self.templates.write().await;
            templates.clear();
            for template in config.templates {
                templates.insert(template.template_id.clone(), template);
            }
        }

        tracing::info!(path = %path.display(), "Reloaded profile catalog from YAML");
        Ok(())
    }

    /// Serialise the current in-memory state back to the YAML file.
    pub async fn save_to_yaml(&self) -> Result<()> {
        let path = self
            .config_path
            .as_ref()
            .ok_or_else(|| Error::Config("No config path set for save".to_string()))?;

        let profiles = self.profiles.read().await;
        let templates = self.templates.read().await;

        let mut sorted_profiles: Vec<ModelProfile> = profiles.values().cloned().collect();
        sorted_profiles.sort_by(|a, b| a.id.cmp(&b.id));

        let mut sorted_templates: Vec<ProfileTemplate> = templates.values().cloned().collect();
        sorted_templates.sort_by(|a, b| a.template_id.cmp(&b.template_id));

        let config = ProfilesConfig {
            profiles: sorted_profiles,
            templates: sorted_templates,
        };

        let yaml = serde_yaml::to_string(&config)
            .map_err(|e| Error::Config(format!("Failed to serialise profile catalog: {}", e)))?;

        tokio::fs::write(path, yaml).await.map_err(|e| {
            Error::Config(format!(
                "Failed to write profile config to {}: {}",
                path.display(),
                e
            ))
        })?;

        tracing::info!(path = %path.display(), "Saved profile catalog to YAML");
        Ok(())
    }

    // -- Template Operations -------------------------------------------------

    /// List all available profile templates.
    pub async fn list_templates(&self) -> Vec<ProfileTemplate> {
        let templates = self.templates.read().await;
        let mut list: Vec<ProfileTemplate> = templates.values().cloned().collect();
        list.sort_by(|a, b| a.template_id.cmp(&b.template_id));
        list
    }

    /// Create a new profile from a template, assigning it the given `new_id`.
    ///
    /// The new profile is automatically inserted into the catalog.
    pub async fn create_from_template(
        &self,
        template_id: &str,
        new_id: &str,
    ) -> Result<ModelProfile> {
        let templates = self.templates.read().await;
        let template = templates.get(template_id).ok_or_else(|| {
            Error::NotFound(format!("Profile template not found: {}", template_id))
        })?;

        let mut profile = template.base_profile.clone();
        profile.id = new_id.to_string();
        // Keep the template's name as a starting point but make it distinguishable.
        if profile.name == template.base_profile.name {
            profile.name = format!("{} ({})", template.name, new_id);
        }

        drop(templates); // release read lock before writing

        tracing::info!(
            template_id = template_id,
            new_id = new_id,
            "Created profile from template"
        );

        self.upsert(profile.clone()).await;
        Ok(profile)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Helper: build a minimal valid profile for testing.
    fn sample_profile(id: &str) -> ModelProfile {
        ModelProfile {
            id: id.to_string(),
            name: format!("Test Profile {}", id),
            description: "A test profile".to_string(),
            enabled: true,
            routing: RoutingConfig {
                strategy: RoutingStrategy::PrimaryFallback,
                primary: Some(ModelTarget {
                    provider: "anthropic".to_string(),
                    model: "claude-opus-4-5-20251101".to_string(),
                }),
                fallbacks: Some(vec![ModelTarget {
                    provider: "openai".to_string(),
                    model: "gpt-4o".to_string(),
                }]),
                ..RoutingConfig::default()
            },
            agent: AgentConfig::default(),
            cache_enabled: false,
            cache_ttl_seconds: 3600,
        }
    }

    fn sample_template() -> ProfileTemplate {
        ProfileTemplate {
            template_id: "tpl-code".to_string(),
            name: "Code Assistant".to_string(),
            description: "Template for code-focused assistants".to_string(),
            category: "coding".to_string(),
            base_profile: sample_profile("__template_base__"),
        }
    }

    fn write_yaml_file(
        profiles: Vec<ModelProfile>,
        templates: Vec<ProfileTemplate>,
    ) -> NamedTempFile {
        let config = ProfilesConfig {
            profiles,
            templates,
        };
        let yaml = serde_yaml::to_string(&config).expect("serialize yaml");
        let mut f = NamedTempFile::new().expect("create temp file");
        f.write_all(yaml.as_bytes()).expect("write yaml");
        f.flush().expect("flush");
        f
    }

    // -- Serde round-trip ----------------------------------------------------

    #[test]
    fn test_routing_strategy_serde() {
        let strategies = vec![
            (RoutingStrategy::PrimaryFallback, "\"primary_fallback\""),
            (RoutingStrategy::TaskTypeRules, "\"task_type_rules\""),
            (RoutingStrategy::RouterAgent, "\"router_agent\""),
            (RoutingStrategy::AbTest, "\"ab_test\""),
            (RoutingStrategy::Cascade, "\"cascade\""),
            (RoutingStrategy::Multimodal, "\"multimodal\""),
        ];
        for (variant, expected_json) in &strategies {
            let json = serde_json::to_string(variant).expect("serialize");
            assert_eq!(
                &json, expected_json,
                "serialization mismatch for {:?}",
                variant
            );
            let back: RoutingStrategy = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&back, variant);
        }
    }

    #[test]
    fn test_model_profile_yaml_round_trip() {
        let profile = sample_profile("round-trip");
        let yaml = serde_yaml::to_string(&profile).expect("to yaml");
        let back: ModelProfile = serde_yaml::from_str(&yaml).expect("from yaml");
        assert_eq!(back.id, "round-trip");
        assert_eq!(back.routing.strategy, RoutingStrategy::PrimaryFallback);
        assert!(back.routing.primary.is_some());
        assert_eq!(back.routing.primary.as_ref().unwrap().provider, "anthropic");
    }

    #[test]
    fn test_profile_template_serde() {
        let tpl = sample_template();
        let json = serde_json::to_string(&tpl).expect("serialize");
        let back: ProfileTemplate = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.template_id, "tpl-code");
        assert_eq!(back.category, "coding");
        assert_eq!(back.base_profile.id, "__template_base__");
    }

    #[test]
    fn test_default_values() {
        let yaml = r#"
id: minimal
name: Minimal Profile
"#;
        let profile: ModelProfile = serde_yaml::from_str(yaml).expect("parse minimal");
        assert!(profile.enabled);
        assert_eq!(profile.routing.strategy, RoutingStrategy::PrimaryFallback);
        assert!(!profile.routing.allow_explicit_model);
        assert_eq!(profile.agent.max_turns, 25);
        assert_eq!(profile.agent.task_timeout_seconds, 300);
        assert_eq!(profile.cache_ttl_seconds, 3600);
        assert!(!profile.cache_enabled);
    }

    #[test]
    fn test_provider_capabilities_defaults() {
        let caps = ProviderCapabilities::default();
        assert!(!caps.tool_calling);
        assert!(!caps.streaming);
        assert!(!caps.vision);
        assert_eq!(caps.max_context_tokens, 128_000);
    }

    #[test]
    fn test_cascade_config_serde() {
        let cascade = CascadeConfig {
            tiers: vec![
                CascadeTier {
                    label: "fast".to_string(),
                    target: ModelTarget {
                        provider: "anthropic".to_string(),
                        model: "claude-haiku-4-5-20251001".to_string(),
                    },
                    max_tokens: Some(1024),
                    quality_threshold: Some(0.7),
                },
                CascadeTier {
                    label: "powerful".to_string(),
                    target: ModelTarget {
                        provider: "anthropic".to_string(),
                        model: "claude-opus-4-5-20251101".to_string(),
                    },
                    max_tokens: None,
                    quality_threshold: None,
                },
            ],
            max_escalations: 2,
        };
        let json = serde_json::to_string(&cascade).expect("serialize");
        let back: CascadeConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.tiers.len(), 2);
        assert_eq!(back.tiers[0].label, "fast");
        assert_eq!(back.max_escalations, 2);
    }

    #[test]
    fn test_ab_test_variant_serde() {
        let variant = AbTestVariant {
            name: "control".to_string(),
            target: ModelTarget {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
            },
            weight: 0.8,
        };
        let yaml = serde_yaml::to_string(&variant).expect("yaml");
        let back: AbTestVariant = serde_yaml::from_str(&yaml).expect("parse");
        assert_eq!(back.name, "control");
        assert!((back.weight - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn test_task_type_rule_serde() {
        let rule = TaskTypeRule {
            task_pattern: "^code_.*".to_string(),
            target: ModelTarget {
                provider: "anthropic".to_string(),
                model: "claude-opus-4-5-20251101".to_string(),
            },
            priority: 10,
        };
        let json = serde_json::to_string(&rule).expect("serialize");
        let back: TaskTypeRule = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.task_pattern, "^code_.*");
        assert_eq!(back.priority, 10);
    }

    #[test]
    fn test_router_agent_config_defaults() {
        let yaml = r#"
router_model:
  provider: anthropic
  model: claude-haiku-4-5-20251001
"#;
        let cfg: RouterAgentConfig = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(cfg.timeout_ms, 3000);
        assert!(cfg.system_prompt.is_empty());
        assert!(cfg.candidates.is_empty());
    }

    // -- Catalog tests (async) -----------------------------------------------

    #[tokio::test]
    async fn test_catalog_empty() {
        let catalog = ProfileCatalog::empty();
        let profiles = catalog.list().await;
        assert!(profiles.is_empty());
    }

    #[tokio::test]
    async fn test_catalog_upsert_and_get() {
        let catalog = ProfileCatalog::empty();

        let profile = sample_profile("alpha");
        let replaced = catalog.upsert(profile.clone()).await;
        assert!(!replaced, "first insert should not be a replace");

        let fetched = catalog.get("alpha").await.expect("should find alpha");
        assert_eq!(fetched.id, "alpha");
        assert_eq!(fetched.name, profile.name);

        // Update
        let mut updated = profile.clone();
        updated.name = "Updated Name".to_string();
        let replaced = catalog.upsert(updated).await;
        assert!(replaced, "second insert should replace");

        let fetched = catalog.get("alpha").await.unwrap();
        assert_eq!(fetched.name, "Updated Name");
    }

    #[tokio::test]
    async fn test_catalog_remove() {
        let catalog = ProfileCatalog::empty();
        catalog.upsert(sample_profile("removable")).await;

        let removed = catalog.remove("removable").await;
        assert!(removed.is_ok());
        assert_eq!(removed.unwrap().id, "removable");

        let err = catalog.remove("removable").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn test_catalog_get_not_found() {
        let catalog = ProfileCatalog::empty();
        let result = catalog.get("nonexistent").await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not found"),
            "expected NotFound error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_catalog_list_sorted() {
        let catalog = ProfileCatalog::empty();
        catalog.upsert(sample_profile("charlie")).await;
        catalog.upsert(sample_profile("alpha")).await;
        catalog.upsert(sample_profile("bravo")).await;

        let list = catalog.list().await;
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].id, "alpha");
        assert_eq!(list[1].id, "bravo");
        assert_eq!(list[2].id, "charlie");
    }

    #[tokio::test]
    async fn test_catalog_from_yaml() {
        let p1 = sample_profile("yaml-one");
        let p2 = sample_profile("yaml-two");
        let tpl = sample_template();
        let file = write_yaml_file(vec![p1, p2], vec![tpl]);

        let catalog = ProfileCatalog::from_yaml(file.path())
            .await
            .expect("load yaml");

        let profiles = catalog.list().await;
        assert_eq!(profiles.len(), 2);

        let templates = catalog.list_templates().await;
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].template_id, "tpl-code");
    }

    #[tokio::test]
    async fn test_catalog_from_yaml_invalid_path() {
        let result = ProfileCatalog::from_yaml("/nonexistent/path.yaml").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_catalog_reload() {
        let p1 = sample_profile("reload-one");
        let file = write_yaml_file(vec![p1], vec![]);

        let catalog = ProfileCatalog::from_yaml(file.path())
            .await
            .expect("initial load");
        assert_eq!(catalog.list().await.len(), 1);

        // Overwrite the file with two profiles
        let new_config = ProfilesConfig {
            profiles: vec![sample_profile("reload-a"), sample_profile("reload-b")],
            templates: vec![],
        };
        let yaml = serde_yaml::to_string(&new_config).unwrap();
        std::fs::write(file.path(), yaml).expect("overwrite");

        catalog.reload().await.expect("reload");
        let list = catalog.list().await;
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|p| p.id == "reload-a"));
        assert!(list.iter().any(|p| p.id == "reload-b"));
    }

    #[tokio::test]
    async fn test_catalog_reload_no_path() {
        let catalog = ProfileCatalog::empty();
        let result = catalog.reload().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_catalog_save_to_yaml() {
        let file = write_yaml_file(vec![], vec![]);
        let catalog = ProfileCatalog::from_yaml(file.path()).await.expect("load");

        catalog.upsert(sample_profile("saved-one")).await;
        catalog.upsert(sample_profile("saved-two")).await;

        catalog.save_to_yaml().await.expect("save");

        // Re-load and verify
        let reloaded = ProfileCatalog::from_yaml(file.path())
            .await
            .expect("reload");
        let list = reloaded.list().await;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "saved-one");
        assert_eq!(list[1].id, "saved-two");
    }

    #[tokio::test]
    async fn test_catalog_save_no_path() {
        let catalog = ProfileCatalog::empty();
        let result = catalog.save_to_yaml().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_catalog_create_from_template() {
        let catalog = ProfileCatalog::empty();
        // Manually insert a template
        {
            let mut templates = catalog.templates.write().await;
            templates.insert("tpl-code".to_string(), sample_template());
        }

        let created = catalog
            .create_from_template("tpl-code", "my-code-agent")
            .await
            .expect("create from template");

        assert_eq!(created.id, "my-code-agent");
        assert!(created.name.contains("my-code-agent"));

        // Should also be in the catalog now
        let fetched = catalog.get("my-code-agent").await.expect("should exist");
        assert_eq!(fetched.id, "my-code-agent");
    }

    #[tokio::test]
    async fn test_catalog_create_from_template_not_found() {
        let catalog = ProfileCatalog::empty();
        let result = catalog.create_from_template("nonexistent", "new-id").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_routing_config_all_strategies() {
        // PrimaryFallback
        let pf_yaml = r#"
strategy: primary_fallback
primary:
  provider: anthropic
  model: claude-opus-4-5-20251101
fallbacks:
  - provider: openai
    model: gpt-4o
allow_explicit_model: true
"#;
        let pf: RoutingConfig = serde_yaml::from_str(pf_yaml).expect("parse pf");
        assert_eq!(pf.strategy, RoutingStrategy::PrimaryFallback);
        assert!(pf.primary.is_some());
        assert!(pf.allow_explicit_model);

        // TaskTypeRules
        let ttr_yaml = r#"
strategy: task_type_rules
task_type_rules:
  - task_pattern: "^code_"
    target:
      provider: anthropic
      model: claude-opus-4-5-20251101
    priority: 1
task_type_default:
  provider: openai
  model: gpt-4o-mini
"#;
        let ttr: RoutingConfig = serde_yaml::from_str(ttr_yaml).expect("parse ttr");
        assert_eq!(ttr.strategy, RoutingStrategy::TaskTypeRules);
        assert_eq!(ttr.task_type_rules.as_ref().unwrap().len(), 1);

        // AbTest
        let ab_yaml = r#"
strategy: ab_test
ab_test_variants:
  - name: control
    target:
      provider: anthropic
      model: claude-opus-4-5-20251101
    weight: 0.5
  - name: treatment
    target:
      provider: openai
      model: gpt-4o
    weight: 0.5
"#;
        let ab: RoutingConfig = serde_yaml::from_str(ab_yaml).expect("parse ab");
        assert_eq!(ab.strategy, RoutingStrategy::AbTest);
        assert_eq!(ab.ab_test_variants.as_ref().unwrap().len(), 2);

        // Cascade
        let cascade_yaml = r#"
strategy: cascade
cascade:
  tiers:
    - label: fast
      target:
        provider: anthropic
        model: claude-haiku-4-5-20251001
      quality_threshold: 0.6
    - label: powerful
      target:
        provider: anthropic
        model: claude-opus-4-5-20251101
  max_escalations: 2
"#;
        let cascade: RoutingConfig = serde_yaml::from_str(cascade_yaml).expect("parse cascade");
        assert_eq!(cascade.strategy, RoutingStrategy::Cascade);
        assert_eq!(cascade.cascade.as_ref().unwrap().tiers.len(), 2);
    }

    #[test]
    fn test_profiles_config_full_round_trip() {
        let config = ProfilesConfig {
            profiles: vec![sample_profile("full-rt")],
            templates: vec![sample_template()],
        };
        let yaml = serde_yaml::to_string(&config).expect("to yaml");
        let back: ProfilesConfig = serde_yaml::from_str(&yaml).expect("from yaml");
        assert_eq!(back.profiles.len(), 1);
        assert_eq!(back.templates.len(), 1);
        assert_eq!(back.profiles[0].id, "full-rt");
        assert_eq!(back.templates[0].template_id, "tpl-code");
    }

    #[test]
    fn test_agent_config_defaults() {
        let agent = AgentConfig::default();
        assert!(agent.system_prompt.is_empty());
        assert!(agent.allowed_tools.is_empty());
        assert_eq!(agent.max_turns, 25);
        assert_eq!(agent.task_timeout_seconds, 300);
    }

    #[test]
    fn test_multimodal_config_serde() {
        let config = MultimodalConfig {
            text_target: ModelTarget {
                provider: "qwen".to_string(),
                model: "qwen3-max".to_string(),
            },
            vision_target: ModelTarget {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
            },
            hybrid_target: ModelTarget {
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4-20250514".to_string(),
            },
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let back: MultimodalConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.text_target.provider, "qwen");
        assert_eq!(back.vision_target.model, "gpt-4o");
        assert_eq!(back.hybrid_target.provider, "anthropic");
    }

    #[test]
    fn test_vision_profile_deserialization() {
        let yaml = r#"
id: vision-aware
name: Vision Aware
description: Routes to vision-capable models when images detected
enabled: true
routing:
  strategy: multimodal
  multimodal:
    text_target:
      provider: qwen
      model: qwen3-max
    vision_target:
      provider: openai
      model: gpt-4o
    hybrid_target:
      provider: anthropic
      model: claude-sonnet-4-20250514
"#;
        let profile: ModelProfile = serde_yaml::from_str(yaml).expect("parse multimodal profile");
        assert_eq!(profile.id, "vision-aware");
        assert_eq!(profile.routing.strategy, RoutingStrategy::Multimodal);
        assert!(profile.routing.multimodal.is_some());
        let mm = profile.routing.multimodal.unwrap();
        assert_eq!(mm.text_target.provider, "qwen");
        assert_eq!(mm.text_target.model, "qwen3-max");
        assert_eq!(mm.vision_target.provider, "openai");
        assert_eq!(mm.vision_target.model, "gpt-4o");
        assert_eq!(mm.hybrid_target.provider, "anthropic");
        assert_eq!(mm.hybrid_target.model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_routing_config_multimodal() {
        let yaml = r#"
strategy: multimodal
multimodal:
  text_target:
    provider: qwen
    model: qwen3-max
  vision_target:
    provider: openai
    model: gpt-4o
  hybrid_target:
    provider: anthropic
    model: claude-sonnet-4-20250514
"#;
        let config: RoutingConfig = serde_yaml::from_str(yaml).expect("parse multimodal routing");
        assert_eq!(config.strategy, RoutingStrategy::Multimodal);
        assert!(config.multimodal.is_some());
        let mm = config.multimodal.unwrap();
        assert_eq!(mm.text_target.model, "qwen3-max");
        assert_eq!(mm.vision_target.model, "gpt-4o");
        assert_eq!(mm.hybrid_target.model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_agent_config_custom() {
        let yaml = r#"
system_prompt: "You are a coding assistant."
allowed_tools:
  - filesystem_read
  - code_execute
max_turns: 50
task_timeout_seconds: 600
"#;
        let agent: AgentConfig = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(agent.system_prompt, "You are a coding assistant.");
        assert_eq!(agent.allowed_tools.len(), 2);
        assert_eq!(agent.max_turns, 50);
        assert_eq!(agent.task_timeout_seconds, 600);
    }
}
