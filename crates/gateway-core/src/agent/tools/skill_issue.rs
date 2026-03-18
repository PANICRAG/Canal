//! Skill Issue Tool - Update skills with learned issues
//!
//! Tool for LLM to record discovered issues during execution.
//! Implements deduplication and minimal efficient language format.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::context::ToolContext;
use super::traits::{AgentTool, ToolError, ToolResult};
use crate::agent::iteration::{LearnedIssue, SkillUpdater};

/// Input for updating skill issues
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateSkillIssueInput {
    /// Skill name (without .md extension)
    pub skill: String,
    /// Error symptom - what went wrong
    pub symptom: String,
    /// Solution - how to fix it
    pub solution: String,
    /// Optional: root cause
    #[serde(default)]
    pub cause: Option<String>,
    /// Optional: verification steps
    #[serde(default)]
    pub verify: Option<String>,
}

/// Output from skill issue update
#[derive(Debug, Clone, Serialize)]
pub struct UpdateSkillIssueOutput {
    /// Whether the issue was added
    pub added: bool,
    /// Message explaining the result
    pub message: String,
    /// Issue ID if added
    pub issue_id: Option<u32>,
}

/// Tool for updating skills with learned issues
pub struct UpdateSkillIssueTool {
    skill_dir: PathBuf,
}

impl UpdateSkillIssueTool {
    /// Create new tool with skill directory
    pub fn new(skill_dir: PathBuf) -> Self {
        Self { skill_dir }
    }

    /// Create with default skill directory
    pub fn default_path() -> Self {
        Self::new(PathBuf::from(".agent/skills"))
    }
}

#[async_trait]
impl AgentTool for UpdateSkillIssueTool {
    type Input = UpdateSkillIssueInput;
    type Output = UpdateSkillIssueOutput;

    fn name(&self) -> &str {
        "update_skill_issue"
    }

    fn description(&self) -> &str {
        "Record a learned issue to a skill file. Checks for duplicates before adding. \
         Use minimal, efficient language - fewer words for maximum clarity."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "Skill name (e.g., 'gmail-automation')"
                },
                "symptom": {
                    "type": "string",
                    "description": "Error symptom - what went wrong (be concise)"
                },
                "solution": {
                    "type": "string",
                    "description": "How to fix it (minimal steps)"
                },
                "cause": {
                    "type": "string",
                    "description": "Root cause (optional)"
                },
                "verify": {
                    "type": "string",
                    "description": "Verification steps (optional)"
                }
            },
            "required": ["skill", "symptom", "solution"]
        })
    }

    fn requires_permission(&self) -> bool {
        // R1-H13: Mutating tools must require permission
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "iteration"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let updater = SkillUpdater::new(self.skill_dir.clone());

        // Check if issue already exists
        let exists = updater
            .issue_exists(&input.skill, &input.symptom)
            .await
            .map_err(|e| ToolError::IoError(e.to_string()))?;

        if exists {
            return Ok(UpdateSkillIssueOutput {
                added: false,
                message: format!("Issue already exists in skill '{}'", input.skill),
                issue_id: None,
            });
        }

        // Create learned issue
        let issue = LearnedIssue {
            symptom: input.symptom,
            cause: input.cause,
            solution: input.solution,
            verify: input.verify,
        };

        // Add issue
        let added = updater
            .add_issue(&input.skill, &issue)
            .await
            .map_err(|e| ToolError::IoError(e.to_string()))?;

        if added {
            Ok(UpdateSkillIssueOutput {
                added: true,
                message: format!("Issue added to skill '{}'", input.skill),
                issue_id: Some(1), // Simplified - actual ID would need parsing
            })
        } else {
            Ok(UpdateSkillIssueOutput {
                added: false,
                message: format!("Skill '{}' not found", input.skill),
                issue_id: None,
            })
        }
    }
}

/// Input for updating skill success rate
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateSkillStatsInput {
    /// Skill name (without .md extension)
    pub skill: String,
    /// Whether the execution was successful
    pub success: bool,
}

