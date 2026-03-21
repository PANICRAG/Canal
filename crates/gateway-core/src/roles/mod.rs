//! Role Constraint System (A46)
//!
//! A Role is a named composition of four existing subsystems:
//! - Tool filtering (which tools are visible)
//! - Permission mode (what access level)
//! - Constraint profile (behavioral anchoring + security)
//! - System prompt injection (role-specific instructions)
//!
//! Roles do NOT add new execution logic — they configure existing
//! subsystems through a single YAML-driven configuration.

mod registry;

pub use registry::RoleRegistry;

use crate::agent::types::PermissionMode;
use serde::{Deserialize, Serialize};

#[cfg(feature = "prompt-constraints")]
use crate::prompt::ConstraintProfile;

/// A Role is a named composition of tool access, permissions, behavioral
/// constraints, and system prompt injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    /// Role identifier (e.g., "platform_operator", "research_planner")
    pub name: String,

    /// Human-readable description
    #[serde(default)]
    pub description: String,

    /// Tool discovery and filtering configuration
    #[serde(default)]
    pub tools: ToolConfig,

    /// Permission level for this role
    #[serde(default)]
    pub permission_mode: PermissionMode,

    /// Constraint profile (security, role anchor, output validation)
    #[cfg(feature = "prompt-constraints")]
    #[serde(default)]
    pub constraint_profile: Option<ConstraintProfile>,

    /// Additional system prompt sections injected for this role
    #[serde(default)]
    pub system_prompt_sections: Vec<PromptInjection>,
}

/// Tool discovery and filtering configuration for a role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    /// Enable search_tools meta-tool for on-demand discovery
    #[serde(default)]
    pub discovery_enabled: bool,

    /// Initial tools always available (before discovery).
    /// Empty = all tools available initially (legacy behavior).
    #[serde(default)]
    pub initial_tools: Vec<String>,

    /// Tool namespaces visible to this role (None = all)
    #[serde(default)]
    pub enabled_namespaces: Option<Vec<String>>,

    /// Tools explicitly blocked for this role
    #[serde(default)]
    pub blocked_tools: Vec<String>,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            discovery_enabled: false,
            initial_tools: Vec::new(),
            enabled_namespaces: None,
            blocked_tools: Vec::new(),
        }
    }
}

/// A system prompt section injected by a role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptInjection {
    /// Section header (e.g., "Role Capabilities")
    pub header: String,

    /// Section content (markdown)
    pub content: String,

    /// Order in prompt (higher = later in the prompt)
    #[serde(default = "default_order")]
    pub order: u8,
}

fn default_order() -> u8 {
    4
}

impl Role {
    /// Apply this role to an AgentFactory by calling existing builder methods.
    ///
    /// This is the core integration point — Role does not add new logic,
    /// it only calls existing AgentFactory configuration methods.
    pub fn apply_to_factory(
        &self,
        mut factory: crate::agent::factory::AgentFactory,
    ) -> crate::agent::factory::AgentFactory {
        // 1. Tool namespace filtering
        if let Some(ref ns) = self.tools.enabled_namespaces {
            factory = factory.with_enabled_namespaces(ns.clone());
        }

        // 2. Permission mode
        factory = factory.with_permission_mode(self.permission_mode);

        // 3. Constraint profile (includes RoleAnchor, SecurityBoundary)
        #[cfg(feature = "prompt-constraints")]
        if let Some(ref profile) = self.constraint_profile {
            factory = factory.with_constraint_profile(Some(profile.clone()));
        }

        // 4. Tool discovery mode
        if self.tools.discovery_enabled && !self.tools.initial_tools.is_empty() {
            factory = factory.with_tool_discovery(self.tools.initial_tools.clone());
        }

        // 5. System prompt sections (role-specific instructions)
        if let Some(sections) = self.generate_prompt_sections() {
            factory = factory.with_role_prompt_sections(sections);
        }

        factory
    }

