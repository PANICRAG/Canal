//! Platform Context Loader
//!
//! Loads and manages platform-level context configuration from YAML files.
//! Platform context defines global rules that apply to all agents, including:
//! - Language requirements
//! - Iteration/learning protocols
//! - System prompt injection rules
//! - Context hierarchy definitions
//! - Skill loading configuration

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

// ============================================================================
// Language Configuration
// ============================================================================

/// Language configuration for generated content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageConfig {
    /// Default language code (e.g., "en").
    #[serde(default = "default_language")]
    pub default: String,

    /// Whether to enforce English for all outputs.
    #[serde(default)]
    pub enforce_english: bool,

    /// System prompt rule for language requirements.
    #[serde(default)]
    pub system_prompt_rule: String,
}

fn default_language() -> String {
    "en".to_string()
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self {
            default: default_language(),
            enforce_english: false,
            system_prompt_rule: String::new(),
        }
    }
}

// ============================================================================
// Iteration Configuration
// ============================================================================

/// A single step in the learning loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningLoopStep {
    /// Step identifier.
    pub step: String,

    /// Human-readable description of the step.
    #[serde(default)]
    pub description: String,
}

/// Issue recording configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueRecordingConfig {
    /// Whether to deduplicate issues before recording.
    #[serde(default)]
    pub deduplicate: bool,

    /// Minimum similarity threshold (0-1) for considering issues as duplicates.
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f32,

    /// Output format for recorded issues.
    #[serde(default)]
    pub format: String,

    /// Required fields for issue recording.
    #[serde(default)]
    pub required_fields: Vec<String>,

    /// Optional fields for issue recording.
    #[serde(default)]
    pub optional_fields: Vec<String>,
}

fn default_similarity_threshold() -> f32 {
    0.8
}

impl Default for IssueRecordingConfig {
    fn default() -> Self {
        Self {
            deduplicate: false,
            similarity_threshold: default_similarity_threshold(),
            format: String::new(),
            required_fields: Vec::new(),
            optional_fields: Vec::new(),
        }
    }
}

/// Self-iteration learning protocol configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationConfig {
    /// Whether iteration is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Maximum retry attempts before giving up.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Whether to automatically record issues on failure.
    #[serde(default)]
    pub auto_record: bool,

    /// Learning loop steps.
    #[serde(default)]
    pub learning_loop: Vec<LearningLoopStep>,

    /// Issue recording configuration.
    #[serde(default)]
    pub issue_recording: IssueRecordingConfig,
}

fn default_max_retries() -> u32 {
    3
}

impl Default for IterationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_retries: default_max_retries(),
            auto_record: true,
            learning_loop: Vec::new(),
            issue_recording: IssueRecordingConfig::default(),
        }
    }
}

// ============================================================================
// System Prompt Configuration
// ============================================================================

/// System prompt injection configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemPromptConfig {
    /// Platform-level rules injected into all agent system prompts.
    #[serde(default)]
    pub platform_rules: String,

    /// Skill-related system prompt rules.
    #[serde(default)]
    pub skill_rules: String,
}

// ============================================================================
// Context Hierarchy Configuration
// ============================================================================

/// A single layer in the context hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextLayer {
    /// Layer name (e.g., "platform", "organization", "user").
    pub name: String,

    /// Human-readable description of the layer.
    #[serde(default)]
    pub description: String,

    /// Scope identifier.
    #[serde(default)]
    pub scope: String,

    /// Priority level (lower = higher priority).
    #[serde(default = "default_priority")]
    pub priority: u32,
}

fn default_priority() -> u32 {
    100
}

/// Context hierarchy configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextHierarchyConfig {
    /// Ordered list of context layers.
    #[serde(default)]
    pub layers: Vec<ContextLayer>,
}

impl ContextHierarchyConfig {
    /// Get layers sorted by priority (ascending).
    pub fn sorted_layers(&self) -> Vec<&ContextLayer> {
        let mut layers: Vec<_> = self.layers.iter().collect();
        layers.sort_by_key(|l| l.priority);
        layers
    }

    /// Find a layer by name.
    pub fn get_layer(&self, name: &str) -> Option<&ContextLayer> {
        self.layers.iter().find(|l| l.name == name)
    }
}

// ============================================================================
// Skill Loading Configuration
// ============================================================================

