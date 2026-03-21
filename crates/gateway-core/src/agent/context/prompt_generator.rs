//! System Prompt Generator
//!
//! This module provides sophisticated system prompt generation from ResolvedContext.
//! It handles template-based generation, section ordering, and formatting options.
//!
//! # Features
//!
//! - Template-based prompt generation
//! - Configurable section ordering
//! - Token budget awareness
//! - Skill injection with proper formatting
//! - Tool permission documentation
//!
//! # Example
//!
//! ```rust,ignore
//! use gateway_core::agent::context::{
//!     ResolvedContext, SystemPromptGenerator, PromptSection,
//! };
//!
//! let resolved = ResolvedContext::default();
//! let generator = SystemPromptGenerator::new()
//!     .with_max_tokens(8000)
//!     .with_section_order(vec![
//!         PromptSection::Platform,
//!         PromptSection::Organization,
//!         PromptSection::User,
//!         PromptSection::Skills,
//!         PromptSection::Task,
//!     ]);
//!
//! let prompt = generator.generate(&resolved);
//! ```

use super::{LoadedSkill, PermissionMode, ResolvedContext};
use std::collections::HashMap;

/// Sections that can be included in the system prompt
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptSection {
    /// Platform rules (L1) - Always first, cannot be omitted
    Platform,
    /// Organization conventions (L2)
    Organization,
    /// User preferences (L3)
    User,
    /// Memory context (A38) - semantic recall, custom instructions, learned patterns
    Memory,
    /// Session context (L4)
    Session,
    /// Skill descriptions (Layer 1 of two-layer loading)
    SkillDescriptions,
    /// Loaded skill content (Layer 2 of two-layer loading)
    LoadedSkills,
    /// Task instructions (L5)
    Task,
    /// SubAgent context (L6)
    SubAgent,
    /// Tool permissions section
    ToolPermissions,
    /// Custom injected section
    Custom(u8),
}

impl PromptSection {
    /// Default ordering priority (lower = earlier in prompt)
    pub fn default_order(&self) -> u8 {
        match self {
            Self::Platform => 0,
            Self::Organization => 1,
            Self::User => 2,
            Self::Memory => 3,
            Self::Session => 4,
            Self::SkillDescriptions => 5,
            Self::LoadedSkills => 6,
            Self::Task => 7,
            Self::SubAgent => 8,
            Self::ToolPermissions => 9,
            Self::Custom(n) => 100 + n,
        }
    }

    /// Section header for markdown formatting
    pub fn header(&self) -> &'static str {
        match self {
            Self::Platform => "# Platform Rules",
            Self::Organization => "# Organization Conventions",
            Self::User => "# User Preferences",
            Self::Memory => "# Memory",
            Self::Session => "# Session Context",
            Self::SkillDescriptions => "# Available Skills",
            Self::LoadedSkills => "# Active Skills",
            Self::Task => "# Current Task",
            Self::SubAgent => "# SubAgent Configuration",
            Self::ToolPermissions => "# Tool Permissions",
            Self::Custom(_) => "# Custom Section",
        }
    }
}

/// Configuration for prompt generation
#[derive(Debug, Clone)]
pub struct PromptConfig {
    /// Maximum tokens for the entire prompt (0 = unlimited)
    pub max_tokens: usize,
    /// Whether to include section headers
    pub include_headers: bool,
    /// Whether to include tool permission section
    pub include_tool_permissions: bool,
    /// Custom section order (if None, use default)
    pub section_order: Option<Vec<PromptSection>>,
    /// Sections to skip
    pub skip_sections: Vec<PromptSection>,
    /// Custom section content
    pub custom_sections: HashMap<u8, String>,
    /// Token budget per section (if max_tokens is set)
    pub section_budgets: HashMap<PromptSection, usize>,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            max_tokens: 0,
            include_headers: true,
            include_tool_permissions: false,
            section_order: None,
            skip_sections: Vec::new(),
            custom_sections: HashMap::new(),
            section_budgets: HashMap::new(),
        }
    }
}

