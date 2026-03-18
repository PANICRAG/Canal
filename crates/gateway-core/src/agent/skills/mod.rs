//! Skill System - Claude Agent SDK Compatible Slash Commands
//!
//! Provides a skill system for defining and executing slash commands
//! compatible with Claude Code's `.claude/commands/*.md` format.
//!
//! # Module Structure
//!
//! - `definition` - Core skill types (Skill, SkillMetadata)
//! - `parser` - Parse skills from markdown files with YAML frontmatter
//! - `registry` - Manage and search skills
//! - `executor` - Execute skills with argument substitution
//! - `builtin` - Built-in skills (commit, plan, bug-fix)
//!
//! # Skill File Format
//!
//! Skills are defined in markdown files with YAML frontmatter:
//!
//! ```markdown
//! ---
//! name: commit
//! description: Create a git commit
//! allowed-tools: Bash, Read, Glob
//! argument-hint: [message]
//! ---
//!
//! # Commit Skill
//!
//! Create a git commit with the following message: $ARGUMENTS
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use gateway_core::agent::skills::{SkillRegistry, SkillExecutor, SkillParser};
//!
//! // Load skills from directory
//! let mut registry = SkillRegistry::new();
//! registry.load_from_directory(".claude/commands")?;
//!
//! // Execute a skill
//! let executor = SkillExecutor::new(&registry);
//! let prompt = executor.execute("commit", Some("fix typo"))?;
//! ```

pub mod builtin;
pub use gateway_plugins::skills::definition;
pub mod executor;
pub use gateway_plugins::skills::parser;
pub mod registry;

pub use builtin::{get_builtin_skills, BuiltinSkill};
pub use definition::{Skill, SkillMetadata};
pub use executor::{SkillExecutionResult, SkillExecutor, SkillExecutorBuilder};
pub use parser::{SkillParseError, SkillParser};
pub use registry::{SkillRegistry, SkillRegistryBuilder};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_full_skill_workflow() {
        // Create a temporary directory with a skill file
        let temp_dir = TempDir::new().unwrap();
        let skill_content = r#"---
name: test-skill
description: A test skill
allowed-tools: Bash, Read
argument-hint: [args]
---

# Test Skill

Execute with arguments: $ARGUMENTS
"#;
        let skill_path = temp_dir.path().join("test-skill.md");
        fs::write(&skill_path, skill_content).unwrap();

        // Load skills
        let registry = SkillRegistry::new();
        let count = registry.load_from_directory(temp_dir.path()).unwrap();
        assert_eq!(count, 1);

        // Get and execute skill
        let skill = registry.get("test-skill").unwrap();
        assert_eq!(skill.name, "test-skill");
        assert!(skill.allowed_tools.contains(&"Bash".to_string()));

        // Execute with arguments
        let executor = SkillExecutor::new(&registry);
        let result = executor.prepare("test-skill", Some("hello world")).unwrap();
        assert!(result.prompt.contains("hello world"));
    }

    #[test]
    fn test_builtin_skills_available() {
        let registry = SkillRegistry::with_builtins();

        // Check that builtin skills are available
        assert!(registry.get("commit").is_some());
        assert!(registry.get("plan").is_some());
        assert!(registry.get("bug-fix").is_some());
    }

    #[test]
    fn test_user_skills_override_builtins() {
        let temp_dir = TempDir::new().unwrap();
        let custom_commit = r#"---
name: commit
description: Custom commit skill
allowed-tools: Bash
---

# Custom Commit

Custom implementation
"#;
        fs::write(temp_dir.path().join("commit.md"), custom_commit).unwrap();

        let registry = SkillRegistry::with_builtins();
        registry.load_from_directory(temp_dir.path()).unwrap();

        let skill = registry.get("commit").unwrap();
        assert_eq!(skill.description, "Custom commit skill");
    }
}
