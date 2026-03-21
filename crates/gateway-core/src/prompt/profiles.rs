//! Profile loading and management for the Prompt Constraint System.
//!
//! This module provides the [`ProfileRegistry`] for loading and managing
//! constraint profiles from YAML files.
//!
//! # Example
//!
//! ```rust,ignore
//! use gateway_core::prompt::ProfileRegistry;
//! use std::path::Path;
//!
//! let registry = ProfileRegistry::load_from_directory(Path::new("config/constraints"))?;
//! let profile = registry.get("browser_automation");
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::ConstraintProfile;
use crate::{Error, Result};

/// Summary information about a constraint profile.
///
/// This is a lightweight struct for listing profiles without loading
/// the full configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSummary {
    /// Profile identifier.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// The reasoning mode as a string.
    pub reasoning_mode: String,
}

impl From<&ConstraintProfile> for ProfileSummary {
    fn from(profile: &ConstraintProfile) -> Self {
        Self {
            name: profile.name.clone(),
            description: profile.description.clone(),
            reasoning_mode: format!("{:?}", profile.reasoning_mode),
        }
    }
}

/// Registry for managing constraint profiles.
///
/// The registry loads profiles from YAML files and provides methods
/// to access them by name.
///
/// # Example
///
/// ```rust,ignore
/// use gateway_core::prompt::ProfileRegistry;
/// use std::path::Path;
///
/// // Load all profiles from a directory
/// let registry = ProfileRegistry::load_from_directory(Path::new("config/constraints"))?;
///
/// // List all available profiles
/// for summary in registry.list() {
///     println!("{}: {}", summary.name, summary.description);
/// }
///
/// // Get a specific profile
/// if let Some(profile) = registry.get("browser_automation") {
///     println!("Found profile: {}", profile.name);
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct ProfileRegistry {
    profiles: HashMap<String, ConstraintProfile>,
}

impl ProfileRegistry {
    /// Create a new empty profile registry.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::ProfileRegistry;
    ///
    /// let registry = ProfileRegistry::new();
    /// assert!(registry.list().is_empty());
    /// ```
    pub fn new() -> Self {
        Self {
            profiles: HashMap::new(),
        }
    }