/// System prompt generator with configurable options
#[derive(Debug, Clone)]
pub struct SystemPromptGenerator {
    config: PromptConfig,
}

impl Default for SystemPromptGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemPromptGenerator {
    /// Create a new generator with default configuration
    pub fn new() -> Self {
        Self {
            config: PromptConfig::default(),
        }
    }

    /// Set maximum token limit
    pub fn with_max_tokens(mut self, max_tokens: usize) -> Self {
        self.config.max_tokens = max_tokens;
        self
    }

    /// Set whether to include section headers
    pub fn with_headers(mut self, include: bool) -> Self {
        self.config.include_headers = include;
        self
    }

    /// Set whether to include tool permissions section
    pub fn with_tool_permissions(mut self, include: bool) -> Self {
        self.config.include_tool_permissions = include;
        self
    }

    /// Set custom section order
    pub fn with_section_order(mut self, order: Vec<PromptSection>) -> Self {
        self.config.section_order = Some(order);
        self
    }

    /// Skip specific sections
    pub fn skip_section(mut self, section: PromptSection) -> Self {
        self.config.skip_sections.push(section);
        self
    }

    /// Add a custom section
    pub fn with_custom_section(mut self, id: u8, content: String) -> Self {
        self.config.custom_sections.insert(id, content);
        self
    }

    /// Set token budget for a specific section
    pub fn with_section_budget(mut self, section: PromptSection, budget: usize) -> Self {
        self.config.section_budgets.insert(section, budget);
        self
    }

    /// Generate the system prompt from resolved context
    pub fn generate(&self, ctx: &ResolvedContext) -> String {
        let mut sections: Vec<(PromptSection, String)> = Vec::new();

        // Build sections
        if !ctx.platform_rules.is_empty() {
            sections.push((PromptSection::Platform, ctx.platform_rules.clone()));
        }

        if let Some(org) = &ctx.org_conventions {
            sections.push((PromptSection::Organization, org.clone()));
        }

        if let Some(user) = &ctx.user_preferences {
            sections.push((PromptSection::User, user.clone()));
        }

        if let Some(memory) = &ctx.memory_context {
            sections.push((PromptSection::Memory, memory.clone()));
        }

        if let Some(session) = &ctx.session_context {
            sections.push((PromptSection::Session, session.clone()));
        }

        if !ctx.skill_descriptions.is_empty() {
            sections.push((
                PromptSection::SkillDescriptions,
                ctx.skill_descriptions.clone(),
            ));
        }

        if !ctx.loaded_skills.is_empty() {
            let skills_content = self.format_loaded_skills(&ctx.loaded_skills);
            sections.push((PromptSection::LoadedSkills, skills_content));
        }

        if let Some(task) = &ctx.task_instructions {
            sections.push((PromptSection::Task, task.clone()));
        }

        if let Some(subagent) = &ctx.subagent_context {
            sections.push((PromptSection::SubAgent, subagent.clone()));
        }

        // Add tool permissions if configured
        if self.config.include_tool_permissions {
            let permissions = self.format_tool_permissions(ctx);
            if !permissions.is_empty() {
                sections.push((PromptSection::ToolPermissions, permissions));
            }
        }

        // Add custom sections
        for (id, content) in &self.config.custom_sections {
            sections.push((PromptSection::Custom(*id), content.clone()));
        }

        // Filter out skipped sections
        sections.retain(|(section, _)| !self.config.skip_sections.contains(section));

        // Sort by order
        if let Some(order) = &self.config.section_order {
            sections.sort_by_key(|(section, _)| {
                order
                    .iter()
                    .position(|s| s == section)
                    .unwrap_or(usize::MAX)
            });
        } else {
            sections.sort_by_key(|(section, _)| section.default_order());
        }

        // Build final prompt
        let mut prompt = String::new();
        for (section, content) in sections {
            if self.config.include_headers && !content.trim().starts_with('#') {
                prompt.push_str(section.header());
                prompt.push_str("\n\n");
            }

            // Apply section budget if set
            let content = if let Some(budget) = self.config.section_budgets.get(&section) {
                self.truncate_to_tokens(&content, *budget)
            } else {
                content
            };

            prompt.push_str(&content);
            prompt.push_str("\n\n");
        }

        // Apply total token limit
        let prompt = if self.config.max_tokens > 0 {
            self.truncate_to_tokens(&prompt, self.config.max_tokens)
        } else {
            prompt
        };

        prompt.trim_end().to_string()
    }

