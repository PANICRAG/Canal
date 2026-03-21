//! Skill Executor - Execute skills with argument substitution
//!
//! Handles skill execution including argument substitution, tool restriction,
//! and skill composition (skills calling other skills).

use super::definition::Skill;
use super::registry::SkillRegistry;
use std::collections::HashSet;
use thiserror::Error;

/// Errors that can occur during skill execution
#[derive(Error, Debug)]
pub enum SkillExecutionError {
    #[error("Skill not found: {0}")]
    NotFound(String),

    #[error("Missing required arguments for skill '{skill}': {hint}")]
    MissingArguments { skill: String, hint: String },

    #[error("Tool '{tool}' is not allowed by skill '{skill}'")]
    ToolNotAllowed { tool: String, skill: String },

    #[error("Circular skill dependency detected: {0}")]
    CircularDependency(String),

    #[error("Dependency not found: skill '{skill}' depends on '{dependency}'")]
    DependencyNotFound { skill: String, dependency: String },

    #[error("Execution error: {0}")]
    ExecutionError(String),
}

/// Result type for skill execution
pub type SkillExecutionResult<T> = Result<T, SkillExecutionError>;

/// Result of preparing a skill for execution
#[derive(Debug, Clone)]
pub struct PreparedSkill {
    /// The rendered prompt with arguments substituted
    pub prompt: String,

    /// The skill that was executed
    pub skill_name: String,

    /// Tools allowed for this skill execution
    pub allowed_tools: HashSet<String>,

    /// Whether all tools are allowed (empty allowed_tools in skill)
    pub all_tools_allowed: bool,

    /// Arguments that were passed
    pub arguments: Option<String>,

    /// Additional context from skill metadata
    pub context: SkillExecutionContext,
}

/// Additional context from skill execution
#[derive(Debug, Clone, Default)]
pub struct SkillExecutionContext {
    /// Namespace of the skill
    pub namespace: Option<String>,

    /// Tags from the skill
    pub tags: Vec<String>,

    /// Whether this is a builtin skill
    pub is_builtin: bool,

    /// Source path of the skill
    pub source_path: Option<String>,

    /// Dependencies that were resolved
    pub resolved_dependencies: Vec<String>,
}

/// Executor for running skills
pub struct SkillExecutor<'a> {
    registry: &'a SkillRegistry,
    max_depth: usize,
}

impl<'a> SkillExecutor<'a> {
    /// Create a new executor with a registry reference
    pub fn new(registry: &'a SkillRegistry) -> Self {
        Self {
            registry,
            max_depth: 10,
        }
    }