    /// Load all constraint profiles from a directory.
    ///
    /// This method reads all YAML files (`.yaml` and `.yml` extensions)
    /// from the specified directory and loads them as constraint profiles.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the directory containing YAML profile files
    ///
    /// # Returns
    ///
    /// A `Result` containing the populated registry or an error if the
    /// directory doesn't exist or couldn't be read.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gateway_core::prompt::ProfileRegistry;
    /// use std::path::Path;
    ///
    /// let registry = ProfileRegistry::load_from_directory(Path::new("config/constraints"))?;
    /// ```
    pub fn load_from_directory(path: &Path) -> Result<Self> {
        let mut registry = Self::new();

        if !path.exists() {
            return Err(Error::NotFound(format!(
                "Profile directory not found: {}",
                path.display()
            )));
        }

        if !path.is_dir() {
            return Err(Error::InvalidInput(format!(
                "Path is not a directory: {}",
                path.display()
            )));
        }

        let entries = fs::read_dir(path).map_err(|e| {
            Error::Internal(format!(
                "Failed to read profile directory {}: {}",
                path.display(),
                e
            ))
        })?;

        let mut loaded_count = 0;
        let mut error_count = 0;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, "Failed to read directory entry");
                    error_count += 1;
                    continue;
                }
            };

            let file_path = entry.path();

            // Skip non-YAML files
            let extension = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");

            if extension != "yaml" && extension != "yml" {
                debug!(path = %file_path.display(), "Skipping non-YAML file");
                continue;
            }

            // Skip directories
            if file_path.is_dir() {
                continue;
            }

            match ConstraintProfile::load(&file_path) {
                Ok(profile) => {
                    info!(
                        profile = %profile.name,
                        path = %file_path.display(),
                        "Loaded constraint profile"
                    );
                    registry.register(profile);
                    loaded_count += 1;
                }
                Err(e) => {
                    warn!(
                        path = %file_path.display(),
                        error = %e,
                        "Failed to load constraint profile"
                    );
                    error_count += 1;
                }
            }
        }

        info!(
            loaded = loaded_count,
            errors = error_count,
            directory = %path.display(),
            "Profile loading complete"
        );

        Ok(registry)
    }

    /// Get a profile by name.
    ///
    /// # Arguments
    ///
    /// * `name` - The profile name to look up
    ///
    /// # Returns
    ///
    /// A reference to the profile if found, or `None` if not.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ProfileRegistry, ConstraintProfile};
    ///
    /// let mut registry = ProfileRegistry::new();
    /// registry.register(ConstraintProfile::new("test", "Test profile"));
    ///
    /// assert!(registry.get("test").is_some());
    /// assert!(registry.get("nonexistent").is_none());
    /// ```
    pub fn get(&self, name: &str) -> Option<&ConstraintProfile> {
        self.profiles.get(name)
    }

    /// List all registered profiles as summaries.
    ///
    /// The summaries are sorted alphabetically by name.
    ///
    /// # Returns
    ///
    /// A vector of [`ProfileSummary`] for all registered profiles.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ProfileRegistry, ConstraintProfile};
    ///
    /// let mut registry = ProfileRegistry::new();
    /// registry.register(ConstraintProfile::new("alpha", "First"));
    /// registry.register(ConstraintProfile::new("beta", "Second"));
    ///
    /// let list = registry.list();
    /// assert_eq!(list.len(), 2);
    /// assert_eq!(list[0].name, "alpha");
    /// assert_eq!(list[1].name, "beta");
    /// ```
    pub fn list(&self) -> Vec<ProfileSummary> {
        let mut summaries: Vec<ProfileSummary> =
            self.profiles.values().map(ProfileSummary::from).collect();
        summaries.sort_by(|a, b| a.name.cmp(&b.name));
        summaries
    }

    /// Register a profile in the registry.
    ///
    /// If a profile with the same name already exists, it will be replaced.
    ///
    /// # Arguments
    ///
    /// * `profile` - The constraint profile to register
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ProfileRegistry, ConstraintProfile};
    ///
    /// let mut registry = ProfileRegistry::new();
    /// registry.register(ConstraintProfile::new("custom", "Custom profile"));
    ///
    /// assert!(registry.get("custom").is_some());
    /// ```
    pub fn register(&mut self, profile: ConstraintProfile) {
        debug!(profile = %profile.name, "Registering constraint profile");
        self.profiles.insert(profile.name.clone(), profile);
    }

    /// Remove a profile from the registry.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the profile to remove
    ///
    /// # Returns
    ///
    /// The removed profile if it existed, or `None` if not.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ProfileRegistry, ConstraintProfile};
    ///
    /// let mut registry = ProfileRegistry::new();
    /// registry.register(ConstraintProfile::new("temp", "Temporary"));
    ///
    /// let removed = registry.remove("temp");
    /// assert!(removed.is_some());
    /// assert!(registry.get("temp").is_none());
    /// ```
    pub fn remove(&mut self, name: &str) -> Option<ConstraintProfile> {
        self.profiles.remove(name)
    }

    /// Check if a profile exists in the registry.
    ///
    /// # Arguments
    ///
    /// * `name` - The profile name to check
    ///
    /// # Returns
    ///
    /// `true` if the profile exists, `false` otherwise.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ProfileRegistry, ConstraintProfile};
    ///
    /// let mut registry = ProfileRegistry::new();
    /// registry.register(ConstraintProfile::new("exists", "A profile"));
    ///
    /// assert!(registry.contains("exists"));
    /// assert!(!registry.contains("missing"));
    /// ```
    pub fn contains(&self, name: &str) -> bool {
        self.profiles.contains_key(name)
    }

    /// Get the number of registered profiles.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ProfileRegistry, ConstraintProfile};
    ///
    /// let mut registry = ProfileRegistry::new();
    /// assert_eq!(registry.len(), 0);
    ///
    /// registry.register(ConstraintProfile::default());
    /// assert_eq!(registry.len(), 1);
    /// ```
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// Check if the registry is empty.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::ProfileRegistry;
    ///
    /// let registry = ProfileRegistry::new();
    /// assert!(registry.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// Get all profile names.
    ///
    /// # Returns
    ///
    /// A sorted vector of profile names.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ProfileRegistry, ConstraintProfile};
    ///
    /// let mut registry = ProfileRegistry::new();
    /// registry.register(ConstraintProfile::new("b", ""));
    /// registry.register(ConstraintProfile::new("a", ""));
    ///
    /// let names = registry.names();
    /// assert_eq!(names, vec!["a", "b"]);
    /// ```
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.profiles.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// Iterate over all profiles.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::{ProfileRegistry, ConstraintProfile};
    ///
    /// let mut registry = ProfileRegistry::new();
    /// registry.register(ConstraintProfile::new("one", "First"));
    /// registry.register(ConstraintProfile::new("two", "Second"));
    ///
    /// for profile in registry.iter() {
    ///     println!("{}: {}", profile.name, profile.description);
    /// }
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = &ConstraintProfile> {
        self.profiles.values()
    }
}

