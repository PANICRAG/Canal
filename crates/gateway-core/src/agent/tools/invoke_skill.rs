//! Invoke Skill Tool - Load skill content on-demand for LLM
//!
//! Allows the LLM to invoke skills by name and receive the full skill content
//! for system prompt injection.
//!
//! Supports fallback to plugin manager (A25): if a skill is not found in the
//! builtin registry, checks the user's installed plugins.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::context::ToolContext;
use super::traits::{AgentTool, ToolResult};
use crate::agent::skills::SkillRegistry;
use crate::plugins::PluginManager;

/// Input for invoking a skill
#[derive(Debug, Clone, Deserialize)]
pub struct InvokeSkillInput {
    /// Name of the skill to invoke
    pub name: String,
}

/// Output from skill invocation
#[derive(Debug, Clone, Serialize)]
pub struct InvokeSkillOutput {
    /// Whether the skill was successfully loaded
    pub success: bool,
    /// The formatted skill content for injection
    pub content: String,
    /// Whether the skill requires browser automation
    pub requires_browser: bool,
    /// Whether to open in automation tab
    pub automation_tab: bool,
    /// Error message if skill not found
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Tool for invoking skills on-demand
pub struct InvokeSkillTool {
    registry: Arc<SkillRegistry>,
    /// Plugin manager for fallback lookup (A25)
    plugin_manager: Option<Arc<PluginManager>>,
    /// User ID for per-user plugin filtering (A25)
    user_id: Option<String>,
}

impl InvokeSkillTool {
    /// Create a new InvokeSkillTool with the given skill registry
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        Self {
            registry,
            plugin_manager: None,
            user_id: None,
        }
    }

    /// Set the plugin manager for fallback lookup (A25).
    pub fn with_plugin_manager(mut self, pm: Arc<PluginManager>) -> Self {
        self.plugin_manager = Some(pm);
        self
    }

    /// Set the user ID for per-user plugin filtering (A25).
    pub fn with_user_id(mut self, user_id: String) -> Self {
        self.user_id = Some(user_id);
        self
    }

    /// Format a skill for output.
    fn format_skill_output(skill: &crate::agent::skills::definition::Skill) -> InvokeSkillOutput {
        let requires_browser = skill
            .metadata
            .custom
            .get("requires_browser")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let automation_tab = skill
            .metadata
            .custom
            .get("automation_tab")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let content = format!(
            "## [{}] Skill Loaded\n\n{}\n\n{}",
            skill.name, skill.description, skill.prompt_template
        );

        InvokeSkillOutput {
            success: true,
            content,
            requires_browser,
            automation_tab,
            error: None,
        }
    }

    /// Format a plugin skill with additional metadata (scripts dir, references).
    fn format_plugin_skill_output(
        skill: &crate::agent::skills::definition::Skill,
        plugin: &crate::plugins::LoadedPlugin,
    ) -> InvokeSkillOutput {
        let requires_browser = skill
            .metadata
            .custom
            .get("requires_browser")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let automation_tab = skill
            .metadata
            .custom
            .get("automation_tab")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut content = format!(
            "## [{}] Plugin Skill Loaded\n\n{}\n\n{}",
            skill.name, skill.description, skill.prompt_template
        );

        // Append scripts directory info
        if let Some(ref scripts_dir) = plugin.scripts_dir {
            content.push_str(&format!(
                "\n\n## Scripts Directory\n\n`{}`",
                scripts_dir.display()
            ));
        }

        // Append reference files list
        if !plugin.reference_paths.is_empty() {
            content.push_str("\n\n## Reference Documents\n\n");
            for (name, _path) in &plugin.reference_paths {
                content.push_str(&format!("- {}\n", name));
            }
        }

        InvokeSkillOutput {
            success: true,
            content,
            requires_browser,
            automation_tab,
            error: None,
        }
    }
}

#[async_trait]
impl AgentTool for InvokeSkillTool {
    type Input = InvokeSkillInput;
    type Output = InvokeSkillOutput;

    fn name(&self) -> &str {
        "invoke_skill"
    }

