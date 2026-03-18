//! Test fixtures for context hierarchy testing
//!
//! Provides reusable test fixtures including temporary directories,
//! platform rules, and skill file creation utilities.

use std::path::PathBuf;
use tempfile::TempDir;

/// Test fixture for context tests
pub struct TestFixture {
    pub temp_dir: TempDir,
    pub platform_rules_path: PathBuf,
    pub skills_dir: PathBuf,
}

impl TestFixture {
    /// Create a new test fixture with temporary directories
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let platform_rules_path = temp_dir.path().join("platform-rules.yaml");
        let skills_dir = temp_dir.path().join("skills");

        // Create skills directory
        std::fs::create_dir_all(&skills_dir).expect("Failed to create skills dir");

        // Create default platform rules
        let default_rules = r#"
language:
  default: "en"
  enforce_english: true
  system_prompt_rule: |
    All responses must be in English.

iteration:
  enabled: true
  max_retries: 3
  auto_record: true
  learning_loop:
    - execute
    - verify
    - diagnose
    - record
    - retry
    - report
  issue_recording:
    deduplicate: true
    similarity_threshold: 0.8
    format: "minimal"

system_prompt:
  platform_rules: |
    ## Platform Rules
    1. All output in English
    2. Verify after each action

context_hierarchy:
  layers:
    - platform
    - organization
    - user
    - session
    - task
    - subagent

skill_loading:
  description_budget: 15000
  on_demand: true
  auto_invoke: false
"#;
        std::fs::write(&platform_rules_path, default_rules).expect("Failed to write platform rules");

        Self {
            temp_dir,
            platform_rules_path,
            skills_dir,
        }
    }

    /// Create a test skill file
    pub fn create_skill(&self, name: &str, content: &str) {
        let skill_path = self.skills_dir.join(format!("{}.md", name));
        std::fs::write(skill_path, content).expect("Failed to write skill file");
    }

    /// Create a test skill with full frontmatter
    pub fn create_skill_with_metadata(
        &self,
        name: &str,
        description: &str,
        requires_browser: bool,
        content: &str,
    ) {
        let full_content = format!(
            r#"---
name: {}
description: |
  {}
requires-browser: {}
automation-tab: {}
---

{}
"#,
            name, description, requires_browser, requires_browser, content
        );
        self.create_skill(name, &full_content);
    }
}

impl Default for TestFixture {
    fn default() -> Self {
        Self::new()
    }
}
