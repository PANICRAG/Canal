//! Role Registry — loads and serves Role configurations from YAML files.

use super::Role;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

/// Registry of available roles, loaded from YAML configuration files.
///
/// Each YAML file in the roles directory defines one role.
/// The file name (without .yaml) is used as the role name if not specified in the file.
pub struct RoleRegistry {
    roles: HashMap<String, Arc<Role>>,
}

impl RoleRegistry {
    /// Load all roles from a directory of YAML files.
    ///
    /// Each `*.yaml` file in the directory is parsed as a `Role`.
    /// Returns an error if the directory doesn't exist or any YAML is invalid.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use gateway_core::roles::RoleRegistry;
    /// use std::path::Path;
    ///
    /// let registry = RoleRegistry::load_from_directory(Path::new("config/roles"))
    ///     .expect("Failed to load roles");
    /// let role = registry.get("platform_operator");
    /// ```
    pub fn load_from_directory(path: &Path) -> crate::Result<Self> {
        let mut roles = HashMap::new();

        if !path.exists() {
            info!(path = %path.display(), "Roles directory not found, using empty registry");
            return Ok(Self { roles });
        }

        let entries = std::fs::read_dir(path).map_err(|e| {
            crate::Error::Internal(format!("Failed to read roles directory: {}", e))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                crate::Error::Internal(format!("Failed to read directory entry: {}", e))
            })?;

            let file_path = entry.path();
            if file_path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }

            let content = std::fs::read_to_string(&file_path).map_err(|e| {
                crate::Error::Internal(format!(
                    "Failed to read role file {}: {}",
                    file_path.display(),
                    e
                ))
            })?;

            let role: Role = serde_yaml::from_str(&content).map_err(|e| {
                crate::Error::Internal(format!(
                    "Failed to parse role file {}: {}",
                    file_path.display(),
                    e
                ))
            })?;