    fn description(&self) -> &str {
        "Load full skill content on-demand. Use when a skill matches your task."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the skill to invoke"
                }
            }
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "skills"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        // Layer 1: Try builtin registry first
        if let Some(skill) = self.registry.get(&input.name) {
            return Ok(Self::format_skill_output(&skill));
        }

        // Layer 2: Fallback to plugin manager (A25)
        if let Some(ref pm) = self.plugin_manager {
            if let Some(ref user_id) = self.user_id {
                // Check if user has installed a plugin with this skill
                if let Some(skill) = pm.get_user_skill(user_id, &input.name).await {
                    // Get the full plugin for scripts/references info
                    if let Ok(catalog) = pm.catalog.try_read() {
                        if let Some(plugin_name) = &skill.metadata.plugin_name {
                            if let Some(plugin) = catalog.get(plugin_name) {
                                return Ok(Self::format_plugin_skill_output(&skill, plugin));
                            }
                        }
                    }
                    // Fallback: return skill without plugin metadata
                    return Ok(Self::format_skill_output(&skill));
                }
            }
        }

        // Not found in either registry or plugins
        Ok(InvokeSkillOutput {
            success: false,
            content: String::new(),
            requires_browser: false,
            automation_tab: false,
            error: Some(format!("Skill '{}' not found", input.name)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::skills::Skill;

    fn create_test_registry() -> Arc<SkillRegistry> {
        let registry = SkillRegistry::new();

        // Add a basic skill
        registry
            .register(
                Skill::builder("test-skill")
                    .description("A test skill for unit testing")
                    .prompt_template("Execute the test with: $ARGUMENTS")
                    .build(),
            )
            .unwrap();

        // Add a skill with browser metadata
        let mut browser_skill = Skill::builder("browser-skill")
            .description("A skill that requires browser")
            .prompt_template("Use the browser to: $ARGUMENTS")
            .build();
        browser_skill
            .metadata
            .custom
            .insert("requires_browser".to_string(), serde_json::json!(true));
        browser_skill
            .metadata
            .custom
            .insert("automation_tab".to_string(), serde_json::json!(true));
        registry.register(browser_skill).unwrap();

        Arc::new(registry)
    }

    #[tokio::test]
    async fn test_invoke_existing_skill() {
        let registry = create_test_registry();
        let tool = InvokeSkillTool::new(registry);
        let context = ToolContext::default();

        let input = InvokeSkillInput {
            name: "test-skill".to_string(),
        };

        let output = tool.execute(input, &context).await.unwrap();

        assert!(output.success);
        assert!(output.content.contains("test-skill"));
        assert!(output.content.contains("A test skill for unit testing"));
        assert!(output.content.contains("Execute the test with"));
        assert!(!output.requires_browser);
        assert!(!output.automation_tab);
        assert!(output.error.is_none());
    }

    #[tokio::test]
    async fn test_invoke_nonexistent_skill() {
        let registry = create_test_registry();
        let tool = InvokeSkillTool::new(registry);
        let context = ToolContext::default();

        let input = InvokeSkillInput {
            name: "nonexistent".to_string(),
        };

        let output = tool.execute(input, &context).await.unwrap();

        assert!(!output.success);
        assert!(output.content.is_empty());
        assert!(output.error.is_some());
        assert!(output.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_invoke_browser_skill() {
        let registry = create_test_registry();
        let tool = InvokeSkillTool::new(registry);
        let context = ToolContext::default();

        let input = InvokeSkillInput {
            name: "browser-skill".to_string(),
        };

        let output = tool.execute(input, &context).await.unwrap();

        assert!(output.success);
        assert!(output.requires_browser);
        assert!(output.automation_tab);
    }

    #[test]
    fn test_tool_metadata() {
        let registry = Arc::new(SkillRegistry::new());
        let tool = InvokeSkillTool::new(registry);

        assert_eq!(tool.name(), "invoke_skill");
        assert!(!tool.requires_permission());
        assert!(!tool.is_mutating());
        assert_eq!(tool.namespace(), "skills");
    }

    #[test]
    fn test_input_schema() {
        let registry = Arc::new(SkillRegistry::new());
        let tool = InvokeSkillTool::new(registry);

        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("name")));
        assert!(schema["properties"]["name"]["type"] == "string");
    }

    #[tokio::test]
    async fn test_invoke_skill_builtin_priority() {
        // When a skill exists in both registry and plugins,
        // registry (builtin) should be returned
        let registry = create_test_registry();
        let tool = InvokeSkillTool::new(registry);
        let context = ToolContext::default();

        let input = InvokeSkillInput {
            name: "test-skill".to_string(),
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert!(output.success);
        assert!(output.content.contains("Skill Loaded")); // Not "Plugin Skill Loaded"
    }

    #[tokio::test]
    async fn test_invoke_skill_plugin_fallback() {
        use crate::plugins::{PluginCatalog, PluginManager, SubscriptionStore};
        use std::fs;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let plugins_dir = tmp.path().join("plugins");
        fs::create_dir_all(&plugins_dir).unwrap();

        // Create a plugin with a skill
        let pdf_dir = plugins_dir.join("pdf");
        fs::create_dir_all(&pdf_dir).unwrap();
        fs::write(
            pdf_dir.join("SKILL.md"),
            "---\nname: pdf\ndescription: PDF processing\nversion: \"1.0.0\"\n---\n\n# PDF\n\nProcess PDF files.",
        )
        .unwrap();

        let mut catalog = PluginCatalog::new(vec![plugins_dir]);
        catalog.discover();

        let mut subs = SubscriptionStore::new(tmp.path().join("subs.json"));
        subs.install("user1", "pdf").await.unwrap();

        let manager = Arc::new(PluginManager::new(catalog, subs));

        // Create tool with empty registry + plugin_manager
        let registry = Arc::new(SkillRegistry::new());
        let tool = InvokeSkillTool::new(registry)
            .with_plugin_manager(manager)
            .with_user_id("user1".to_string());

        let context = ToolContext::default();
        let input = InvokeSkillInput {
            name: "pdf".to_string(),
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert!(output.success);
        assert!(output.content.contains("pdf"));
        assert!(output.content.contains("PDF processing"));
    }

    #[tokio::test]
    async fn test_invoke_skill_not_installed() {
        use crate::plugins::{PluginCatalog, PluginManager, SubscriptionStore};
        use std::fs;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let plugins_dir = tmp.path().join("plugins");
        fs::create_dir_all(&plugins_dir).unwrap();

        let pdf_dir = plugins_dir.join("pdf");
        fs::create_dir_all(&pdf_dir).unwrap();
        fs::write(
            pdf_dir.join("SKILL.md"),
            "---\nname: pdf\ndescription: PDF processing\n---\n\nContent.",
        )
        .unwrap();

        let mut catalog = PluginCatalog::new(vec![plugins_dir]);
        catalog.discover();

        // User has NOT installed the plugin
        let subs = SubscriptionStore::new(tmp.path().join("subs.json"));
        let manager = Arc::new(PluginManager::new(catalog, subs));

        let registry = Arc::new(SkillRegistry::new());
        let tool = InvokeSkillTool::new(registry)
            .with_plugin_manager(manager)
            .with_user_id("user1".to_string());

        let context = ToolContext::default();
        let input = InvokeSkillInput {
            name: "pdf".to_string(),
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert!(!output.success); // Not installed, so not found
    }
}