/// Skill loading configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillLoadingConfig {
    /// Whether to use two-layer loading (descriptions always visible, full content on-demand).
    #[serde(default)]
    pub two_layer: bool,

    /// Maximum description size in characters for system prompt.
    #[serde(default = "default_max_description_chars")]
    pub max_description_chars: usize,

    /// Directory containing skill files.
    #[serde(default = "default_skill_dir")]
    pub skill_dir: String,

    /// Whether to automatically match skills to user intent.
    #[serde(default)]
    pub auto_match: bool,

    /// Keyword matching threshold (0-1).
    #[serde(default = "default_keyword_threshold")]
    pub keyword_threshold: f32,
}

fn default_max_description_chars() -> usize {
    15000
}

fn default_skill_dir() -> String {
    ".agent/skills".to_string()
}

fn default_keyword_threshold() -> f32 {
    0.6
}

impl Default for SkillLoadingConfig {
    fn default() -> Self {
        Self {
            two_layer: true,
            max_description_chars: default_max_description_chars(),
            skill_dir: default_skill_dir(),
            auto_match: true,
            keyword_threshold: default_keyword_threshold(),
        }
    }
}

// ============================================================================
// Platform Context
// ============================================================================

/// Complete platform context configuration.
///
/// This is the top-level structure that contains all platform-wide settings
/// loaded from the `platform-rules.yaml` configuration file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformContext {
    /// Language configuration.
    #[serde(default)]
    pub language: LanguageConfig,

    /// Iteration/learning protocol configuration.
    #[serde(default)]
    pub iteration: IterationConfig,

    /// System prompt injection configuration.
    #[serde(default)]
    pub system_prompt: SystemPromptConfig,

    /// Context hierarchy configuration.
    #[serde(default)]
    pub context_hierarchy: ContextHierarchyConfig,

    /// Skill loading configuration.
    #[serde(default)]
    pub skill_loading: SkillLoadingConfig,
}

impl Default for PlatformContext {
    fn default() -> Self {
        Self {
            language: LanguageConfig::default(),
            iteration: IterationConfig::default(),
            system_prompt: SystemPromptConfig::default(),
            context_hierarchy: ContextHierarchyConfig::default(),
            skill_loading: SkillLoadingConfig::default(),
        }
    }
}

impl PlatformContext {
    /// Check if English is enforced for all outputs.
    pub fn is_english_enforced(&self) -> bool {
        self.language.enforce_english
    }

    /// Check if iteration learning is enabled.
    pub fn is_iteration_enabled(&self) -> bool {
        self.iteration.enabled
    }

    /// Get the combined platform rules for system prompt injection.
    pub fn get_platform_rules(&self) -> String {
        let mut rules = String::new();

        if !self.system_prompt.platform_rules.is_empty() {
            rules.push_str(&self.system_prompt.platform_rules);
        }

        if !self.system_prompt.skill_rules.is_empty() {
            if !rules.is_empty() {
                rules.push_str("\n\n");
            }
            rules.push_str(&self.system_prompt.skill_rules);
        }

        rules
    }

    /// Generate the iteration/learning loop rules for system prompt.
    ///
    /// Returns an empty string if iteration is disabled.
    pub fn get_iteration_rules(&self) -> String {
        if !self.iteration.enabled {
            return String::new();
        }

        let mut rules = String::new();
        rules.push_str("## Self-Iteration Learning Protocol\n\n");
        rules.push_str("When a task fails, follow this loop:\n");

        for (i, step) in self.iteration.learning_loop.iter().enumerate() {
            rules.push_str(&format!("{}. {}\n", i + 1, step.step.to_uppercase()));
            if !step.description.is_empty() {
                rules.push_str(&format!("   {}\n", step.description));
            }
        }

        rules.push_str(&format!("\nMax retries: {}\n", self.iteration.max_retries));

        if self.iteration.auto_record {
            rules.push_str("Auto-record new issues: enabled\n");
        }

        rules
    }

    /// Generate the context hierarchy rules.
    ///
    /// Returns a formatted string describing the context priority layers.
    pub fn get_context_rules(&self) -> String {
        let mut rules = String::new();
        rules.push_str("## Context Priority\n\n");
        rules.push_str(
            "Rules are applied in priority order. Higher priority cannot be overridden:\n\n",
        );

        for layer in self.context_hierarchy.sorted_layers() {
            rules.push_str(&format!(
                "{}. **{}** ({}): {}\n",
                layer.priority,
                layer.name,
                layer.scope,
                if layer.description.is_empty() {
                    ""
                } else {
                    &layer.description
                }
            ));
        }

        rules
    }

