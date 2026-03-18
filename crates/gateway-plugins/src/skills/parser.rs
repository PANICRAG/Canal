//! Skill Parser - Parse skill files from `.claude/commands/*.md` format
//!
//! Parses markdown files with YAML frontmatter into `Skill` objects.

use super::definition::{Skill, SkillMetadata};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

/// Errors that can occur during skill parsing
#[derive(Error, Debug)]
pub enum SkillParseError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    YamlParse(String),

    #[error("Invalid frontmatter: {0}")]
    InvalidFrontmatter(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid skill file: {0}")]
    InvalidFile(String),
}

/// Result type for skill parsing
pub type SkillParseResult<T> = Result<T, SkillParseError>;

/// Raw frontmatter structure from skill files
#[derive(Debug, Default, Deserialize)]
struct SkillFrontmatter {
    /// Skill name
    name: Option<String>,

    /// Skill description
    description: Option<String>,

    /// Allowed tools (comma-separated or array)
    #[serde(alias = "allowed-tools")]
    allowed_tools: Option<ToolsValue>,

    /// Argument hint
    #[serde(alias = "argument-hint")]
    argument_hint: Option<ArgumentHintValue>,

    /// Version
    #[serde(default)]
    version: Option<String>,

    /// Author
    #[serde(default)]
    author: Option<String>,

    /// Tags
    #[serde(default)]
    tags: Vec<String>,

    /// Hidden flag
    #[serde(default)]
    hidden: bool,

    /// Namespace
    #[serde(default)]
    namespace: Option<String>,

    /// Priority
    #[serde(default)]
    priority: Option<i32>,

    /// Dependencies
    #[serde(alias = "depends-on", default)]
    depends_on: Vec<String>,

    /// Slash command tools hidden
    #[serde(alias = "slash-command-tools-hidden", default)]
    slash_command_tools_hidden: bool,

    /// Custom metadata
    #[serde(flatten)]
    custom: HashMap<String, serde_json::Value>,
}

/// Tools value that can be either a comma-separated string or an array
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ToolsValue {
    String(String),
    Array(Vec<String>),
}

impl ToolsValue {
    fn into_vec(self) -> Vec<String> {
        match self {
            ToolsValue::String(s) => s
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect(),
            ToolsValue::Array(arr) => arr,
        }
    }
}

/// Argument hint value that can be a string or array (YAML interprets [foo] as array)
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ArgumentHintValue {
    String(String),
    Array(Vec<String>),
}

impl ArgumentHintValue {
    fn into_string(self) -> String {
        match self {
            ArgumentHintValue::String(s) => s,
            // Convert array back to [a, b, c] format
            ArgumentHintValue::Array(arr) => format!("[{}]", arr.join(", ")),
        }
    }
}

/// Parser for skill files
pub struct SkillParser;

impl SkillParser {
    /// Parse a skill from a markdown string
    pub fn parse(content: &str) -> SkillParseResult<Skill> {
        Self::parse_with_filename(content, None)
    }

    /// Parse a skill with an optional filename for name inference
    pub fn parse_with_filename(content: &str, filename: Option<&str>) -> SkillParseResult<Skill> {
        let (frontmatter_str, markdown_content) = Self::split_frontmatter(content)?;

        let frontmatter: SkillFrontmatter = if frontmatter_str.is_empty() {
            SkillFrontmatter::default()
        } else {
            serde_yaml::from_str(&frontmatter_str)
                .map_err(|e| SkillParseError::YamlParse(e.to_string()))?
        };

        // Determine skill name
        let name = frontmatter
            .name
            .or_else(|| {
                filename.map(|f| {
                    // Remove .md extension and convert to skill name
                    let name = Path::new(f)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(f);
                    name.to_string()
                })
            })
            .ok_or_else(|| SkillParseError::MissingField("name".to_string()))?;

        // Build allowed tools
        let allowed_tools = frontmatter
            .allowed_tools
            .map(|t| t.into_vec())
            .unwrap_or_default();

        // Build metadata
        let mut metadata = SkillMetadata {
            version: frontmatter.version.unwrap_or_else(|| "1.0.0".to_string()),
            author: frontmatter.author,
            tags: frontmatter.tags,
            hidden: frontmatter.hidden,
            builtin: false,
            namespace: frontmatter.namespace,
            source_path: filename.map(|f| f.to_string()),
            slash_command_tools_hidden: frontmatter.slash_command_tools_hidden,
            priority: frontmatter.priority.unwrap_or(0),
            depends_on: frontmatter.depends_on,
            custom: frontmatter.custom,
            plugin_name: None,
        };

        // Remove known fields from custom to avoid duplication
        metadata.custom.remove("name");
        metadata.custom.remove("description");
        metadata.custom.remove("allowed-tools");
        metadata.custom.remove("allowed_tools");
        metadata.custom.remove("argument-hint");
        metadata.custom.remove("argument_hint");
        metadata.custom.remove("version");
        metadata.custom.remove("author");
        metadata.custom.remove("tags");
        metadata.custom.remove("hidden");
        metadata.custom.remove("namespace");
        metadata.custom.remove("priority");
        metadata.custom.remove("depends-on");
        metadata.custom.remove("depends_on");
        metadata.custom.remove("slash-command-tools-hidden");
        metadata.custom.remove("slash_command_tools_hidden");

        Ok(Skill {
            name,
            description: frontmatter.description.unwrap_or_default(),
            allowed_tools,
            argument_hint: frontmatter.argument_hint.map(|v| v.into_string()),
            prompt_template: markdown_content.trim().to_string(),
            metadata,
        })
    }