    /// Format loaded skills for the prompt
    fn format_loaded_skills(&self, skills: &[LoadedSkill]) -> String {
        let mut output = String::new();

        for skill in skills {
            output.push_str(&format!("## {} (Active)\n\n", skill.name));

            if skill.requires_browser {
                output.push_str("> ⚠️ This skill requires browser access\n\n");
            }
            if skill.automation_tab {
                output.push_str("> 🤖 This skill uses automation tab\n\n");
            }

            output.push_str(&skill.content);
            output.push_str("\n\n---\n\n");
        }

        output.trim_end_matches("\n---\n\n").to_string()
    }

    /// Format tool permissions section
    fn format_tool_permissions(&self, ctx: &ResolvedContext) -> String {
        let mut output = String::new();

        // Permission mode
        output.push_str(&format!(
            "**Permission Mode**: {}\n\n",
            match ctx.permission_mode {
                PermissionMode::Normal => "Normal",
                PermissionMode::Elevated => "Elevated",
                PermissionMode::Restricted => "Restricted",
            }
        ));

        // Allowed tools
        if !ctx.allowed_tools.is_empty() {
            output.push_str("**Allowed Tools**:\n");
            for tool in &ctx.allowed_tools {
                output.push_str(&format!("- `{}`\n", tool));
            }
            output.push('\n');
        }

        // Blocked tools
        if !ctx.blocked_tools.is_empty() {
            output.push_str("**Blocked Tools**:\n");
            for tool in &ctx.blocked_tools {
                output.push_str(&format!("- `{}` ❌\n", tool));
            }
        }

        output
    }

    /// Truncate content to approximate token count
    /// Uses rough estimate of 4 chars per token
    fn truncate_to_tokens(&self, content: &str, max_tokens: usize) -> String {
        let max_chars = max_tokens * 4;
        if content.len() <= max_chars {
            return content.to_string();
        }

        // Find a good break point
        let truncated = &content[..max_chars];
        if let Some(last_newline) = truncated.rfind('\n') {
            format!(
                "{}\n\n[... truncated for token limit ...]",
                &truncated[..last_newline]
            )
        } else {
            format!("{}\n\n[... truncated for token limit ...]", truncated)
        }
    }

    /// Get the current configuration
    pub fn config(&self) -> &PromptConfig {
        &self.config
    }

    /// Estimate token count for a string (rough approximation)
    pub fn estimate_tokens(content: &str) -> usize {
        content.len() / 4
    }
}

/// Builder for creating prompts with fluent API
#[derive(Debug, Default)]
pub struct PromptBuilder {
    sections: Vec<(u8, String)>,
    separator: String,
}

impl PromptBuilder {
    /// Create a new prompt builder
    pub fn new() -> Self {
        Self {
            sections: Vec::new(),
            separator: "\n\n".to_string(),
        }
    }

    /// Set the separator between sections
    pub fn with_separator(mut self, sep: &str) -> Self {
        self.separator = sep.to_string();
        self
    }

    /// Add a section with priority
    pub fn add_section(mut self, priority: u8, content: impl Into<String>) -> Self {
        self.sections.push((priority, content.into()));
        self
    }

    /// Add platform rules
    pub fn platform(self, content: impl Into<String>) -> Self {
        self.add_section(0, content)
    }