    /// Generate complete platform section for system prompt.
    ///
    /// Combines language rules, system prompt rules, and iteration rules
    /// into a single formatted section.
    pub fn generate_platform_section(&self) -> String {
        let mut section = String::new();

        // Language rules
        section.push_str("# Platform Rules\n\n");
        section.push_str(&self.language.system_prompt_rule);
        section.push_str("\n\n");

        // System prompt rules
        section.push_str(&self.system_prompt.platform_rules);
        section.push_str("\n\n");

        // Iteration rules
        section.push_str(&self.get_iteration_rules());
        section.push('\n');

        section
    }

    /// Get skill loading configuration.
    pub fn skill_loading_config(&self) -> &SkillLoadingConfig {
        &self.skill_loading
    }

    /// Check if two-layer skill loading is enabled.
    pub fn is_two_layer_loading_enabled(&self) -> bool {
        self.skill_loading.two_layer
    }

    /// Get max description chars for skill descriptions.
    pub fn max_skill_description_chars(&self) -> usize {
        self.skill_loading.max_description_chars
    }
}

// ============================================================================
// ContextLayer Implementation
// ============================================================================

impl super::resolver::ContextLayer for PlatformContext {
    fn layer_name(&self) -> &str {
        "platform"
    }

    fn priority(&self) -> super::resolver::ContextPriority {
        super::resolver::ContextPriority::Platform
    }

    fn apply_to(&self, resolved: &mut super::resolver::ResolvedContext) {
        // Apply platform rules
        resolved.platform_rules = self.generate_platform_section();

        // Apply language enforcement
        resolved.enforce_english = self.language.enforce_english;

        // Apply config values from platform
        resolved.config_values.insert(
            "max_skill_description_chars".to_string(),
            serde_json::json!(self.skill_loading.max_description_chars),
        );
        resolved.config_values.insert(
            "two_layer_loading".to_string(),
            serde_json::json!(self.skill_loading.two_layer),
        );
        resolved.config_values.insert(
            "iteration_enabled".to_string(),
            serde_json::json!(self.iteration.enabled),
        );
        resolved.config_values.insert(
            "max_retries".to_string(),
            serde_json::json!(self.iteration.max_retries),
        );
    }
}

// ============================================================================
// Platform Context Loader
// ============================================================================

/// Loader for platform context configuration from YAML files.
///
/// # Example
///
/// ```rust,ignore
/// use gateway_core::agent::context::PlatformContextLoader;
///
/// let loader = PlatformContextLoader::new("config/platform-rules.yaml");
/// let context = loader.load()?;
///
/// // Generate system prompt injection
/// let prompt = loader.get_system_prompt_injection(&context);
/// println!("{}", prompt);
/// ```
pub struct PlatformContextLoader {
    config_path: PathBuf,
}

