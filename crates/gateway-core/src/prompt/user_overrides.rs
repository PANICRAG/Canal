//! User-Editable Prompt Overrides
//!
//! This module provides user-editable prompt customizations that are stored locally
//! and can be modified through the App UI. These preferences are persisted to
//! `~/.canal/prompt_overrides.yaml`.
//!
//! # Example
//!
//! ```rust,ignore
//! use gateway_core::prompt::UserPromptOverrides;
//!
//! // Load existing overrides or create defaults
//! let mut overrides = UserPromptOverrides::load().unwrap_or_default();
//!
//! // Add custom instructions
//! overrides.custom_instructions = Some("Always explain step by step.".to_string());
//!
//! // Save to disk
//! overrides.save().unwrap();
//! ```

use crate::agent::context::PromptSection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::error::{Error, Result};

/// User-editable prompt overrides (stored locally, editable via App UI)
///
/// These overrides allow users to customize their prompt experience without
/// modifying system configuration files. Changes are persisted to the user's
/// home directory.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserPromptOverrides {
    /// Custom instructions appended to system prompt
    ///
    /// These instructions are injected into the prompt after the standard
    /// sections but before task-specific content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,

    /// Sections to disable
    ///
    /// Any section listed here will be omitted from the generated prompt.
    /// Note: Some sections (like Platform) may be enforced and cannot be disabled.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub disabled_sections: HashSet<PromptSectionRef>,

    /// Custom token budgets per section
    ///
    /// Override the default token allocation for specific sections.
    /// Values are approximate token counts.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub token_budgets: HashMap<PromptSectionRef, usize>,

    /// Custom few-shot examples
    ///
    /// User-defined examples that demonstrate desired behavior patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_examples: Vec<CustomExample>,

    /// Tool usage preferences
    #[serde(default)]
    pub tool_preferences: ToolPreferences,

    /// Active constraint profile name
    ///
    /// References a profile from `config/constraints/`. If None, uses the
    /// default profile for the current task type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,
}

/// Serializable reference to a PromptSection
///
/// This type is used for serialization since `PromptSection` contains
/// `Custom(u8)` which needs special handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSectionRef {
    /// Platform rules (L1)
    Platform,
    /// Organization conventions (L2)
    Organization,
    /// User preferences (L3)
    User,
    /// Memory context (A38) - semantic recall, custom instructions, learned patterns
    Memory,
    /// Session context (L4)
    Session,
    /// Skill descriptions
    SkillDescriptions,
    /// Loaded skill content
    LoadedSkills,
    /// Task instructions (L5)
    Task,
    /// SubAgent context (L6)
    SubAgent,
    /// Tool permissions
    ToolPermissions,
    /// Custom section with ID
    Custom(u8),
}

impl From<PromptSection> for PromptSectionRef {
    fn from(section: PromptSection) -> Self {
        match section {
            PromptSection::Platform => Self::Platform,
            PromptSection::Organization => Self::Organization,
            PromptSection::User => Self::User,
            PromptSection::Memory => Self::Memory,
            PromptSection::Session => Self::Session,
            PromptSection::SkillDescriptions => Self::SkillDescriptions,
            PromptSection::LoadedSkills => Self::LoadedSkills,
            PromptSection::Task => Self::Task,
            PromptSection::SubAgent => Self::SubAgent,
            PromptSection::ToolPermissions => Self::ToolPermissions,
            PromptSection::Custom(id) => Self::Custom(id),
        }
    }
}

impl From<PromptSectionRef> for PromptSection {
    fn from(section: PromptSectionRef) -> Self {
        match section {
            PromptSectionRef::Platform => Self::Platform,
            PromptSectionRef::Organization => Self::Organization,
            PromptSectionRef::User => Self::User,
            PromptSectionRef::Memory => Self::Memory,
            PromptSectionRef::Session => Self::Session,
            PromptSectionRef::SkillDescriptions => Self::SkillDescriptions,
            PromptSectionRef::LoadedSkills => Self::LoadedSkills,
            PromptSectionRef::Task => Self::Task,
            PromptSectionRef::SubAgent => Self::SubAgent,
            PromptSectionRef::ToolPermissions => Self::ToolPermissions,
            PromptSectionRef::Custom(id) => Self::Custom(id),
        }
    }
}