/// Output from skill stats update
#[derive(Debug, Clone, Serialize)]
pub struct UpdateSkillStatsOutput {
    /// Whether stats were updated
    pub updated: bool,
    /// Message explaining the result
    pub message: String,
}

/// Tool for updating skill success rates
pub struct UpdateSkillStatsTool {
    skill_dir: PathBuf,
}

impl UpdateSkillStatsTool {
    /// Create new tool with skill directory
    pub fn new(skill_dir: PathBuf) -> Self {
        Self { skill_dir }
    }

    /// Create with default skill directory
    pub fn default_path() -> Self {
        Self::new(PathBuf::from(".agent/skills"))
    }
}

#[async_trait]
impl AgentTool for UpdateSkillStatsTool {
    type Input = UpdateSkillStatsInput;
    type Output = UpdateSkillStatsOutput;

    fn name(&self) -> &str {
        "update_skill_stats"
    }

    fn description(&self) -> &str {
        "Update skill success rate after execution. Call after completing a skill-related task."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "Skill name (e.g., 'gmail-automation')"
                },
                "success": {
                    "type": "boolean",
                    "description": "Whether execution succeeded"
                }
            },
            "required": ["skill", "success"]
        })
    }

    fn requires_permission(&self) -> bool {
        // R1-H13: Mutating tools must require permission
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "iteration"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let updater = SkillUpdater::new(self.skill_dir.clone());

        updater
            .update_stats(&input.skill, input.success)
            .await
            .map_err(|e| ToolError::IoError(e.to_string()))?;

        Ok(UpdateSkillStatsOutput {
            updated: true,
            message: format!(
                "Updated '{}' stats: {}",
                input.skill,
                if input.success { "success" } else { "failure" }
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs;

    #[tokio::test]
    async fn test_update_skill_issue_dedup() {
        let dir = TempDir::new().unwrap();
        let skill_path = dir.path().join("test.md");

        // Create skill file with existing issue
        fs::write(
            &skill_path,
            "# Test Skill\n\n### Issue #1: Timeout\n- Symptom: Connection timeout\n- Solution: Retry\n",
        )
        .await
        .unwrap();

        let tool = UpdateSkillIssueTool::new(dir.path().to_path_buf());
        let context = ToolContext::default();

        // Try to add duplicate
        let input = UpdateSkillIssueInput {
            skill: "test".to_string(),
            symptom: "connection timeout".to_string(), // Same symptom
            solution: "Different solution".to_string(),
            cause: None,
            verify: None,
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert!(!output.added);
        assert!(output.message.contains("already exists"));
    }

    #[tokio::test]
    async fn test_update_skill_issue_new() {
        let dir = TempDir::new().unwrap();
        let skill_path = dir.path().join("test.md");

        // Create empty skill file
        fs::write(&skill_path, "# Test Skill\n").await.unwrap();

        let tool = UpdateSkillIssueTool::new(dir.path().to_path_buf());
        let context = ToolContext::default();

        let input = UpdateSkillIssueInput {
            skill: "test".to_string(),
            symptom: "New error".to_string(),
            solution: "Fix it".to_string(),
            cause: None,
            verify: None,
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert!(output.added);

        // Verify content
        let content = fs::read_to_string(&skill_path).await.unwrap();
        assert!(content.contains("Issue #1"));
        assert!(content.contains("New error"));
    }

    #[tokio::test]
    async fn test_update_skill_stats() {
        let dir = TempDir::new().unwrap();
        let skill_path = dir.path().join("test.md");

        fs::write(&skill_path, "# Test\nsuccess-rate: 50% (1/2)\n")
            .await
            .unwrap();

        let tool = UpdateSkillStatsTool::new(dir.path().to_path_buf());
        let context = ToolContext::default();

        let input = UpdateSkillStatsInput {
            skill: "test".to_string(),
            success: true,
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert!(output.updated);

        // Verify updated rate
        let content = fs::read_to_string(&skill_path).await.unwrap();
        assert!(content.contains("66%") || content.contains("2/3"));
    }
}
