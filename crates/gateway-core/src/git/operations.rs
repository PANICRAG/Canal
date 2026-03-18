//! Git operations (commit, diff, branch, etc.)

use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;

use crate::error::{Error, Result};

/// Git status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatus {
    pub branch: String,
    pub commit: Option<String>,
    pub is_clean: bool,
    pub files: Vec<GitFileStatus>,
    pub ahead: u32,
    pub behind: u32,
}

/// File status in git
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileStatus {
    pub path: String,
    pub status: String,
    pub staged: bool,
}

/// Git diff response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiff {
    pub files: Vec<GitFileDiff>,
    pub total_additions: u32,
    pub total_deletions: u32,
}

/// File diff
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitFileDiff {
    pub path: String,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    pub patch: String,
}

/// Git operations handler
pub struct GitOperations {
    repo_path: std::path::PathBuf,
}

impl GitOperations {
    /// Create a new git operations handler
    pub fn new(repo_path: impl AsRef<Path>) -> Self {
        Self {
            repo_path: repo_path.as_ref().to_path_buf(),
        }
    }

    /// Get repository status
    pub async fn status(&self) -> Result<GitStatus> {
        // Get branch
        let branch = self.get_branch().await?;

        // Get commit
        let commit = self.get_head_commit().await?;

        // Get status
        let output = Command::new("git")
            .args(["status", "--porcelain=v1"])
            .current_dir(&self.repo_path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get status: {}", e)))?;

        let status_output = String::from_utf8_lossy(&output.stdout);
        let files: Vec<GitFileStatus> = status_output
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let status_chars = &line[0..2];
                let path = line[3..].to_string();
                let staged = !status_chars.starts_with(' ') && !status_chars.starts_with('?');
                let status = match status_chars.trim() {
                    "M" | " M" => "modified",
                    "A" | " A" => "added",
                    "D" | " D" => "deleted",
                    "R" => "renamed",
                    "??" => "untracked",
                    _ => "unknown",
                };
                GitFileStatus {
                    path,
                    status: status.to_string(),
                    staged,
                }
            })
            .collect();

        let is_clean = files.is_empty();

        Ok(GitStatus {
            branch,
            commit,
            is_clean,
            files,
            ahead: 0,
            behind: 0,
        })
    }

    /// Get diff
    pub async fn diff(&self, staged: bool) -> Result<GitDiff> {
        let mut args = vec!["diff", "--numstat"];
        if staged {
            args.push("--staged");
        }

        let output = Command::new("git")
            .args(&args)
            .current_dir(&self.repo_path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get diff: {}", e)))?;

        let numstat_output = String::from_utf8_lossy(&output.stdout);
        let mut total_additions = 0u32;
        let mut total_deletions = 0u32;
        let mut files = Vec::new();

        for line in numstat_output.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                let additions: u32 = parts[0].parse().unwrap_or(0);
                let deletions: u32 = parts[1].parse().unwrap_or(0);
                let path = parts[2].to_string();

                total_additions += additions;
                total_deletions += deletions;

                // Get patch for this file
                let patch = self.get_file_patch(&path, staged).await.unwrap_or_default();

                files.push(GitFileDiff {
                    path,
                    status: "modified".to_string(),
                    additions,
                    deletions,
                    patch,
                });
            }
        }

        Ok(GitDiff {
            files,
            total_additions,
            total_deletions,
        })
    }

    /// Get patch for a specific file
    async fn get_file_patch(&self, path: &str, staged: bool) -> Result<String> {
        let mut args = vec!["diff"];
        if staged {
            args.push("--staged");
        }
        args.push("--");
        args.push(path);

        let output = Command::new("git")
            .args(&args)
            .current_dir(&self.repo_path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get patch: {}", e)))?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Create a commit
    pub async fn commit(&self, message: &str, files: Option<Vec<String>>) -> Result<String> {
        // Stage files
        if let Some(files) = files {
            for file in files {
                Command::new("git")
                    .args(["add", &file])
                    .current_dir(&self.repo_path)
                    .output()
                    .await
                    .map_err(|e| Error::Internal(format!("Failed to stage file: {}", e)))?;
            }
        } else {
            // Stage all changes
            Command::new("git")
                .args(["add", "-A"])
                .current_dir(&self.repo_path)
                .output()
                .await
                .map_err(|e| Error::Internal(format!("Failed to stage changes: {}", e)))?;
        }

        // Create commit
        let output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(&self.repo_path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to commit: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Commit failed: {}", stderr)));
        }

        // Get the new commit hash
        let commit = self
            .get_head_commit()
            .await?
            .ok_or_else(|| Error::Internal("Failed to get commit hash".to_string()))?;

        tracing::info!(commit = %commit, message = %message, "Commit created");

        Ok(commit)
    }

    /// Switch branch
    pub async fn checkout(&self, branch: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["checkout", branch])
            .current_dir(&self.repo_path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to checkout: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Checkout failed: {}", stderr)));
        }

        tracing::info!(branch = %branch, "Switched to branch");

        Ok(())
    }

    /// Create a new branch
    pub async fn create_branch(&self, branch: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["checkout", "-b", branch])
            .current_dir(&self.repo_path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to create branch: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Create branch failed: {}", stderr)));
        }

        tracing::info!(branch = %branch, "Created new branch");

        Ok(())
    }

    /// List branches
    pub async fn list_branches(&self) -> Result<Vec<String>> {
        let output = Command::new("git")
            .args(["branch", "--list"])
            .current_dir(&self.repo_path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to list branches: {}", e)))?;

        let branches: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.trim().trim_start_matches("* ").to_string())
            .filter(|b| !b.is_empty())
            .collect();

        Ok(branches)
    }

    /// Pull changes
    pub async fn pull(&self) -> Result<()> {
        let output = Command::new("git")
            .args(["pull"])
            .current_dir(&self.repo_path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to pull: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Pull failed: {}", stderr)));
        }

        Ok(())
    }

    /// Push changes
    pub async fn push(&self, set_upstream: bool) -> Result<()> {
        let mut args = vec!["push"];
        if set_upstream {
            args.extend(["--set-upstream", "origin", "HEAD"]);
        }

        let output = Command::new("git")
            .args(&args)
            .current_dir(&self.repo_path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to push: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Internal(format!("Push failed: {}", stderr)));
        }

        Ok(())
    }

    /// Get current branch
    async fn get_branch(&self) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&self.repo_path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get branch: {}", e)))?;

        if !output.status.success() {
            return Ok("main".to_string());
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Get HEAD commit hash
    async fn get_head_commit(&self) -> Result<Option<String>> {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&self.repo_path)
            .output()
            .await
            .map_err(|e| Error::Internal(format!("Failed to get commit: {}", e)))?;

        if !output.status.success() {
            return Ok(None);
        }

        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ))
    }
}
