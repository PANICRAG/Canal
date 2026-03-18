//! Skill Definition - Core skill types
//!
//! Defines the `Skill` struct and associated metadata types.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A skill definition representing a slash command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Skill name (used as the command, e.g., "commit" for /commit)
    pub name: String,

    /// Human-readable description of what the skill does
    pub description: String,

    /// Tools that this skill is allowed to use
    /// If empty, all tools are allowed
    pub allowed_tools: Vec<String>,

    /// Hint for the argument format (e.g., `[message]`, `<file>`)
    pub argument_hint: Option<String>,

    /// The prompt template with $ARGUMENTS placeholder
    pub prompt_template: String,

    /// Additional metadata about the skill
    pub metadata: SkillMetadata,
}

impl Skill {
    /// Create a new skill with required fields
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        prompt_template: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            allowed_tools: Vec::new(),
            argument_hint: None,
            prompt_template: prompt_template.into(),
            metadata: SkillMetadata::default(),
        }
    }

    /// Create a skill builder
    pub fn builder(name: impl Into<String>) -> SkillBuilder {
        SkillBuilder::new(name)
    }

    /// Get the fully qualified name (namespace:name)
    pub fn qualified_name(&self) -> String {
        if let Some(ref namespace) = self.metadata.namespace {
            format!("{}:{}", namespace, self.name)
        } else {
            self.name.clone()
        }
    }

    /// Check if a tool is allowed by this skill
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        if self.allowed_tools.is_empty() {
            return true;
        }

        self.allowed_tools.iter().any(|allowed| {
            // Support wildcards
            if allowed.ends_with('*') {
                let prefix = &allowed[..allowed.len() - 1];
                tool_name.starts_with(prefix)
            } else {
                allowed == tool_name
            }
        })
    }

    /// Get the set of allowed tool names
    pub fn allowed_tool_set(&self) -> HashSet<String> {
        self.allowed_tools.iter().cloned().collect()
    }

    /// Substitute arguments into the prompt template
    pub fn render_prompt(&self, arguments: Option<&str>) -> String {
        let args = arguments.unwrap_or("");
        self.prompt_template
            .replace("$ARGUMENTS", args)
            .replace("${ARGUMENTS}", args)
    }

    /// Check if this skill is hidden from listing
    pub fn is_hidden(&self) -> bool {
        self.metadata.hidden
    }

    /// Check if this skill is a builtin
    pub fn is_builtin(&self) -> bool {
        self.metadata.builtin
    }

    /// Get the skill's tags
    pub fn tags(&self) -> &[String] {
        &self.metadata.tags
    }
}

impl Default for Skill {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            allowed_tools: Vec::new(),
            argument_hint: None,
            prompt_template: String::new(),
            metadata: SkillMetadata::default(),
        }
    }
}

/// Metadata about a skill
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillMetadata {
    /// Version of the skill
    #[serde(default = "default_version")]
    pub version: String,

    /// Author of the skill
    #[serde(default)]
    pub author: Option<String>,

    /// Tags for categorization and search
    #[serde(default)]
    pub tags: Vec<String>,

    /// Whether the skill is hidden from listing
    #[serde(default)]
    pub hidden: bool,

    /// Whether this is a builtin skill
    #[serde(default)]
    pub builtin: bool,

    /// Namespace for the skill (e.g., "git", "docker")
    #[serde(default)]
    pub namespace: Option<String>,

    /// Source file path (set during loading)
    #[serde(default)]
    pub source_path: Option<String>,

    /// Whether tools should be hidden in slash command display
    #[serde(default)]
    pub slash_command_tools_hidden: bool,

    /// Priority for ordering (higher = shown first)
    #[serde(default)]
    pub priority: i32,

    /// Skills that this skill depends on
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Custom metadata key-value pairs
    #[serde(default)]
    pub custom: std::collections::HashMap<String, serde_json::Value>,

    /// Plugin name if this skill was loaded from a plugin.
    #[serde(default)]
    pub plugin_name: Option<String>,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

impl SkillMetadata {
    /// Create new metadata with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the version
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// Set the author
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// Add a tag
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Set hidden flag
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// Set builtin flag
    pub fn builtin(mut self, builtin: bool) -> Self {
        self.builtin = builtin;
        self
    }

    /// Set namespace
    pub fn namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }
}