    /// Parse a skill from a file path
    pub fn parse_file(path: &Path) -> SkillParseResult<Skill> {
        let content = std::fs::read_to_string(path)?;
        let filename = path.file_name().and_then(|n| n.to_str());
        let mut skill = Self::parse_with_filename(&content, filename)?;

        // Set source path
        skill.metadata.source_path = Some(path.to_string_lossy().to_string());

        Ok(skill)
    }

    /// Split frontmatter from markdown content
    fn split_frontmatter(input: &str) -> SkillParseResult<(String, String)> {
        let input = input.trim();

        if !input.starts_with("---") {
            // No frontmatter, entire content is markdown
            return Ok((String::new(), input.to_string()));
        }

        // Find the closing ---
        let after_first = &input[3..];
        let end_pos = after_first.find("\n---");

        match end_pos {
            Some(pos) => {
                let frontmatter = after_first[..pos].trim().to_string();
                let content = after_first[pos + 4..].to_string();
                Ok((frontmatter, content))
            }
            None => Err(SkillParseError::InvalidFrontmatter(
                "Missing closing --- for frontmatter".to_string(),
            )),
        }
    }

    /// Validate a parsed skill
    pub fn validate(skill: &Skill) -> SkillParseResult<()> {
        if skill.name.is_empty() {
            return Err(SkillParseError::MissingField("name".to_string()));
        }

        // Name should be valid identifier-like
        if !skill
            .name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(SkillParseError::InvalidFile(format!(
                "Invalid skill name '{}': must contain only alphanumeric characters, hyphens, and underscores",
                skill.name
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_skill() {
        let content = r#"---
name: commit
description: Create a git commit
allowed-tools: Bash, Read, Glob
argument-hint: [message]
---

# Commit Skill

Create a git commit with the message: $ARGUMENTS
"#;

        let skill = SkillParser::parse(content).unwrap();
        assert_eq!(skill.name, "commit");
        assert_eq!(skill.description, "Create a git commit");
        assert_eq!(skill.allowed_tools, vec!["Bash", "Read", "Glob"]);
        assert_eq!(skill.argument_hint, Some("[message]".to_string()));
        assert!(skill.prompt_template.contains("Create a git commit"));
    }

    #[test]
    fn test_parse_tools_as_array() {
        let content = r#"---
name: test
allowed-tools:
  - Bash
  - Read
  - Glob
---

Test skill
"#;

        let skill = SkillParser::parse(content).unwrap();
        assert_eq!(skill.allowed_tools, vec!["Bash", "Read", "Glob"]);
    }

    #[test]
    fn test_parse_name_from_filename() {
        let content = r#"---
description: A test skill
---

Test content
"#;

        let skill = SkillParser::parse_with_filename(content, Some("my-skill.md")).unwrap();
        assert_eq!(skill.name, "my-skill");
    }

    #[test]
    fn test_parse_no_frontmatter() {
        let content = r#"# Just Markdown

This is just markdown content.
"#;

        let result = SkillParser::parse_with_filename(content, Some("simple.md"));
        assert!(result.is_ok());
        let skill = result.unwrap();
        assert_eq!(skill.name, "simple");
        assert!(skill.prompt_template.contains("Just Markdown"));
    }

    #[test]
    fn test_parse_missing_name() {
        let content = r#"---
description: No name
---

Content
"#;

        let result = SkillParser::parse(content);
        assert!(matches!(
            result.unwrap_err(),
            SkillParseError::MissingField(_)
        ));
    }

    #[test]
    fn test_parse_invalid_yaml() {
        let content = r#"---
name: [invalid yaml
---

Content
"#;

        let result = SkillParser::parse(content);
        assert!(matches!(result.unwrap_err(), SkillParseError::YamlParse(_)));
    }

    #[test]
    fn test_parse_missing_closing_frontmatter() {
        let content = r#"---
name: test
description: No closing

Content continues...
"#;

        let result = SkillParser::parse(content);
        assert!(matches!(
            result.unwrap_err(),
            SkillParseError::InvalidFrontmatter(_)
        ));
    }

    #[test]
    fn test_parse_full_metadata() {
        let content = r#"---
name: full-skill
description: A full skill
version: 2.0.0
author: Test Author
tags:
  - git
  - vcs
hidden: false
namespace: git
priority: 100
depends-on:
  - other-skill
slash-command-tools-hidden: true
---

Full skill content
"#;

        let skill = SkillParser::parse(content).unwrap();
        assert_eq!(skill.metadata.version, "2.0.0");
        assert_eq!(skill.metadata.author, Some("Test Author".to_string()));
        assert_eq!(skill.metadata.tags, vec!["git", "vcs"]);
        assert!(!skill.metadata.hidden);
        assert_eq!(skill.metadata.namespace, Some("git".to_string()));
        assert_eq!(skill.metadata.priority, 100);
        assert_eq!(skill.metadata.depends_on, vec!["other-skill"]);
        assert!(skill.metadata.slash_command_tools_hidden);
    }

    #[test]
    fn test_validate_valid_skill() {
        let skill = Skill::builder("valid-skill").build();
        assert!(SkillParser::validate(&skill).is_ok());
    }

    #[test]
    fn test_validate_empty_name() {
        let skill = Skill::builder("").build();
        assert!(SkillParser::validate(&skill).is_err());
    }

    #[test]
    fn test_validate_invalid_name() {
        let skill = Skill::builder("invalid/name").build();
        assert!(SkillParser::validate(&skill).is_err());
    }

    #[test]
    fn test_parse_with_custom_metadata() {
        let content = r#"---
name: custom
custom-field: custom-value
another-field: 123
---

Content
"#;

        let skill = SkillParser::parse(content).unwrap();
        assert_eq!(
            skill.metadata.custom.get("custom-field"),
            Some(&serde_json::json!("custom-value"))
        );
        assert_eq!(
            skill.metadata.custom.get("another-field"),
            Some(&serde_json::json!(123))
        );
    }

    #[test]
    fn test_tools_value_string() {
        let tools = ToolsValue::String("Bash, Read, Glob".to_string());
        assert_eq!(tools.into_vec(), vec!["Bash", "Read", "Glob"]);
    }

    #[test]
    fn test_tools_value_array() {
        let tools = ToolsValue::Array(vec![
            "Bash".to_string(),
            "Read".to_string(),
            "Glob".to_string(),
        ]);
        assert_eq!(tools.into_vec(), vec!["Bash", "Read", "Glob"]);
    }

    #[test]
    fn test_parse_empty_tools() {
        let content = r#"---
name: no-tools
---

Content
"#;

        let skill = SkillParser::parse(content).unwrap();
        assert!(skill.allowed_tools.is_empty());
    }

    #[test]
    fn test_parse_underscore_fields() {
        let content = r#"---
name: test
allowed_tools: Bash, Read
argument_hint: [arg]
---

Content
"#;

        let skill = SkillParser::parse(content).unwrap();
        assert_eq!(skill.allowed_tools, vec!["Bash", "Read"]);
        assert_eq!(skill.argument_hint, Some("[arg]".to_string()));
    }

    #[test]
    fn test_split_frontmatter_edge_cases() {
        // Test with just dashes content
        let content = "---\nname: test\n---\ncontent with --- in it";
        let (fm, content) = SkillParser::split_frontmatter(content).unwrap();
        assert_eq!(fm, "name: test");
        assert!(content.contains("--- in it"));
    }
}