impl PlatformContextLoader {
    /// Create a new platform context loader.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the platform rules YAML configuration file.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            config_path: path.as_ref().to_path_buf(),
        }
    }

    /// Get the configuration file path.
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Load platform context from the YAML file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub fn load(&self) -> Result<PlatformContext> {
        let content = std::fs::read_to_string(&self.config_path).map_err(|e| {
            Error::Config(format!(
                "Failed to read platform rules at {}: {}",
                self.config_path.display(),
                e
            ))
        })?;

        serde_yaml::from_str(&content).map_err(|e| {
            Error::Config(format!(
                "Failed to parse platform rules at {}: {}",
                self.config_path.display(),
                e
            ))
        })
    }

    /// Load platform context asynchronously.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub async fn load_async(&self) -> Result<PlatformContext> {
        let content = tokio::fs::read_to_string(&self.config_path)
            .await
            .map_err(|e| {
                Error::Config(format!(
                    "Failed to read platform rules at {}: {}",
                    self.config_path.display(),
                    e
                ))
            })?;

        serde_yaml::from_str(&content).map_err(|e| {
            Error::Config(format!(
                "Failed to parse platform rules at {}: {}",
                self.config_path.display(),
                e
            ))
        })
    }

    /// Generate the platform rules section for system prompt injection.
    ///
    /// This creates a formatted string suitable for injection into
    /// an agent's system prompt.
    pub fn get_system_prompt_injection(&self, ctx: &PlatformContext) -> String {
        let mut prompt = String::new();

        // Add platform rules header
        prompt.push_str("## Platform Rules\n\n");

        // Add language requirement if enforced
        if ctx.language.enforce_english && !ctx.language.system_prompt_rule.is_empty() {
            prompt.push_str(&ctx.language.system_prompt_rule);
            prompt.push('\n');
        }

        // Add platform-specific rules
        if !ctx.system_prompt.platform_rules.is_empty() {
            if !prompt.ends_with('\n') {
                prompt.push('\n');
            }
            prompt.push_str(&ctx.system_prompt.platform_rules);
        }

        prompt
    }

    /// Generate a complete system prompt with all platform context.
    ///
    /// This includes language rules, platform rules, and skill rules.
    pub fn get_full_system_prompt(&self, ctx: &PlatformContext) -> String {
        let mut prompt = String::new();

        // Language section
        if ctx.language.enforce_english && !ctx.language.system_prompt_rule.is_empty() {
            prompt.push_str("## Language Requirements\n\n");
            prompt.push_str(&ctx.language.system_prompt_rule);
            prompt.push_str("\n\n");
        }

        // Platform rules section
        if !ctx.system_prompt.platform_rules.is_empty() {
            prompt.push_str(&ctx.system_prompt.platform_rules);
            prompt.push_str("\n\n");
        }

        // Skill rules section
        if !ctx.system_prompt.skill_rules.is_empty() {
            prompt.push_str(&ctx.system_prompt.skill_rules);
        }

        prompt.trim_end().to_string()
    }

    /// Check if the configuration file exists.
    pub fn exists(&self) -> bool {
        self.config_path.exists()
    }

    /// Load platform context with a fallback to defaults if the file doesn't exist.
    pub fn load_or_default(&self) -> PlatformContext {
        if self.exists() {
            self.load().unwrap_or_default()
        } else {
            PlatformContext::default()
        }
    }

    /// Load platform context asynchronously with a fallback to defaults.
    pub async fn load_or_default_async(&self) -> PlatformContext {
        if self.exists() {
            self.load_async().await.unwrap_or_default()
        } else {
            PlatformContext::default()
        }
    }

    /// Generate the full system prompt with platform rules and skill descriptions.
    ///
    /// This method combines the platform section with optional skill descriptions,
    /// respecting the two-layer loading configuration.
    ///
    /// # Arguments
    ///
    /// * `platform` - The platform context to use for generation.
    /// * `skill_descriptions` - Optional skill descriptions to include.
    pub fn generate_full_system_prompt(
        &self,
        platform: &PlatformContext,
        skill_descriptions: &str,
    ) -> String {
        let mut prompt = String::new();

        // Add platform section
        prompt.push_str(&platform.generate_platform_section());

        // Add skill descriptions if two-layer loading is enabled
        if platform.is_two_layer_loading_enabled() && !skill_descriptions.is_empty() {
            prompt.push('\n');
            prompt.push_str(skill_descriptions);
            prompt.push('\n');
        }

        prompt
    }
}