impl ConstraintProfile {
    /// Load a constraint profile from a YAML file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the YAML file
    ///
    /// # Returns
    ///
    /// A `Result` containing the loaded profile or an error.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gateway_core::prompt::ConstraintProfile;
    /// use std::path::Path;
    ///
    /// let profile = ConstraintProfile::load(Path::new("config/constraints/browser_automation.yaml"))?;
    /// assert_eq!(profile.name, "browser_automation");
    /// ```
    pub fn load(path: &Path) -> Result<Self> {
        /// Maximum profile YAML file size (1 MB).
        const MAX_PROFILE_FILE_SIZE: u64 = 1_048_576;

        if !path.exists() {
            return Err(Error::NotFound(format!(
                "Profile file not found: {}",
                path.display()
            )));
        }

        // Check file size before reading to prevent memory exhaustion.
        let metadata = fs::metadata(path).map_err(|e| {
            Error::Internal(format!(
                "Failed to read profile file metadata {}: {}",
                path.display(),
                e
            ))
        })?;
        if metadata.len() > MAX_PROFILE_FILE_SIZE {
            return Err(Error::InvalidInput(format!(
                "Profile file too large ({} bytes, max {} bytes): {}",
                metadata.len(),
                MAX_PROFILE_FILE_SIZE,
                path.display()
            )));
        }

        let content = fs::read_to_string(path).map_err(|e| {
            Error::Internal(format!(
                "Failed to read profile file {}: {}",
                path.display(),
                e
            ))
        })?;

        Self::from_yaml(&content)
    }