    /// Create a builder for the executor
    pub fn builder(registry: &'a SkillRegistry) -> SkillExecutorBuilder<'a> {
        SkillExecutorBuilder::new(registry)
    }

    /// Prepare a skill for execution
    ///
    /// Returns the rendered prompt and tool restrictions
    pub fn prepare(
        &self,
        skill_name: &str,
        arguments: Option<&str>,
    ) -> SkillExecutionResult<PreparedSkill> {
        let skill = self
            .registry
            .get(skill_name)
            .ok_or_else(|| SkillExecutionError::NotFound(skill_name.to_string()))?;

        // Resolve dependencies first
        let resolved_deps = self.resolve_dependencies(&skill, &mut vec![])?;

        // Render the prompt
        let prompt = skill.render_prompt(arguments);

        // Build allowed tools set
        let all_tools_allowed = skill.allowed_tools.is_empty();
        let allowed_tools = skill.allowed_tool_set();

        // Build context
        let context = SkillExecutionContext {
            namespace: skill.metadata.namespace.clone(),
            tags: skill.metadata.tags.clone(),
            is_builtin: skill.is_builtin(),
            source_path: skill.metadata.source_path.clone(),
            resolved_dependencies: resolved_deps,
        };

        Ok(PreparedSkill {
            prompt,
            skill_name: skill.name.clone(),
            allowed_tools,
            all_tools_allowed,
            arguments: arguments.map(|s| s.to_string()),
            context,
        })
    }

    /// Check if a tool is allowed for a skill
    pub fn is_tool_allowed(&self, skill_name: &str, tool_name: &str) -> SkillExecutionResult<bool> {
        let skill = self
            .registry
            .get(skill_name)
            .ok_or_else(|| SkillExecutionError::NotFound(skill_name.to_string()))?;

        Ok(skill.is_tool_allowed(tool_name))
    }

    /// Validate tool usage against skill restrictions
    pub fn validate_tool(&self, skill_name: &str, tool_name: &str) -> SkillExecutionResult<()> {
        if !self.is_tool_allowed(skill_name, tool_name)? {
            return Err(SkillExecutionError::ToolNotAllowed {
                tool: tool_name.to_string(),
                skill: skill_name.to_string(),
            });
        }
        Ok(())
    }

    /// Resolve all dependencies for a skill
    fn resolve_dependencies(
        &self,
        skill: &Skill,
        visited: &mut Vec<String>,
    ) -> SkillExecutionResult<Vec<String>> {
        // Check for circular dependency
        if visited.contains(&skill.name) {
            let cycle = format!("{} -> {}", visited.join(" -> "), skill.name);
            return Err(SkillExecutionError::CircularDependency(cycle));
        }

        // Check depth limit
        if visited.len() >= self.max_depth {
            return Err(SkillExecutionError::CircularDependency(
                "Max dependency depth exceeded".to_string(),
            ));
        }

        visited.push(skill.name.clone());
        let mut resolved = Vec::new();

        for dep_name in &skill.metadata.depends_on {
            let dep_skill = self.registry.get(dep_name).ok_or_else(|| {
                SkillExecutionError::DependencyNotFound {
                    skill: skill.name.clone(),
                    dependency: dep_name.clone(),
                }
            })?;

            // Recursively resolve
            let sub_deps = self.resolve_dependencies(&dep_skill, visited)?;
            resolved.extend(sub_deps);
            resolved.push(dep_name.clone());
        }

        visited.pop();
        Ok(resolved)
    }

    /// Get combined allowed tools for a skill and its dependencies
    pub fn get_combined_allowed_tools(
        &self,
        skill_name: &str,
    ) -> SkillExecutionResult<HashSet<String>> {
        let skill = self
            .registry
            .get(skill_name)
            .ok_or_else(|| SkillExecutionError::NotFound(skill_name.to_string()))?;

        // If main skill allows all tools, return empty (meaning all allowed)
        if skill.allowed_tools.is_empty() {
            return Ok(HashSet::new());
        }

        let mut combined = skill.allowed_tool_set();

        // Add tools from dependencies
        for dep_name in &skill.metadata.depends_on {
            if let Some(dep_skill) = self.registry.get(dep_name) {
                if dep_skill.allowed_tools.is_empty() {
                    // Dependency allows all tools
                    return Ok(HashSet::new());
                }
                combined.extend(dep_skill.allowed_tool_set());
            }
        }

        Ok(combined)
    }

    /// Execute a skill and return the prepared result
    /// This is a convenience method that wraps prepare()
    pub fn execute(
        &self,
        skill_name: &str,
        arguments: Option<&str>,
    ) -> SkillExecutionResult<PreparedSkill> {
        self.prepare(skill_name, arguments)
    }

    /// Get the prompt for a skill with arguments
    pub fn get_prompt(
        &self,
        skill_name: &str,
        arguments: Option<&str>,
    ) -> SkillExecutionResult<String> {
        let result = self.prepare(skill_name, arguments)?;
        Ok(result.prompt)
    }
}

/// Builder for SkillExecutor
pub struct SkillExecutorBuilder<'a> {
    registry: &'a SkillRegistry,
    max_depth: usize,
}

impl<'a> SkillExecutorBuilder<'a> {
    /// Create a new builder
    pub fn new(registry: &'a SkillRegistry) -> Self {
        Self {
            registry,
            max_depth: 10,
        }
    }

    /// Set maximum dependency resolution depth
    pub fn max_depth(mut self, depth: usize) -> Self {
        self.max_depth = depth;
        self
    }