    /// Generate the role-specific system prompt sections as a single string.
    ///
    /// This is appended to the system prompt during context integration.
    pub fn generate_prompt_sections(&self) -> Option<String> {
        if self.system_prompt_sections.is_empty() {
            return None;
        }

        let mut sections = self.system_prompt_sections.clone();
        sections.sort_by_key(|s| s.order);

        let mut prompt = String::new();
        for section in &sections {
            prompt.push_str(&format!("\n# {}\n\n{}\n", section.header, section.content));
        }

        Some(prompt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_config_default() {
        let config = ToolConfig::default();
        assert!(!config.discovery_enabled);
        assert!(config.initial_tools.is_empty());
        assert!(config.enabled_namespaces.is_none());
        assert!(config.blocked_tools.is_empty());
    }

    #[test]
    fn test_role_deserialize_minimal() {
        let yaml = r#"
name: test_role
description: A test role
"#;
        let role: Role = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(role.name, "test_role");
        assert_eq!(role.description, "A test role");
        assert!(!role.tools.discovery_enabled);
        assert_eq!(role.permission_mode, PermissionMode::BypassPermissions);
        assert!(role.system_prompt_sections.is_empty());
    }

    #[test]
    fn test_role_deserialize_full() {
        let yaml = r#"
name: platform_operator
description: Platform infrastructure management AI
tools:
  discovery_enabled: true
  initial_tools:
    - Read
    - Write
    - Edit
    - search_tools
  enabled_namespaces:
    - platform
    - hosting
  blocked_tools:
    - Computer
permission_mode: plan
system_prompt_sections:
  - header: "Platform Tools Reference"
    content: "Use search_tools to discover platform tools."
    order: 4
"#;
        let role: Role = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(role.name, "platform_operator");
        assert!(role.tools.discovery_enabled);
        assert_eq!(role.tools.initial_tools.len(), 4);
        assert_eq!(
            role.tools.enabled_namespaces,
            Some(vec!["platform".to_string(), "hosting".to_string()])
        );
        assert_eq!(role.tools.blocked_tools, vec!["Computer".to_string()]);
        assert_eq!(role.permission_mode, PermissionMode::Plan);
        assert_eq!(role.system_prompt_sections.len(), 1);
        assert_eq!(
            role.system_prompt_sections[0].header,
            "Platform Tools Reference"
        );
    }

    #[test]
    fn test_role_generate_prompt_sections_empty() {
        let role = Role {
            name: "test".to_string(),
            description: String::new(),
            tools: ToolConfig::default(),
            permission_mode: PermissionMode::Default,
            #[cfg(feature = "prompt-constraints")]
            constraint_profile: None,
            system_prompt_sections: Vec::new(),
        };
        assert!(role.generate_prompt_sections().is_none());
    }

    #[test]
    fn test_role_generate_prompt_sections_ordered() {
        let role = Role {
            name: "test".to_string(),
            description: String::new(),
            tools: ToolConfig::default(),
            permission_mode: PermissionMode::Default,
            #[cfg(feature = "prompt-constraints")]
            constraint_profile: None,
            system_prompt_sections: vec![
                PromptInjection {
                    header: "Second".to_string(),
                    content: "Content B".to_string(),
                    order: 5,
                },
                PromptInjection {
                    header: "First".to_string(),
                    content: "Content A".to_string(),
                    order: 3,
                },
            ],
        };

        let prompt = role.generate_prompt_sections().unwrap();
        let first_pos = prompt.find("First").unwrap();
        let second_pos = prompt.find("Second").unwrap();
        assert!(first_pos < second_pos);
    }

    #[test]
    fn test_role_serialization_roundtrip() {
        let role = Role {
            name: "test_role".to_string(),
            description: "Test".to_string(),
            tools: ToolConfig {
                discovery_enabled: true,
                initial_tools: vec!["Read".to_string()],
                enabled_namespaces: Some(vec!["platform".to_string()]),
                blocked_tools: vec!["Bash".to_string()],
            },
            permission_mode: PermissionMode::Plan,
            #[cfg(feature = "prompt-constraints")]
            constraint_profile: None,
            system_prompt_sections: vec![PromptInjection {
                header: "Test".to_string(),
                content: "Content".to_string(),
                order: 1,
            }],
        };

        let yaml = serde_yaml::to_string(&role).unwrap();
        let deserialized: Role = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(deserialized.name, role.name);
        assert_eq!(deserialized.tools.discovery_enabled, true);
        assert_eq!(deserialized.permission_mode, PermissionMode::Plan);
    }
}