    /// Parse a constraint profile from YAML content.
    ///
    /// # Arguments
    ///
    /// * `yaml` - YAML string content
    ///
    /// # Returns
    ///
    /// A `Result` containing the parsed profile or an error.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::ConstraintProfile;
    ///
    /// let yaml = r#"
    /// name: test
    /// description: Test profile
    /// reasoning_mode: Direct
    /// "#;
    ///
    /// let profile = ConstraintProfile::from_yaml(yaml).unwrap();
    /// assert_eq!(profile.name, "test");
    /// ```
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        serde_yaml::from_str(yaml).map_err(|e| Error::InvalidInput(format!("Invalid YAML: {}", e)))
    }

    /// Save a constraint profile to a YAML file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the output file
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or failure.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use gateway_core::prompt::ConstraintProfile;
    /// use std::path::Path;
    ///
    /// let profile = ConstraintProfile::new("custom", "Custom profile");
    /// profile.save(Path::new("config/constraints/custom.yaml"))?;
    /// ```
    pub fn save(&self, path: &Path) -> Result<()> {
        let yaml = self.to_yaml()?;
        fs::write(path, yaml).map_err(|e| {
            Error::Internal(format!(
                "Failed to write profile file {}: {}",
                path.display(),
                e
            ))
        })
    }

    /// Serialize a constraint profile to YAML.
    ///
    /// # Returns
    ///
    /// A `Result` containing the YAML string or an error.
    ///
    /// # Example
    ///
    /// ```rust
    /// use gateway_core::prompt::ConstraintProfile;
    ///
    /// let profile = ConstraintProfile::new("test", "Test profile");
    /// let yaml = profile.to_yaml().unwrap();
    /// assert!(yaml.contains("name: test"));
    /// ```
    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self)
            .map_err(|e| Error::Internal(format!("Failed to serialize profile: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::ReasoningMode;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_profile_registry_new() {
        let registry = ProfileRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_profile_registry_register() {
        let mut registry = ProfileRegistry::new();
        let profile = ConstraintProfile::new("test", "Test profile");

        registry.register(profile);

        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
        assert!(registry.contains("test"));
    }

    #[test]
    fn test_profile_registry_get() {
        let mut registry = ProfileRegistry::new();
        registry.register(ConstraintProfile::new("test", "Test profile"));

        let profile = registry.get("test");
        assert!(profile.is_some());
        assert_eq!(profile.unwrap().name, "test");

        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_profile_registry_list() {
        let mut registry = ProfileRegistry::new();
        registry.register(ConstraintProfile::new("zebra", "Last"));
        registry.register(ConstraintProfile::new("alpha", "First"));
        registry.register(ConstraintProfile::new("beta", "Second"));

        let list = registry.list();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "alpha");
        assert_eq!(list[1].name, "beta");
        assert_eq!(list[2].name, "zebra");
    }

    #[test]
    fn test_profile_registry_remove() {
        let mut registry = ProfileRegistry::new();
        registry.register(ConstraintProfile::new("test", "Test"));

        let removed = registry.remove("test");
        assert!(removed.is_some());
        assert!(registry.get("test").is_none());

        let removed_again = registry.remove("test");
        assert!(removed_again.is_none());
    }

    #[test]
    fn test_profile_registry_names() {
        let mut registry = ProfileRegistry::new();
        registry.register(ConstraintProfile::new("charlie", ""));
        registry.register(ConstraintProfile::new("alice", ""));
        registry.register(ConstraintProfile::new("bob", ""));

        let names = registry.names();
        assert_eq!(names, vec!["alice", "bob", "charlie"]);
    }

    #[test]
    fn test_profile_registry_iter() {
        let mut registry = ProfileRegistry::new();
        registry.register(ConstraintProfile::new("one", ""));
        registry.register(ConstraintProfile::new("two", ""));

        let count = registry.iter().count();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_profile_summary_from_profile() {
        let profile = ConstraintProfile::new("test", "Test description")
            .with_reasoning_mode(ReasoningMode::ChainOfThought);

        let summary = ProfileSummary::from(&profile);
        assert_eq!(summary.name, "test");
        assert_eq!(summary.description, "Test description");
        assert_eq!(summary.reasoning_mode, "ChainOfThought");
    }

    #[test]
    fn test_constraint_profile_from_yaml() {
        let yaml = r#"
name: test_profile
description: A test profile
reasoning_mode: ChainOfThought
security:
  blocked_commands:
    - rm -rf /
token_limits:
  system_prompt_max: 10000
  response_max: 5000
"#;

        let profile = ConstraintProfile::from_yaml(yaml).unwrap();
        assert_eq!(profile.name, "test_profile");
        assert_eq!(profile.description, "A test profile");
        assert_eq!(profile.reasoning_mode, ReasoningMode::ChainOfThought);
        assert!(profile
            .security
            .blocked_commands
            .contains(&"rm -rf /".to_string()));
        assert_eq!(profile.token_limits.system_prompt_max, 10000);
    }

    #[test]
    fn test_constraint_profile_to_yaml() {
        let profile = ConstraintProfile::new("test", "Test profile")
            .with_reasoning_mode(ReasoningMode::Direct);

        let yaml = profile.to_yaml().unwrap();
        assert!(yaml.contains("name: test"));
        assert!(yaml.contains("description: Test profile"));
        assert!(yaml.contains("reasoning_mode: Direct"));
    }

    #[test]
    fn test_constraint_profile_yaml_roundtrip() {
        let original = ConstraintProfile::new("roundtrip", "Roundtrip test")
            .with_reasoning_mode(ReasoningMode::ReAct);

        let yaml = original.to_yaml().unwrap();
        let parsed = ConstraintProfile::from_yaml(&yaml).unwrap();

        assert_eq!(parsed.name, original.name);
        assert_eq!(parsed.description, original.description);
        assert_eq!(parsed.reasoning_mode, original.reasoning_mode);
    }

    #[test]
    fn test_constraint_profile_load_save() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.yaml");

        let original = ConstraintProfile::new("file_test", "File test profile");
        original.save(&file_path).unwrap();

        let loaded = ConstraintProfile::load(&file_path).unwrap();
        assert_eq!(loaded.name, "file_test");
        assert_eq!(loaded.description, "File test profile");
    }

    #[test]
    fn test_constraint_profile_load_not_found() {
        let result = ConstraintProfile::load(Path::new("/nonexistent/path.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_profile_registry_load_from_directory() {
        let temp_dir = TempDir::new().unwrap();

        // Create test YAML files
        let profile1_path = temp_dir.path().join("profile1.yaml");
        let mut file1 = fs::File::create(&profile1_path).unwrap();
        writeln!(
            file1,
            r#"
name: profile1
description: First profile
reasoning_mode: Direct
"#
        )
        .unwrap();

        let profile2_path = temp_dir.path().join("profile2.yml");
        let mut file2 = fs::File::create(&profile2_path).unwrap();
        writeln!(
            file2,
            r#"
name: profile2
description: Second profile
reasoning_mode: ChainOfThought
"#
        )
        .unwrap();

        // Create a non-YAML file that should be skipped
        let txt_path = temp_dir.path().join("readme.txt");
        fs::write(&txt_path, "This should be ignored").unwrap();

        let registry = ProfileRegistry::load_from_directory(temp_dir.path()).unwrap();

        assert_eq!(registry.len(), 2);
        assert!(registry.contains("profile1"));
        assert!(registry.contains("profile2"));

        let profile1 = registry.get("profile1").unwrap();
        assert_eq!(profile1.reasoning_mode, ReasoningMode::Direct);

        let profile2 = registry.get("profile2").unwrap();
        assert_eq!(profile2.reasoning_mode, ReasoningMode::ChainOfThought);
    }

    #[test]
    fn test_profile_registry_load_from_nonexistent_directory() {
        let result = ProfileRegistry::load_from_directory(Path::new("/nonexistent/directory"));
        assert!(result.is_err());
    }

    #[test]
    fn test_profile_registry_load_from_file_not_directory() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("file.yaml");
        fs::write(&file_path, "name: test").unwrap();

        let result = ProfileRegistry::load_from_directory(&file_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_profile_registry_handles_invalid_yaml() {
        let temp_dir = TempDir::new().unwrap();

        // Create an invalid YAML file
        let invalid_path = temp_dir.path().join("invalid.yaml");
        fs::write(&invalid_path, "{{{{invalid yaml").unwrap();

        // Create a valid YAML file
        let valid_path = temp_dir.path().join("valid.yaml");
        let mut file = fs::File::create(&valid_path).unwrap();
        writeln!(
            file,
            r#"
name: valid
description: Valid profile
reasoning_mode: Direct
"#
        )
        .unwrap();

        // Registry should still load the valid file
        let registry = ProfileRegistry::load_from_directory(temp_dir.path()).unwrap();
        assert_eq!(registry.len(), 1);
        assert!(registry.contains("valid"));
    }

    #[test]
    fn test_profile_with_role_anchor_yaml() {
        let yaml = r#"
name: with_anchor
description: Profile with role anchor
role_anchor:
  role_name: Test Agent
  anchor_prompt: You are a test agent.
  reanchor_interval: 5
  drift_detection: true
  drift_keywords:
    - pretend
    - roleplay
  drift_response: I am a test agent.
reasoning_mode: Direct
"#;

        let profile = ConstraintProfile::from_yaml(yaml).unwrap();
        assert!(profile.role_anchor.is_some());

        let anchor = profile.role_anchor.unwrap();
        assert_eq!(anchor.role_name, "Test Agent");
        assert_eq!(anchor.reanchor_interval, Some(5));
        assert!(anchor.drift_detection);
        assert_eq!(anchor.drift_keywords.len(), 2);
    }

    #[test]
    fn test_profile_with_output_constraints_yaml() {
        let yaml = r#"
name: with_constraints
description: Profile with output constraints
output_constraints:
  - name: json_format
    description: Enforce JSON output
    prompt_injection: Respond with valid JSON only.
    validation_mode: Strict
    enabled: true
  - name: code_format
    description: Code formatting
    prompt_injection: Use proper code blocks.
    validation_mode: WarnOnly
    enabled: false
reasoning_mode: Direct
"#;

        let profile = ConstraintProfile::from_yaml(yaml).unwrap();
        assert_eq!(profile.output_constraints.len(), 2);

        let enabled: Vec<_> = profile.enabled_output_constraints().collect();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name, "json_format");
    }

    #[test]
    fn test_profile_with_security_yaml() {
        let yaml = r#"
name: secure_profile
description: Profile with security settings
security:
  allowed_paths:
    - "${WORKSPACE}/**"
  blocked_patterns:
    - "**/.env*"
    - "**/secrets/**"
  blocked_commands:
    - rm -rf /
    - sudo rm
  require_confirmation:
    - git push --force
reasoning_mode: Direct
"#;

        let profile = ConstraintProfile::from_yaml(yaml).unwrap();
        assert_eq!(profile.security.allowed_paths.len(), 1);
        assert_eq!(profile.security.blocked_patterns.len(), 2);
        assert_eq!(profile.security.blocked_commands.len(), 2);
        assert_eq!(profile.security.require_confirmation.len(), 1);
    }

    #[test]
    fn test_profile_with_token_limits_yaml() {
        let yaml = r#"
name: limited_profile
description: Profile with token limits
token_limits:
  system_prompt_max: 6000
  response_max: 8000
  section_budgets:
    role_anchor: 500
    examples: 1500
reasoning_mode: Direct
"#;

        let profile = ConstraintProfile::from_yaml(yaml).unwrap();
        assert_eq!(profile.token_limits.system_prompt_max, 6000);
        assert_eq!(profile.token_limits.response_max, 8000);
        assert_eq!(
            profile.token_limits.section_budgets.get("role_anchor"),
            Some(&500)
        );
        assert_eq!(
            profile.token_limits.section_budgets.get("examples"),
            Some(&1500)
        );
    }

    #[test]
    fn test_repair_and_retry_validation_mode_yaml() {
        // serde_yaml uses externally-tagged enum representation with YAML tags
        // For enums with data, use the !EnumVariant tag format
        let yaml = "name: repair_profile
description: Profile with repair mode
output_constraints:
  - name: repairable
    description: Can be repaired
    prompt_injection: Try again
    validation_mode: !RepairAndRetry
      max_attempts: 5
    enabled: true
reasoning_mode: Direct
";

        let profile = ConstraintProfile::from_yaml(yaml).unwrap();
        assert_eq!(profile.output_constraints.len(), 1);

        let constraint = &profile.output_constraints[0];
        match &constraint.validation_mode {
            crate::prompt::ValidationMode::RepairAndRetry { max_attempts } => {
                assert_eq!(*max_attempts, 5);
            }
            _ => panic!("Expected RepairAndRetry mode"),
        }
    }
}