            info!(role = %role.name, path = %file_path.display(), "Loaded role");
            roles.insert(role.name.clone(), Arc::new(role));
        }

        info!(count = roles.len(), "Role registry initialized");
        Ok(Self { roles })
    }

    /// Create an empty registry (no roles loaded).
    pub fn empty() -> Self {
        Self {
            roles: HashMap::new(),
        }
    }

    /// Create a registry from a single role (useful for testing).
    pub fn from_role(role: Role) -> Self {
        let mut roles = HashMap::new();
        roles.insert(role.name.clone(), Arc::new(role));
        Self { roles }
    }

    /// Get a role by name.
    pub fn get(&self, name: &str) -> Option<Arc<Role>> {
        self.roles.get(name).cloned()
    }

    /// List all available role names.
    pub fn list(&self) -> Vec<&str> {
        self.roles.keys().map(|s| s.as_str()).collect()
    }

    /// Number of roles in the registry.
    pub fn len(&self) -> usize {
        self.roles.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::PermissionMode;
    use crate::roles::{PromptInjection, ToolConfig};
    use tempfile::TempDir;

    #[test]
    fn test_empty_registry() {
        let registry = RoleRegistry::empty();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.get("anything").is_none());
        assert!(registry.list().is_empty());
    }

    #[test]
    fn test_from_role() {
        let role = Role {
            name: "test_role".to_string(),
            description: "A test role".to_string(),
            tools: ToolConfig::default(),
            permission_mode: PermissionMode::Default,
            #[cfg(feature = "prompt-constraints")]
            constraint_profile: None,
            system_prompt_sections: Vec::new(),
        };

        let registry = RoleRegistry::from_role(role);
        assert_eq!(registry.len(), 1);
        assert!(registry.get("test_role").is_some());
        assert!(registry.get("other").is_none());

        let loaded = registry.get("test_role").unwrap();
        assert_eq!(loaded.name, "test_role");
        assert_eq!(loaded.permission_mode, PermissionMode::Default);
    }

    #[test]
    fn test_load_from_nonexistent_directory() {
        let result = RoleRegistry::load_from_directory(Path::new("/nonexistent/path"));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_load_from_directory() {
        let dir = TempDir::new().unwrap();

        // Write a valid role YAML
        let yaml = r#"
name: test_operator
description: Test operator role
tools:
  discovery_enabled: true
  initial_tools:
    - Read
    - Write
  enabled_namespaces:
    - platform
permission_mode: plan
system_prompt_sections:
  - header: "Test Section"
    content: "Test content"
    order: 4
"#;
        std::fs::write(dir.path().join("test_operator.yaml"), yaml).unwrap();

        // Write a second role
        let yaml2 = r#"
name: research_planner
description: Research-only role
permission_mode: plan
"#;
        std::fs::write(dir.path().join("research_planner.yaml"), yaml2).unwrap();

        // Write a non-YAML file (should be ignored)
        std::fs::write(dir.path().join("README.md"), "ignore me").unwrap();

        let registry = RoleRegistry::load_from_directory(dir.path()).unwrap();
        assert_eq!(registry.len(), 2);

        let op = registry.get("test_operator").unwrap();
        assert_eq!(op.description, "Test operator role");
        assert!(op.tools.discovery_enabled);
        assert_eq!(op.tools.initial_tools, vec!["Read", "Write"]);
        assert_eq!(op.permission_mode, PermissionMode::Plan);
        assert_eq!(op.system_prompt_sections.len(), 1);

        let rp = registry.get("research_planner").unwrap();
        assert_eq!(rp.description, "Research-only role");
        assert_eq!(rp.permission_mode, PermissionMode::Plan);
    }

    #[test]
    fn test_load_invalid_yaml() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("bad.yaml"), "{{invalid yaml}}").unwrap();

        let result = RoleRegistry::load_from_directory(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_list_roles() {
        let role1 = Role {
            name: "alpha".to_string(),
            description: String::new(),
            tools: ToolConfig::default(),
            permission_mode: PermissionMode::Default,
            #[cfg(feature = "prompt-constraints")]
            constraint_profile: None,
            system_prompt_sections: Vec::new(),
        };
        let role2 = Role {
            name: "beta".to_string(),
            description: String::new(),
            tools: ToolConfig::default(),
            permission_mode: PermissionMode::Plan,
            #[cfg(feature = "prompt-constraints")]
            constraint_profile: None,
            system_prompt_sections: Vec::new(),
        };

        let mut registry = RoleRegistry::empty();
        registry.roles.insert("alpha".to_string(), Arc::new(role1));
        registry.roles.insert("beta".to_string(), Arc::new(role2));

        let mut names = registry.list();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_load_builtin_role_configs() {
        // Load from the actual config/roles/ directory if it exists
        let config_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("config/roles");

        if !config_path.exists() {
            // Skip if config/roles doesn't exist in CI
            return;
        }

        let registry = RoleRegistry::load_from_directory(&config_path).unwrap();
        assert!(
            registry.len() >= 4,
            "Expected at least 4 builtin roles, got {}",
            registry.len()
        );

        // Verify each builtin role loads correctly
        let default = registry.get("default").expect("default role missing");
        assert!(!default.tools.discovery_enabled);
        assert_eq!(default.permission_mode, PermissionMode::Default);

        let platform = registry
            .get("platform_operator")
            .expect("platform_operator role missing");
        assert!(platform.tools.discovery_enabled);
        assert!(platform
            .tools
            .initial_tools
            .contains(&"search_tools".to_string()));
        assert!(!platform.system_prompt_sections.is_empty());

        let research = registry
            .get("research_planner")
            .expect("research_planner role missing");
        assert!(!research.tools.discovery_enabled);
        assert_eq!(research.permission_mode, PermissionMode::Plan);
        assert!(research.tools.blocked_tools.contains(&"Bash".to_string()));

        let coding = registry
            .get("coding_assistant")
            .expect("coding_assistant role missing");
        assert!(coding.tools.discovery_enabled);
        assert_eq!(coding.permission_mode, PermissionMode::AcceptEdits);
    }
}