/// Builder for creating skills with a fluent API
#[derive(Debug)]
pub struct SkillBuilder {
    skill: Skill,
}

impl SkillBuilder {
    /// Create a new builder with a skill name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            skill: Skill {
                name: name.into(),
                ..Default::default()
            },
        }
    }

    /// Set the description
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.skill.description = description.into();
        self
    }

    /// Set the prompt template
    pub fn prompt_template(mut self, template: impl Into<String>) -> Self {
        self.skill.prompt_template = template.into();
        self
    }

    /// Add an allowed tool
    pub fn allow_tool(mut self, tool: impl Into<String>) -> Self {
        self.skill.allowed_tools.push(tool.into());
        self
    }

    /// Set multiple allowed tools
    pub fn allowed_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.skill.allowed_tools = tools.into_iter().map(|t| t.into()).collect();
        self
    }

    /// Set the argument hint
    pub fn argument_hint(mut self, hint: impl Into<String>) -> Self {
        self.skill.argument_hint = Some(hint.into());
        self
    }

    /// Set the version
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.skill.metadata.version = version.into();
        self
    }

    /// Set the author
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.skill.metadata.author = Some(author.into());
        self
    }

    /// Add a tag
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.skill.metadata.tags.push(tag.into());
        self
    }

    /// Set hidden flag
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.skill.metadata.hidden = hidden;
        self
    }

    /// Set builtin flag
    pub fn builtin(mut self, builtin: bool) -> Self {
        self.skill.metadata.builtin = builtin;
        self
    }

    /// Set namespace
    pub fn namespace(mut self, namespace: impl Into<String>) -> Self {
        self.skill.metadata.namespace = Some(namespace.into());
        self
    }

    /// Set priority
    pub fn priority(mut self, priority: i32) -> Self {
        self.skill.metadata.priority = priority;
        self
    }

    /// Add a dependency
    pub fn depends_on(mut self, skill_name: impl Into<String>) -> Self {
        self.skill.metadata.depends_on.push(skill_name.into());
        self
    }

    /// Set plugin name (for skills loaded from plugins)
    pub fn plugin_name(mut self, name: impl Into<String>) -> Self {
        self.skill.metadata.plugin_name = Some(name.into());
        self
    }

    /// Build the skill
    pub fn build(self) -> Skill {
        self.skill
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_new() {
        let skill = Skill::new("test", "A test skill", "Run: $ARGUMENTS");

        assert_eq!(skill.name, "test");
        assert_eq!(skill.description, "A test skill");
        assert!(skill.allowed_tools.is_empty());
        assert!(skill.argument_hint.is_none());
    }

    #[test]
    fn test_skill_builder() {
        let skill = Skill::builder("commit")
            .description("Create a git commit")
            .prompt_template("Commit with message: $ARGUMENTS")
            .allowed_tools(vec!["Bash", "Read", "Glob"])
            .argument_hint("[message]")
            .version("1.0.0")
            .author("Test Author")
            .tag("git")
            .tag("vcs")
            .namespace("git")
            .priority(100)
            .build();

        assert_eq!(skill.name, "commit");
        assert_eq!(skill.description, "Create a git commit");
        assert_eq!(skill.allowed_tools.len(), 3);
        assert_eq!(skill.argument_hint, Some("[message]".to_string()));
        assert_eq!(skill.metadata.version, "1.0.0");
        assert_eq!(skill.metadata.author, Some("Test Author".to_string()));
        assert_eq!(skill.metadata.tags, vec!["git", "vcs"]);
        assert_eq!(skill.metadata.namespace, Some("git".to_string()));
        assert_eq!(skill.metadata.priority, 100);
    }

    #[test]
    fn test_is_tool_allowed() {
        let skill = Skill::builder("test")
            .allowed_tools(vec!["Bash", "Read*", "Glob"])
            .build();

        assert!(skill.is_tool_allowed("Bash"));
        assert!(skill.is_tool_allowed("Read"));
        assert!(skill.is_tool_allowed("ReadFile"));
        assert!(skill.is_tool_allowed("Glob"));
        assert!(!skill.is_tool_allowed("Write"));
    }

    #[test]
    fn test_is_tool_allowed_empty() {
        let skill = Skill::builder("test").build();

        // Empty allowed_tools means all tools are allowed
        assert!(skill.is_tool_allowed("Bash"));
        assert!(skill.is_tool_allowed("Write"));
        assert!(skill.is_tool_allowed("AnyTool"));
    }

    #[test]
    fn test_render_prompt() {
        let skill = Skill::builder("test")
            .prompt_template("Execute command: $ARGUMENTS\nWith ${ARGUMENTS}")
            .build();

        let rendered = skill.render_prompt(Some("ls -la"));
        assert_eq!(rendered, "Execute command: ls -la\nWith ls -la");

        let rendered_empty = skill.render_prompt(None);
        assert_eq!(rendered_empty, "Execute command: \nWith ");
    }

    #[test]
    fn test_qualified_name() {
        let skill_no_ns = Skill::builder("commit").build();
        assert_eq!(skill_no_ns.qualified_name(), "commit");

        let skill_with_ns = Skill::builder("commit").namespace("git").build();
        assert_eq!(skill_with_ns.qualified_name(), "git:commit");
    }

    #[test]
    fn test_is_hidden() {
        let visible_skill = Skill::builder("test").build();
        assert!(!visible_skill.is_hidden());

        let hidden_skill = Skill::builder("test").hidden(true).build();
        assert!(hidden_skill.is_hidden());
    }

    #[test]
    fn test_is_builtin() {
        let user_skill = Skill::builder("test").build();
        assert!(!user_skill.is_builtin());

        let builtin_skill = Skill::builder("test").builtin(true).build();
        assert!(builtin_skill.is_builtin());
    }

    #[test]
    fn test_allowed_tool_set() {
        let skill = Skill::builder("test")
            .allowed_tools(vec!["Bash", "Read", "Glob"])
            .build();

        let tool_set = skill.allowed_tool_set();
        assert_eq!(tool_set.len(), 3);
        assert!(tool_set.contains("Bash"));
        assert!(tool_set.contains("Read"));
        assert!(tool_set.contains("Glob"));
    }

    #[test]
    fn test_skill_metadata_builder() {
        let metadata = SkillMetadata::new()
            .version("2.0.0")
            .author("Claude")
            .tag("git")
            .hidden(false)
            .builtin(true)
            .namespace("vcs");

        assert_eq!(metadata.version, "2.0.0");
        assert_eq!(metadata.author, Some("Claude".to_string()));
        assert_eq!(metadata.tags, vec!["git"]);
        assert!(!metadata.hidden);
        assert!(metadata.builtin);
        assert_eq!(metadata.namespace, Some("vcs".to_string()));
    }

    #[test]
    fn test_skill_default() {
        let skill = Skill::default();
        assert!(skill.name.is_empty());
        assert!(skill.description.is_empty());
        assert!(skill.allowed_tools.is_empty());
        assert!(skill.argument_hint.is_none());
        assert!(skill.prompt_template.is_empty());
    }

    #[test]
    fn test_skill_metadata_plugin_name() {
        let skill = Skill::builder("pdf")
            .description("PDF processing")
            .plugin_name("pdf")
            .build();
        assert_eq!(skill.metadata.plugin_name, Some("pdf".to_string()));

        let skill_no_plugin = Skill::builder("test").build();
        assert_eq!(skill_no_plugin.metadata.plugin_name, None);
    }

    #[test]
    fn test_skill_serialization() {
        let skill = Skill::builder("commit")
            .description("Create a git commit")
            .prompt_template("Commit: $ARGUMENTS")
            .allowed_tools(vec!["Bash", "Read"])
            .build();

        let json = serde_json::to_string(&skill).unwrap();
        let deserialized: Skill = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, skill.name);
        assert_eq!(deserialized.description, skill.description);
        assert_eq!(deserialized.allowed_tools, skill.allowed_tools);
    }
}