/// A custom few-shot example for the prompt
///
/// Examples help guide the LLM's behavior by demonstrating desired
/// input/output patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomExample {
    /// Unique identifier for this example
    pub id: String,

    /// Display name for UI
    pub name: String,

    /// User message in the example
    pub user_message: String,

    /// Assistant response in the example
    pub assistant_response: String,

    /// Tags for categorization and filtering
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Whether this example is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for CustomExample {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: String::new(),
            user_message: String::new(),
            assistant_response: String::new(),
            tags: Vec::new(),
            enabled: true,
        }
    }
}

/// Tool usage preferences
///
/// Allows users to customize which tools the LLM should prefer or avoid.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolPreferences {
    /// Tools to emphasize in prompts
    ///
    /// These tools will be highlighted as preferred options.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferred_tools: Vec<String>,

    /// Tools to de-emphasize or warn about
    ///
    /// The LLM will be instructed to avoid these tools when alternatives exist.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub avoided_tools: Vec<String>,

    /// Include tool usage examples in prompt
    ///
    /// When enabled, includes example invocations for preferred tools.
    #[serde(default)]
    pub include_tool_examples: bool,
}

impl UserPromptOverrides {
    /// Load overrides from local storage
    ///
    /// Reads from `~/.canal/prompt_overrides.yaml`. If the file doesn't exist,
    /// returns default overrides. If the file exists but is invalid, returns an error.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let overrides = UserPromptOverrides::load()?;
    /// ```
    pub fn load() -> Result<Self> {
        let path = Self::storage_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path).map_err(|e| {
                Error::Config(format!(
                    "Failed to read prompt overrides from {}: {}",
                    path.display(),
                    e
                ))
            })?;
            serde_yaml::from_str(&content).map_err(|e| {
                Error::Config(format!(
                    "Failed to parse prompt overrides from {}: {}",
                    path.display(),
                    e
                ))
            })
        } else {
            Ok(Self::default())
        }
    }

    /// Save overrides to local storage
    ///
    /// Writes to `~/.canal/prompt_overrides.yaml`. Creates the parent
    /// directory if it doesn't exist.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let overrides = UserPromptOverrides::default();
    /// overrides.save()?;
    /// ```
    pub fn save(&self) -> Result<()> {
        let path = Self::storage_path();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Error::Config(format!(
                    "Failed to create directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        let content = serde_yaml::to_string(self)
            .map_err(|e| Error::Config(format!("Failed to serialize prompt overrides: {}", e)))?;

        // Atomic write: write to temp file then rename to avoid partial writes
        let temp_path = path.with_extension("yaml.tmp");
        std::fs::write(&temp_path, &content).map_err(|e| {
            Error::Config(format!(
                "Failed to write prompt overrides temp file {}: {}",
                temp_path.display(),
                e
            ))
        })?;
        std::fs::rename(&temp_path, &path).map_err(|e| {
            // Clean up temp file on rename failure
            let _ = std::fs::remove_file(&temp_path);
            Error::Config(format!(
                "Failed to rename prompt overrides to {}: {}",
                path.display(),
                e
            ))
        })?;

        Ok(())
    }

    /// Get storage path (`~/.canal/prompt_overrides.yaml`)
    ///
    /// Returns the path where user overrides are stored. Uses the `dirs` crate
    /// to locate the user's home directory.
    pub fn storage_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".canal")
            .join("prompt_overrides.yaml")
    }

    /// Add a custom example
    ///
    /// Generates a unique ID for the example if not already set.
    pub fn add_example(&mut self, mut example: CustomExample) {
        if example.id.is_empty() {
            example.id = uuid::Uuid::new_v4().to_string();
        }
        self.custom_examples.push(example);
    }

    /// Remove an example by ID
    ///
    /// Returns true if an example was removed, false if not found.
    pub fn remove_example(&mut self, id: &str) -> bool {
        let len_before = self.custom_examples.len();
        self.custom_examples.retain(|e| e.id != id);
        self.custom_examples.len() != len_before
    }

    /// Get enabled examples only
    pub fn enabled_examples(&self) -> impl Iterator<Item = &CustomExample> {
        self.custom_examples.iter().filter(|e| e.enabled)
    }

    /// Get examples by tag
    pub fn examples_by_tag(&self, tag: &str) -> Vec<&CustomExample> {
        let tag_string = tag.to_string();
        self.custom_examples
            .iter()
            .filter(|e| e.tags.contains(&tag_string))
            .collect()
    }

    /// Check if a section is disabled
    pub fn is_section_disabled(&self, section: PromptSection) -> bool {
        self.disabled_sections.contains(&section.into())
    }

    /// Disable a section
    pub fn disable_section(&mut self, section: PromptSection) {
        self.disabled_sections.insert(section.into());
    }

    /// Enable a section (remove from disabled set)
    pub fn enable_section(&mut self, section: PromptSection) {
        self.disabled_sections
            .remove(&PromptSectionRef::from(section));
    }

    /// Set token budget for a section
    pub fn set_token_budget(&mut self, section: PromptSection, budget: usize) {
        self.token_budgets.insert(section.into(), budget);
    }

    /// Get token budget for a section
    pub fn get_token_budget(&self, section: PromptSection) -> Option<usize> {
        self.token_budgets.get(&section.into()).copied()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_overrides() {
        let overrides = UserPromptOverrides::default();
        assert!(overrides.custom_instructions.is_none());
        assert!(overrides.disabled_sections.is_empty());
        assert!(overrides.token_budgets.is_empty());
        assert!(overrides.custom_examples.is_empty());
        assert!(overrides.active_profile.is_none());
    }

    #[test]
    fn test_custom_example_default() {
        let example = CustomExample::default();
        assert!(!example.id.is_empty()); // UUID is generated
        assert!(example.enabled);
        assert!(example.tags.is_empty());
    }

    #[test]
    fn test_tool_preferences_default() {
        let prefs = ToolPreferences::default();
        assert!(prefs.preferred_tools.is_empty());
        assert!(prefs.avoided_tools.is_empty());
        assert!(!prefs.include_tool_examples);
    }

    #[test]
    fn test_add_remove_example() {
        let mut overrides = UserPromptOverrides::default();

        let example = CustomExample {
            id: "test-1".to_string(),
            name: "Test Example".to_string(),
            user_message: "Hello".to_string(),
            assistant_response: "Hi there!".to_string(),
            tags: vec!["greeting".to_string()],
            enabled: true,
        };

        overrides.add_example(example);
        assert_eq!(overrides.custom_examples.len(), 1);

        assert!(overrides.remove_example("test-1"));
        assert!(overrides.custom_examples.is_empty());

        // Removing non-existent returns false
        assert!(!overrides.remove_example("test-1"));
    }

    #[test]
    fn test_enabled_examples() {
        let mut overrides = UserPromptOverrides::default();

        overrides.add_example(CustomExample {
            id: "1".to_string(),
            name: "Enabled".to_string(),
            enabled: true,
            ..Default::default()
        });
        overrides.add_example(CustomExample {
            id: "2".to_string(),
            name: "Disabled".to_string(),
            enabled: false,
            ..Default::default()
        });

        let enabled: Vec<_> = overrides.enabled_examples().collect();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].id, "1");
    }

    #[test]
    fn test_examples_by_tag() {
        let mut overrides = UserPromptOverrides::default();

        overrides.add_example(CustomExample {
            id: "1".to_string(),
            name: "Coding Example".to_string(),
            tags: vec!["coding".to_string(), "rust".to_string()],
            ..Default::default()
        });
        overrides.add_example(CustomExample {
            id: "2".to_string(),
            name: "Browser Example".to_string(),
            tags: vec!["browser".to_string()],
            ..Default::default()
        });

        let coding = overrides.examples_by_tag("coding");
        assert_eq!(coding.len(), 1);
        assert_eq!(coding[0].id, "1");
    }

    #[test]
    fn test_section_disable_enable() {
        let mut overrides = UserPromptOverrides::default();

        assert!(!overrides.is_section_disabled(PromptSection::Organization));

        overrides.disable_section(PromptSection::Organization);
        assert!(overrides.is_section_disabled(PromptSection::Organization));

        overrides.enable_section(PromptSection::Organization);
        assert!(!overrides.is_section_disabled(PromptSection::Organization));
    }

    #[test]
    fn test_token_budgets() {
        let mut overrides = UserPromptOverrides::default();

        assert!(overrides
            .get_token_budget(PromptSection::Platform)
            .is_none());

        overrides.set_token_budget(PromptSection::Platform, 1000);
        assert_eq!(
            overrides.get_token_budget(PromptSection::Platform),
            Some(1000)
        );
    }

    #[test]
    fn test_prompt_section_ref_conversion() {
        let sections = vec![
            PromptSection::Platform,
            PromptSection::Organization,
            PromptSection::User,
            PromptSection::Session,
            PromptSection::SkillDescriptions,
            PromptSection::LoadedSkills,
            PromptSection::Task,
            PromptSection::SubAgent,
            PromptSection::ToolPermissions,
            PromptSection::Custom(5),
        ];

        for section in sections {
            let ref_section: PromptSectionRef = section.into();
            let back: PromptSection = ref_section.into();
            assert_eq!(section, back);
        }
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut overrides = UserPromptOverrides::default();
        overrides.custom_instructions = Some("Test instruction".to_string());
        overrides.disable_section(PromptSection::Organization);
        overrides.set_token_budget(PromptSection::Task, 500);
        overrides.active_profile = Some("browser_automation".to_string());
        overrides.tool_preferences = ToolPreferences {
            preferred_tools: vec!["computer_screenshot".to_string()],
            avoided_tools: vec!["bash".to_string()],
            include_tool_examples: true,
        };
        overrides.add_example(CustomExample {
            id: "ex1".to_string(),
            name: "Example 1".to_string(),
            user_message: "Hi".to_string(),
            assistant_response: "Hello!".to_string(),
            tags: vec!["greeting".to_string()],
            enabled: true,
        });

        // Serialize to YAML
        let yaml = serde_yaml::to_string(&overrides).expect("serialize");

        // Deserialize back
        let loaded: UserPromptOverrides = serde_yaml::from_str(&yaml).expect("deserialize");

        assert_eq!(loaded.custom_instructions, overrides.custom_instructions);
        assert_eq!(loaded.disabled_sections, overrides.disabled_sections);
        assert_eq!(loaded.token_budgets, overrides.token_budgets);
        assert_eq!(loaded.active_profile, overrides.active_profile);
        assert_eq!(
            loaded.tool_preferences.preferred_tools,
            overrides.tool_preferences.preferred_tools
        );
        assert_eq!(
            loaded.tool_preferences.avoided_tools,
            overrides.tool_preferences.avoided_tools
        );
        assert_eq!(
            loaded.tool_preferences.include_tool_examples,
            overrides.tool_preferences.include_tool_examples
        );
        assert_eq!(loaded.custom_examples.len(), 1);
        assert_eq!(loaded.custom_examples[0].id, "ex1");
    }

    #[test]
    fn test_storage_path() {
        let path = UserPromptOverrides::storage_path();
        assert!(path.ends_with("prompt_overrides.yaml"));
        assert!(path.to_string_lossy().contains(".canal"));
    }

    #[test]
    fn test_save_and_load() {
        // Create a temporary directory for testing
        let temp_dir = TempDir::new().expect("create temp dir");
        let config_dir = temp_dir.path().join(".canal");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let file_path = config_dir.join("prompt_overrides.yaml");

        // Create overrides
        let mut overrides = UserPromptOverrides::default();
        overrides.custom_instructions = Some("Save test".to_string());
        overrides.active_profile = Some("test_profile".to_string());

        // Manually save to temp location
        let yaml = serde_yaml::to_string(&overrides).expect("serialize");
        std::fs::write(&file_path, yaml).expect("write file");

        // Load from temp location
        let content = std::fs::read_to_string(&file_path).expect("read file");
        let loaded: UserPromptOverrides = serde_yaml::from_str(&content).expect("deserialize");

        assert_eq!(loaded.custom_instructions, Some("Save test".to_string()));
        assert_eq!(loaded.active_profile, Some("test_profile".to_string()));
    }

    #[test]
    fn test_load_nonexistent_returns_default() {
        // Create a path that definitely doesn't exist
        let temp_dir = TempDir::new().expect("create temp dir");
        let nonexistent = temp_dir
            .path()
            .join("nonexistent")
            .join("prompt_overrides.yaml");

        // When file doesn't exist, load should return default
        // We can't easily test this without mocking storage_path,
        // but we can at least verify default behavior
        let default = UserPromptOverrides::default();
        assert!(default.custom_instructions.is_none());
    }

    #[test]
    fn test_yaml_format() {
        let mut overrides = UserPromptOverrides::default();
        overrides.custom_instructions = Some("Always be concise.".to_string());
        overrides.disable_section(PromptSection::Organization);

        let yaml = serde_yaml::to_string(&overrides).expect("serialize");

        // Verify the YAML format is human-readable
        assert!(yaml.contains("custom_instructions:"));
        assert!(yaml.contains("Always be concise."));
        assert!(yaml.contains("disabled_sections:"));
        assert!(yaml.contains("organization"));
    }

    #[test]
    fn test_skip_empty_fields_in_serialization() {
        let overrides = UserPromptOverrides::default();
        let yaml = serde_yaml::to_string(&overrides).expect("serialize");

        // Empty optional fields should be skipped
        assert!(!yaml.contains("custom_instructions:"));
        assert!(!yaml.contains("active_profile:"));

        // The YAML should be minimal for defaults
        // Only tool_preferences with its defaults should appear
        assert!(yaml.contains("tool_preferences:"));
    }
}