impl std::fmt::Debug for PlatformContextLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformContextLoader")
            .field("config_path", &self.config_path)
            .finish()
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

    fn create_test_yaml() -> NamedTempFile {
        let yaml = r#"
language:
  default: "en"
  enforce_english: true
  system_prompt_rule: |
    All responses must be in English.

iteration:
  enabled: true
  max_retries: 5
  auto_record: true
  learning_loop:
    - step: "execute"
      description: "Execute tool"
    - step: "verify"
      description: "Verify result"
  issue_recording:
    deduplicate: true
    similarity_threshold: 0.85
    format: "minimal"
    required_fields:
      - symptom
      - solution

system_prompt:
  platform_rules: |
    1. Always verify actions
    2. Log all changes
  skill_rules: |
    Check skill docs first

context_hierarchy:
  layers:
    - name: "platform"
      description: "Global rules"
      scope: "all"
      priority: 1
    - name: "user"
      description: "User preferences"
      scope: "user"
      priority: 3

skill_loading:
  two_layer: true
  max_description_chars: 10000
  skill_dir: ".skills"
  auto_match: true
  keyword_threshold: 0.7
"#;
        let mut file = NamedTempFile::new().expect("create temp file");
        file.write_all(yaml.as_bytes()).expect("write yaml");
        file.flush().expect("flush");
        file
    }

    #[test]
    fn test_language_config_defaults() {
        let config = LanguageConfig::default();
        assert_eq!(config.default, "en");
        assert!(!config.enforce_english);
        assert!(config.system_prompt_rule.is_empty());
    }

    #[test]
    fn test_iteration_config_defaults() {
        let config = IterationConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_retries, 3);
        assert!(config.auto_record);
        assert!(config.learning_loop.is_empty());
    }

    #[test]
    fn test_skill_loading_defaults() {
        let config = SkillLoadingConfig::default();
        assert!(config.two_layer);
        assert_eq!(config.max_description_chars, 15000);
        assert_eq!(config.skill_dir, ".agent/skills");
        assert!(config.auto_match);
        assert!((config.keyword_threshold - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn test_issue_recording_defaults() {
        let config = IssueRecordingConfig::default();
        assert!(!config.deduplicate);
        assert!((config.similarity_threshold - 0.8).abs() < f32::EPSILON);
        assert!(config.format.is_empty());
    }

    #[test]
    fn test_platform_context_defaults() {
        let ctx = PlatformContext::default();
        assert!(!ctx.is_english_enforced());
        assert!(ctx.is_iteration_enabled());
        assert!(ctx.get_platform_rules().is_empty());
    }

    #[test]
    fn test_load_platform_context() {
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());

        let ctx = loader.load().expect("load should succeed");

        // Language
        assert_eq!(ctx.language.default, "en");
        assert!(ctx.language.enforce_english);
        assert!(ctx.language.system_prompt_rule.contains("English"));

        // Iteration
        assert!(ctx.iteration.enabled);
        assert_eq!(ctx.iteration.max_retries, 5);
        assert!(ctx.iteration.auto_record);
        assert_eq!(ctx.iteration.learning_loop.len(), 2);
        assert_eq!(ctx.iteration.learning_loop[0].step, "execute");

        // Issue recording
        assert!(ctx.iteration.issue_recording.deduplicate);
        assert!((ctx.iteration.issue_recording.similarity_threshold - 0.85).abs() < f32::EPSILON);
        assert_eq!(ctx.iteration.issue_recording.required_fields.len(), 2);

        // System prompt
        assert!(ctx.system_prompt.platform_rules.contains("verify"));
        assert!(ctx.system_prompt.skill_rules.contains("skill docs"));

        // Context hierarchy
        assert_eq!(ctx.context_hierarchy.layers.len(), 2);
        assert_eq!(ctx.context_hierarchy.layers[0].name, "platform");
        assert_eq!(ctx.context_hierarchy.layers[0].priority, 1);

        // Skill loading
        assert!(ctx.skill_loading.two_layer);
        assert_eq!(ctx.skill_loading.max_description_chars, 10000);
        assert_eq!(ctx.skill_loading.skill_dir, ".skills");
        assert!((ctx.skill_loading.keyword_threshold - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn test_context_hierarchy_sorted() {
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());
        let ctx = loader.load().expect("load");

        let sorted = ctx.context_hierarchy.sorted_layers();
        assert_eq!(sorted.len(), 2);
        assert_eq!(sorted[0].name, "platform");
        assert_eq!(sorted[1].name, "user");
    }

    #[test]
    fn test_context_hierarchy_get_layer() {
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());
        let ctx = loader.load().expect("load");

        let layer = ctx.context_hierarchy.get_layer("platform");
        assert!(layer.is_some());
        assert_eq!(layer.unwrap().priority, 1);

        let missing = ctx.context_hierarchy.get_layer("nonexistent");
        assert!(missing.is_none());
    }

    #[test]
    fn test_get_system_prompt_injection() {
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());
        let ctx = loader.load().expect("load");

        let prompt = loader.get_system_prompt_injection(&ctx);
        assert!(prompt.contains("## Platform Rules"));
        assert!(prompt.contains("English"));
        assert!(prompt.contains("verify"));
    }

    #[test]
    fn test_get_full_system_prompt() {
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());
        let ctx = loader.load().expect("load");

        let prompt = loader.get_full_system_prompt(&ctx);
        assert!(prompt.contains("Language Requirements"));
        assert!(prompt.contains("English"));
        assert!(prompt.contains("skill docs"));
    }

    #[test]
    fn test_get_platform_rules() {
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());
        let ctx = loader.load().expect("load");

        let rules = ctx.get_platform_rules();
        assert!(rules.contains("verify"));
        assert!(rules.contains("skill docs"));
    }

    #[test]
    fn test_loader_exists() {
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());
        assert!(loader.exists());

        let missing_loader = PlatformContextLoader::new("/nonexistent/path.yaml");
        assert!(!missing_loader.exists());
    }

    #[test]
    fn test_load_or_default() {
        // Existing file
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());
        let ctx = loader.load_or_default();
        assert!(ctx.language.enforce_english);

        // Missing file
        let missing_loader = PlatformContextLoader::new("/nonexistent/path.yaml");
        let default_ctx = missing_loader.load_or_default();
        assert!(!default_ctx.language.enforce_english);
    }

    #[test]
    fn test_load_invalid_yaml() {
        let mut file = NamedTempFile::new().expect("create temp file");
        file.write_all(b"invalid: yaml: content: [").expect("write");
        file.flush().expect("flush");

        let loader = PlatformContextLoader::new(file.path());
        let result = loader.load();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parse"));
    }

    #[test]
    fn test_load_missing_file() {
        let loader = PlatformContextLoader::new("/nonexistent/path.yaml");
        let result = loader.load();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("read"));
    }

    #[tokio::test]
    async fn test_load_async() {
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());

        let ctx = loader.load_async().await.expect("async load");
        assert!(ctx.language.enforce_english);
        assert_eq!(ctx.iteration.max_retries, 5);
    }

    #[tokio::test]
    async fn test_load_or_default_async() {
        // Existing file
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());
        let ctx = loader.load_or_default_async().await;
        assert!(ctx.language.enforce_english);

        // Missing file
        let missing_loader = PlatformContextLoader::new("/nonexistent/path.yaml");
        let default_ctx = missing_loader.load_or_default_async().await;
        assert!(!default_ctx.language.enforce_english);
    }

    #[test]
    fn test_serde_round_trip() {
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());
        let ctx = loader.load().expect("load");

        // Serialize to YAML
        let yaml = serde_yaml::to_string(&ctx).expect("serialize");

        // Deserialize back
        let ctx2: PlatformContext = serde_yaml::from_str(&yaml).expect("deserialize");

        assert_eq!(ctx.language.default, ctx2.language.default);
        assert_eq!(ctx.language.enforce_english, ctx2.language.enforce_english);
        assert_eq!(ctx.iteration.max_retries, ctx2.iteration.max_retries);
        assert_eq!(
            ctx.context_hierarchy.layers.len(),
            ctx2.context_hierarchy.layers.len()
        );
    }

    #[test]
    fn test_minimal_yaml() {
        let yaml = r#"
language:
  default: "ja"
"#;
        let mut file = NamedTempFile::new().expect("create temp file");
        file.write_all(yaml.as_bytes()).expect("write");
        file.flush().expect("flush");

        let loader = PlatformContextLoader::new(file.path());
        let ctx = loader.load().expect("load minimal");

        // Specified value
        assert_eq!(ctx.language.default, "ja");

        // Defaults applied
        assert!(!ctx.language.enforce_english);
        assert!(ctx.iteration.enabled);
        assert_eq!(ctx.iteration.max_retries, 3);
        assert!(ctx.skill_loading.two_layer);
    }

    #[test]
    fn test_debug_impl() {
        let loader = PlatformContextLoader::new("test/path.yaml");
        let debug = format!("{:?}", loader);
        assert!(debug.contains("PlatformContextLoader"));
        assert!(debug.contains("test/path.yaml"));
    }

    // ========================================================================
    // Tests for new prompt generation methods
    // ========================================================================

    fn create_test_platform_context() -> PlatformContext {
        PlatformContext {
            language: LanguageConfig {
                default: "en".to_string(),
                enforce_english: true,
                system_prompt_rule: "All output in English.".to_string(),
            },
            iteration: IterationConfig {
                enabled: true,
                max_retries: 3,
                auto_record: true,
                learning_loop: vec![
                    LearningLoopStep {
                        step: "execute".to_string(),
                        description: String::new(),
                    },
                    LearningLoopStep {
                        step: "verify".to_string(),
                        description: "Check results".to_string(),
                    },
                ],
                issue_recording: IssueRecordingConfig::default(),
            },
            system_prompt: SystemPromptConfig {
                platform_rules: "Verify after each action.".to_string(),
                skill_rules: String::new(),
            },
            context_hierarchy: ContextHierarchyConfig::default(),
            skill_loading: SkillLoadingConfig::default(),
        }
    }

    #[test]
    fn test_iteration_rules_generation() {
        let platform = create_test_platform_context();
        let rules = platform.get_iteration_rules();

        assert!(rules.contains("Self-Iteration"));
        assert!(rules.contains("EXECUTE"));
        assert!(rules.contains("VERIFY"));
        assert!(rules.contains("Max retries"));
        assert!(rules.contains("3"));
        assert!(rules.contains("Auto-record new issues: enabled"));
    }

    #[test]
    fn test_iteration_rules_disabled() {
        let mut platform = create_test_platform_context();
        platform.iteration.enabled = false;

        let rules = platform.get_iteration_rules();
        assert!(rules.is_empty());
    }

    #[test]
    fn test_iteration_rules_with_description() {
        let platform = create_test_platform_context();
        let rules = platform.get_iteration_rules();

        // The "verify" step has a description
        assert!(rules.contains("Check results"));
    }

    #[test]
    fn test_context_rules_generation() {
        let mut platform = create_test_platform_context();
        platform.context_hierarchy = ContextHierarchyConfig {
            layers: vec![
                ContextLayer {
                    name: "platform".to_string(),
                    description: "Global rules".to_string(),
                    scope: "all".to_string(),
                    priority: 1,
                },
                ContextLayer {
                    name: "user".to_string(),
                    description: "User preferences".to_string(),
                    scope: "user".to_string(),
                    priority: 3,
                },
            ],
        };

        let rules = platform.get_context_rules();

        assert!(rules.contains("Context Priority"));
        assert!(rules.contains("**platform**"));
        assert!(rules.contains("**user**"));
        assert!(rules.contains("Global rules"));
        assert!(rules.contains("User preferences"));
    }

    #[test]
    fn test_platform_section_generation() {
        let platform = create_test_platform_context();
        let section = platform.generate_platform_section();

        assert!(section.contains("Platform Rules"));
        assert!(section.contains("English"));
        assert!(section.contains("Verify after each action"));
        assert!(section.contains("Self-Iteration"));
    }

    #[test]
    fn test_skill_loading_accessors() {
        let platform = create_test_platform_context();

        assert!(platform.is_two_layer_loading_enabled());
        assert_eq!(platform.max_skill_description_chars(), 15000);

        let config = platform.skill_loading_config();
        assert!(config.two_layer);
        assert!(config.auto_match);
    }

    #[test]
    fn test_generate_full_system_prompt() {
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());
        let platform = create_test_platform_context();

        let skill_descriptions =
            "## Available Skills\n\n- commit: Create git commits\n- review: Review code";

        let prompt = loader.generate_full_system_prompt(&platform, skill_descriptions);

        // Should contain platform section
        assert!(prompt.contains("Platform Rules"));
        assert!(prompt.contains("English"));

        // Should contain skill descriptions since two_layer is enabled
        assert!(prompt.contains("Available Skills"));
        assert!(prompt.contains("commit"));
        assert!(prompt.contains("review"));
    }

    #[test]
    fn test_generate_full_system_prompt_no_skills() {
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());
        let platform = create_test_platform_context();

        let prompt = loader.generate_full_system_prompt(&platform, "");

        // Should contain platform section
        assert!(prompt.contains("Platform Rules"));

        // Should not have extra newlines for empty skills
        assert!(!prompt.ends_with("\n\n\n"));
    }

    #[test]
    fn test_generate_full_system_prompt_two_layer_disabled() {
        let file = create_test_yaml();
        let loader = PlatformContextLoader::new(file.path());
        let mut platform = create_test_platform_context();
        platform.skill_loading.two_layer = false;

        let skill_descriptions = "## Available Skills\n\n- commit: Create git commits";

        let prompt = loader.generate_full_system_prompt(&platform, skill_descriptions);

        // Should contain platform section
        assert!(prompt.contains("Platform Rules"));

        // Should NOT contain skill descriptions since two_layer is disabled
        assert!(!prompt.contains("Available Skills"));
    }
}