    /// Add organization conventions
    pub fn organization(self, content: impl Into<String>) -> Self {
        self.add_section(1, content)
    }

    /// Add user preferences
    pub fn user(self, content: impl Into<String>) -> Self {
        self.add_section(2, content)
    }

    /// Add session context
    pub fn session(self, content: impl Into<String>) -> Self {
        self.add_section(3, content)
    }

    /// Add skill descriptions
    pub fn skills(self, content: impl Into<String>) -> Self {
        self.add_section(4, content)
    }

    /// Add task instructions
    pub fn task(self, content: impl Into<String>) -> Self {
        self.add_section(5, content)
    }

    /// Add subagent context
    pub fn subagent(self, content: impl Into<String>) -> Self {
        self.add_section(6, content)
    }

    /// Build the final prompt
    pub fn build(mut self) -> String {
        self.sections.sort_by_key(|(priority, _)| *priority);
        self.sections
            .into_iter()
            .map(|(_, content)| content)
            .collect::<Vec<_>>()
            .join(&self.separator)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_context() -> ResolvedContext {
        let mut ctx = ResolvedContext::default();
        ctx.platform_rules = "Always use English.".to_string();
        ctx.org_conventions = Some("Follow team coding standards.".to_string());
        ctx.user_preferences = Some("Prefer explicit types.".to_string());
        ctx.skill_descriptions = "- commit: Create git commits\n- review: Review code".to_string();
        ctx.loaded_skills.push(LoadedSkill {
            name: "commit".to_string(),
            content: "When committing, use conventional commits format.".to_string(),
            requires_browser: false,
            automation_tab: false,
        });
        ctx.task_instructions = Some("Fix the authentication bug.".to_string());
        ctx
    }

    #[test]
    fn test_default_generation() {
        let ctx = sample_context();
        let generator = SystemPromptGenerator::new();
        let prompt = generator.generate(&ctx);

        assert!(prompt.contains("Always use English"));
        assert!(prompt.contains("Follow team coding standards"));
        assert!(prompt.contains("Prefer explicit types"));
        assert!(prompt.contains("commit: Create git commits"));
        assert!(prompt.contains("commit (Active)"));
        assert!(prompt.contains("Fix the authentication bug"));
    }

    #[test]
    fn test_section_order() {
        let ctx = sample_context();
        let generator = SystemPromptGenerator::new();
        let prompt = generator.generate(&ctx);

        // Platform should come before Organization
        let platform_pos = prompt.find("Always use English").unwrap();
        let org_pos = prompt.find("Follow team coding standards").unwrap();
        assert!(platform_pos < org_pos);

        // Task should come after skills
        let skills_pos = prompt.find("commit: Create git commits").unwrap();
        let task_pos = prompt.find("Fix the authentication bug").unwrap();
        assert!(skills_pos < task_pos);
    }

    #[test]
    fn test_custom_section_order() {
        let ctx = sample_context();
        let generator = SystemPromptGenerator::new()
            .with_section_order(vec![PromptSection::Task, PromptSection::Platform]);
        let prompt = generator.generate(&ctx);

        // Task should now come before Platform
        let task_pos = prompt.find("Fix the authentication bug").unwrap();
        let platform_pos = prompt.find("Always use English").unwrap();
        assert!(task_pos < platform_pos);
    }

    #[test]
    fn test_skip_sections() {
        let ctx = sample_context();
        let generator = SystemPromptGenerator::new().skip_section(PromptSection::Organization);
        let prompt = generator.generate(&ctx);

        assert!(!prompt.contains("Follow team coding standards"));
        assert!(prompt.contains("Always use English"));
    }

    #[test]
    fn test_without_headers() {
        let ctx = sample_context();
        let generator = SystemPromptGenerator::new().with_headers(false);
        let prompt = generator.generate(&ctx);

        // Should not have the generic headers
        // (but content that starts with # is preserved)
        assert!(prompt.contains("Always use English"));
    }

    #[test]
    fn test_with_tool_permissions() {
        let mut ctx = sample_context();
        ctx.allowed_tools = vec!["read".to_string(), "write".to_string()];
        ctx.blocked_tools = vec!["bash".to_string()];
        ctx.permission_mode = PermissionMode::Restricted;

        let generator = SystemPromptGenerator::new().with_tool_permissions(true);
        let prompt = generator.generate(&ctx);

        assert!(prompt.contains("**Permission Mode**: Restricted"));
        assert!(prompt.contains("`read`"));
        assert!(prompt.contains("`bash` ❌"));
    }

    #[test]
    fn test_custom_sections() {
        let ctx = sample_context();
        let generator =
            SystemPromptGenerator::new().with_custom_section(1, "Custom content here.".to_string());
        let prompt = generator.generate(&ctx);

        assert!(prompt.contains("Custom content here"));
    }

    #[test]
    fn test_token_limit() {
        let ctx = sample_context();
        let generator = SystemPromptGenerator::new().with_max_tokens(20); // Very low limit
        let prompt = generator.generate(&ctx);

        // Should be truncated
        assert!(prompt.len() < 200);
        assert!(prompt.contains("truncated"));
    }

    #[test]
    fn test_section_budget() {
        let mut ctx = ResolvedContext::default();
        ctx.platform_rules = "A".repeat(1000); // Long content

        let generator =
            SystemPromptGenerator::new().with_section_budget(PromptSection::Platform, 50);
        let prompt = generator.generate(&ctx);

        // Should be truncated to budget
        assert!(prompt.len() < 500);
        assert!(prompt.contains("truncated"));
    }

    #[test]
    fn test_loaded_skill_formatting() {
        let mut ctx = ResolvedContext::default();
        ctx.loaded_skills.push(LoadedSkill {
            name: "browser-skill".to_string(),
            content: "Content here".to_string(),
            requires_browser: true,
            automation_tab: true,
        });

        let generator = SystemPromptGenerator::new();
        let prompt = generator.generate(&ctx);

        assert!(prompt.contains("browser-skill (Active)"));
        assert!(prompt.contains("requires browser access"));
        assert!(prompt.contains("automation tab"));
    }

    #[test]
    fn test_estimate_tokens() {
        let content = "This is a test string with about 40 characters.";
        let tokens = SystemPromptGenerator::estimate_tokens(content);
        assert!(tokens > 0);
        assert!(tokens < content.len()); // Should be less than char count
    }

    #[test]
    fn test_prompt_builder() {
        let prompt = PromptBuilder::new()
            .platform("Platform rules")
            .organization("Org conventions")
            .task("Task instructions")
            .build();

        assert!(prompt.contains("Platform rules"));
        assert!(prompt.contains("Org conventions"));
        assert!(prompt.contains("Task instructions"));

        // Verify order
        let platform_pos = prompt.find("Platform rules").unwrap();
        let task_pos = prompt.find("Task instructions").unwrap();
        assert!(platform_pos < task_pos);
    }

    #[test]
    fn test_prompt_builder_custom_separator() {
        let prompt = PromptBuilder::new()
            .with_separator("\n---\n")
            .platform("Platform")
            .task("Task")
            .build();

        assert!(prompt.contains("Platform\n---\nTask"));
    }

    #[test]
    fn test_empty_context() {
        let ctx = ResolvedContext::default();
        let generator = SystemPromptGenerator::new();
        let prompt = generator.generate(&ctx);

        assert!(prompt.is_empty() || prompt.trim().is_empty());
    }

    #[test]
    fn test_section_default_order() {
        assert!(
            PromptSection::Platform.default_order() < PromptSection::Organization.default_order()
        );
        assert!(PromptSection::Organization.default_order() < PromptSection::User.default_order());
        assert!(PromptSection::Task.default_order() < PromptSection::SubAgent.default_order());
        assert!(
            PromptSection::Custom(0).default_order()
                > PromptSection::ToolPermissions.default_order()
        );
    }
}
