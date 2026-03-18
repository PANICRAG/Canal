//! Skill Updater - Updates skills with learned issues

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Learned issue from execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedIssue {
    pub symptom: String,
    pub cause: Option<String>,
    pub solution: String,
    pub verify: Option<String>,
}

/// Updates skill files with new issues and stats
#[derive(Clone)]
pub struct SkillUpdater {
    skill_dir: PathBuf,
}

impl SkillUpdater {
    pub fn new(skill_dir: PathBuf) -> Self {
        Self { skill_dir }
    }

    /// Check if issue already exists in skill
    pub async fn issue_exists(&self, skill: &str, symptom: &str) -> Result<bool, std::io::Error> {
        let path = self.skill_dir.join(format!("{}.md", skill));
        if !path.exists() {
            return Ok(false);
        }

        let content = fs::read_to_string(&path).await?;
        let symptom_lower = symptom.to_lowercase();

        // Check existing issues for similar symptom
        Ok(content.to_lowercase().contains(&symptom_lower))
    }

    /// Add new issue to skill (only if not exists)
    pub async fn add_issue(
        &self,
        skill: &str,
        issue: &LearnedIssue,
    ) -> Result<bool, std::io::Error> {
        // Check existence first
        if self.issue_exists(skill, &issue.symptom).await? {
            tracing::debug!("Issue already exists in {}: {}", skill, issue.symptom);
            return Ok(false);
        }

        let path = self.skill_dir.join(format!("{}.md", skill));
        if !path.exists() {
            return Ok(false);
        }

        let content = fs::read_to_string(&path).await?;
        let next_id = self.next_issue_id(&content);

        // Minimal format - only essential info
        let issue_md = format!(
            "\n### Issue #{}: {}\n- Symptom: {}\n- Solution: {}\n",
            next_id,
            issue
                .symptom
                .split_whitespace()
                .take(5)
                .collect::<Vec<_>>()
                .join(" "),
            issue.symptom,
            issue.solution,
        );

        let mut file = fs::OpenOptions::new().append(true).open(&path).await?;
        file.write_all(issue_md.as_bytes()).await?;

        tracing::info!("Added Issue #{} to {}", next_id, skill);
        Ok(true)
    }

    /// Update success rate
    pub async fn update_stats(&self, skill: &str, success: bool) -> Result<(), std::io::Error> {
        let path = self.skill_dir.join(format!("{}.md", skill));
        if !path.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(&path).await?;
        let (s, t) = self.parse_stats(&content);
        let new_s = s + if success { 1 } else { 0 };
        let new_t = t + 1;
        let rate = (new_s as f32 / new_t as f32 * 100.0) as u32;

        let new_content = self.update_field(
            &content,
            "success-rate",
            &format!("{}% ({}/{})", rate, new_s, new_t),
        );

        fs::write(&path, new_content).await?;
        Ok(())
    }

    fn next_issue_id(&self, content: &str) -> u32 {
        let re = Regex::new(r"### Issue #(\d+)").unwrap();
        re.captures_iter(content)
            .filter_map(|c| c[1].parse::<u32>().ok())
            .max()
            .unwrap_or(0)
            + 1
    }

    fn parse_stats(&self, content: &str) -> (u32, u32) {
        let re = Regex::new(r"success-rate:\s*\d+%\s*\((\d+)/(\d+)\)").unwrap();
        re.captures(content)
            .map(|c| (c[1].parse().unwrap_or(0), c[2].parse().unwrap_or(1)))
            .unwrap_or((0, 0))
    }

    fn update_field(&self, content: &str, field: &str, value: &str) -> String {
        let re = Regex::new(&format!(r"{}:\s*[^\n]+", field)).unwrap();
        if re.is_match(content) {
            re.replace(content, format!("{}: {}", field, value))
                .to_string()
        } else {
            content.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_issue_exists() {
        let dir = TempDir::new().unwrap();
        let updater = SkillUpdater::new(dir.path().to_path_buf());

        fs::write(
            dir.path().join("test.md"),
            "# Test\n### Issue #1: Timeout\n- Symptom: Connection timeout\n",
        )
        .await
        .unwrap();

        assert!(updater
            .issue_exists("test", "connection timeout")
            .await
            .unwrap());
        assert!(!updater.issue_exists("test", "new error").await.unwrap());
    }
}