    /// Build the executor
    pub fn build(self) -> SkillExecutor<'a> {
        SkillExecutor {
            registry: self.registry,
            max_depth: self.max_depth,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_registry() -> SkillRegistry {
        let registry = SkillRegistry::new();

        registry
            .register(
                Skill::builder("simple")
                    .description("A simple skill")
                    .prompt_template("Execute: $ARGUMENTS")
                    .build(),
            )
            .unwrap();

        registry
            .register(
                Skill::builder("restricted")
                    .description("A skill with tool restrictions")
                    .prompt_template("Execute with restrictions: $ARGUMENTS")
                    .allowed_tools(vec!["Bash", "Read"])
                    .build(),
            )
            .unwrap();

        registry
            .register(
                Skill::builder("with-deps")
                    .description("A skill with dependencies")
                    .prompt_template("Execute with deps: $ARGUMENTS")
                    .depends_on("simple")
                    .build(),
            )
            .unwrap();

        registry
    }

    #[test]
    fn test_prepare_simple() {
        let registry = create_test_registry();
        let executor = SkillExecutor::new(&registry);

        let result = executor.prepare("simple", Some("hello")).unwrap();

        assert_eq!(result.skill_name, "simple");
        assert_eq!(result.prompt, "Execute: hello");
        assert!(result.all_tools_allowed);
        assert_eq!(result.arguments, Some("hello".to_string()));
    }

    #[test]
    fn test_prepare_with_tool_restrictions() {
        let registry = create_test_registry();
        let executor = SkillExecutor::new(&registry);

        let result = executor.prepare("restricted", None).unwrap();

        assert!(!result.all_tools_allowed);
        assert!(result.allowed_tools.contains("Bash"));
        assert!(result.allowed_tools.contains("Read"));
        assert!(!result.allowed_tools.contains("Write"));
    }

    #[test]
    fn test_prepare_not_found() {
        let registry = create_test_registry();
        let executor = SkillExecutor::new(&registry);

        let result = executor.prepare("nonexistent", None);
        assert!(matches!(
            result.unwrap_err(),
            SkillExecutionError::NotFound(_)
        ));
    }

    #[test]
    fn test_is_tool_allowed() {
        let registry = create_test_registry();
        let executor = SkillExecutor::new(&registry);

        assert!(executor.is_tool_allowed("restricted", "Bash").unwrap());
        assert!(executor.is_tool_allowed("restricted", "Read").unwrap());
        assert!(!executor.is_tool_allowed("restricted", "Write").unwrap());

        // Simple skill allows all tools
        assert!(executor.is_tool_allowed("simple", "Bash").unwrap());
        assert!(executor.is_tool_allowed("simple", "Write").unwrap());
        assert!(executor.is_tool_allowed("simple", "AnyTool").unwrap());
    }

    #[test]
    fn test_validate_tool_allowed() {
        let registry = create_test_registry();
        let executor = SkillExecutor::new(&registry);

        assert!(executor.validate_tool("restricted", "Bash").is_ok());
    }

    #[test]
    fn test_validate_tool_not_allowed() {
        let registry = create_test_registry();
        let executor = SkillExecutor::new(&registry);

        let result = executor.validate_tool("restricted", "Write");
        assert!(matches!(
            result.unwrap_err(),
            SkillExecutionError::ToolNotAllowed { .. }
        ));
    }

    #[test]
    fn test_resolve_dependencies() {
        let registry = create_test_registry();
        let executor = SkillExecutor::new(&registry);

        let result = executor.prepare("with-deps", None).unwrap();
        assert!(result
            .context
            .resolved_dependencies
            .contains(&"simple".to_string()));
    }

    #[test]
    fn test_circular_dependency() {
        let registry = SkillRegistry::new();

        registry
            .register(Skill::builder("a").depends_on("b").build())
            .unwrap();

        registry
            .register(Skill::builder("b").depends_on("a").build())
            .unwrap();

        let executor = SkillExecutor::new(&registry);
        let result = executor.prepare("a", None);

        assert!(matches!(
            result.unwrap_err(),
            SkillExecutionError::CircularDependency(_)
        ));
    }

    #[test]
    fn test_missing_dependency() {
        let registry = SkillRegistry::new();

        registry
            .register(Skill::builder("orphan").depends_on("nonexistent").build())
            .unwrap();

        let executor = SkillExecutor::new(&registry);
        let result = executor.prepare("orphan", None);

        assert!(matches!(
            result.unwrap_err(),
            SkillExecutionError::DependencyNotFound { .. }
        ));
    }

    #[test]
    fn test_get_combined_allowed_tools() {
        let registry = SkillRegistry::new();

        registry
            .register(
                Skill::builder("base")
                    .allowed_tools(vec!["Bash", "Read"])
                    .build(),
            )
            .unwrap();

        registry
            .register(
                Skill::builder("extended")
                    .allowed_tools(vec!["Write", "Glob"])
                    .depends_on("base")
                    .build(),
            )
            .unwrap();

        let executor = SkillExecutor::new(&registry);
        let combined = executor.get_combined_allowed_tools("extended").unwrap();

        assert!(combined.contains("Bash"));
        assert!(combined.contains("Read"));
        assert!(combined.contains("Write"));
        assert!(combined.contains("Glob"));
    }

    #[test]
    fn test_get_combined_allowed_tools_with_unrestricted_dep() {
        let registry = SkillRegistry::new();

        registry
            .register(Skill::builder("unrestricted")
                // No allowed_tools means all tools allowed
                .build())
            .unwrap();

        registry
            .register(
                Skill::builder("restricted")
                    .allowed_tools(vec!["Bash"])
                    .depends_on("unrestricted")
                    .build(),
            )
            .unwrap();

        let executor = SkillExecutor::new(&registry);
        let combined = executor.get_combined_allowed_tools("restricted").unwrap();

        // Should be empty (meaning all tools allowed) because dep is unrestricted
        assert!(combined.is_empty());
    }

    #[test]
    fn test_execute_alias() {
        let registry = create_test_registry();
        let executor = SkillExecutor::new(&registry);

        let result = executor.execute("simple", Some("test")).unwrap();
        assert_eq!(result.prompt, "Execute: test");
    }

    #[test]
    fn test_get_prompt() {
        let registry = create_test_registry();
        let executor = SkillExecutor::new(&registry);

        let prompt = executor.get_prompt("simple", Some("test")).unwrap();
        assert_eq!(prompt, "Execute: test");
    }

    #[test]
    fn test_builder() {
        let registry = create_test_registry();
        let executor = SkillExecutor::builder(&registry).max_depth(5).build();

        assert_eq!(executor.max_depth, 5);
    }

    #[test]
    fn test_context_includes_metadata() {
        let registry = SkillRegistry::new();

        registry
            .register(
                Skill::builder("full")
                    .namespace("git")
                    .tag("vcs")
                    .builtin(true)
                    .prompt_template("Test")
                    .build(),
            )
            .unwrap();

        let executor = SkillExecutor::new(&registry);
        let result = executor.prepare("full", None).unwrap();

        assert_eq!(result.context.namespace, Some("git".to_string()));
        assert!(result.context.tags.contains(&"vcs".to_string()));
        assert!(result.context.is_builtin);
    }

    #[test]
    fn test_deep_dependencies() {
        let registry = SkillRegistry::new();

        registry.register(Skill::builder("level0").build()).unwrap();
        registry
            .register(Skill::builder("level1").depends_on("level0").build())
            .unwrap();
        registry
            .register(Skill::builder("level2").depends_on("level1").build())
            .unwrap();
        registry
            .register(Skill::builder("level3").depends_on("level2").build())
            .unwrap();

        let executor = SkillExecutor::new(&registry);
        let result = executor.prepare("level3", None).unwrap();

        // Should include all transitive dependencies
        let deps = &result.context.resolved_dependencies;
        assert!(deps.contains(&"level0".to_string()));
        assert!(deps.contains(&"level1".to_string()));
        assert!(deps.contains(&"level2".to_string()));
    }

    #[test]
    fn test_max_depth_exceeded() {
        let registry = SkillRegistry::new();

        // Create a chain of 15 dependencies
        for i in 0..15 {
            let mut builder = Skill::builder(format!("skill{}", i));
            if i > 0 {
                builder = builder.depends_on(format!("skill{}", i - 1));
            }
            registry.register(builder.build()).unwrap();
        }

        let executor = SkillExecutor::builder(&registry).max_depth(10).build();

        let result = executor.prepare("skill14", None);
        assert!(matches!(
            result.unwrap_err(),
            SkillExecutionError::CircularDependency(_)
        ));
    }
}
