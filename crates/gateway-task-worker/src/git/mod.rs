//! Git operations module
//!
//! Provides Git operations using shell commands via tokio::process::Command.

use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::process::Command;
use tracing::{debug, info};

/// Git operation errors
#[derive(Error, Debug)]
pub enum GitError {
    #[error("Git command failed: {0}")]
    CommandFailed(String),

    #[error("Repository not found at path: {0}")]
    RepositoryNotFound(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    ParseError(String),
}

/// Result of a git clone operation
#[derive(Debug, Clone)]
pub struct CloneResult {
    pub path: String,
    pub commit_hash: String,
    pub branch: String,
}

/// Result of a git status operation
#[derive(Debug, Clone)]
pub struct StatusResult {
    pub branch: String,
    pub commit_hash: String,
    pub files: Vec<FileStatus>,
    pub is_clean: bool,
    pub ahead: i32,
    pub behind: i32,
}

/// Status of a file in the git repository
#[derive(Debug, Clone)]
pub struct FileStatus {
    pub path: String,
    pub status: String,
    pub staged_status: String,
}

/// Result of a git commit operation
#[derive(Debug, Clone)]
pub struct CommitResult {
    pub commit_hash: String,
    pub message: String,
    pub files_changed: i32,
}

/// Result of a git diff operation
#[derive(Debug, Clone)]
pub struct DiffResult {
    pub diffs: Vec<FileDiff>,
    pub total_additions: i32,
    pub total_deletions: i32,
}

/// Diff information for a single file
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub status: String,
    pub additions: i32,
    pub deletions: i32,
    pub patch: String,
}

/// Result of a git branch operation
#[derive(Debug, Clone)]
pub struct BranchResult {
    pub current_branch: String,
    pub branches: Vec<String>,
}

/// Branch operation type
#[derive(Debug, Clone)]
pub enum BranchAction {
    List,
    Checkout(String),
    Create(String),
    Delete(String),
}

/// Git executor that runs git commands in a workspace
#[derive(Clone)]
pub struct GitExecutor {
    workspace: PathBuf,
}

impl GitExecutor {
    /// Create a new GitExecutor with the given workspace directory
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }

    /// R8-H8: Validate that a user-supplied path resolves within the workspace.
    /// Prevents path traversal attacks (e.g. "../../etc/passwd").
    fn validate_path(&self, user_path: &str) -> Result<PathBuf, GitError> {
        // Reject obvious traversal patterns before joining
        if user_path.contains("..") {
            return Err(GitError::InvalidPath(format!(
                "Path must not contain '..': {}",
                user_path
            )));
        }
        let resolved = self.workspace.join(user_path);
        // Canonicalize the workspace (must exist) and check prefix
        if let Ok(canonical_workspace) = self.workspace.canonicalize() {
            // For existing paths, canonicalize and check containment
            if let Ok(canonical_resolved) = resolved.canonicalize() {
                if !canonical_resolved.starts_with(&canonical_workspace) {
                    return Err(GitError::InvalidPath(format!(
                        "Path escapes workspace: {}",
                        user_path
                    )));
                }
            }
            // For non-existing paths (e.g. clone target), the `..` check above suffices
        }
        Ok(resolved)
    }

    /// Run a git command and return its output
    async fn run_git_command(&self, args: &[&str], cwd: &Path) -> Result<String, GitError> {
        debug!(args = ?args, cwd = ?cwd, "Running git command");

        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .await?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Err(GitError::CommandFailed(stderr))
        }
    }

    /// Clone a git repository
    pub async fn clone(
        &self,
        url: &str,
        target: &str,
        branch: Option<&str>,
        depth: Option<i32>,
    ) -> Result<CloneResult, GitError> {
        // R8-H8: Validate target path to prevent path traversal
        let target_path = self.validate_path(target)?;
        info!(url = %url, target = ?target_path, "Cloning repository");

        let mut args = vec!["clone"];

        if let Some(b) = branch {
            args.push("--branch");
            args.push(b);
        }

        let depth_string;
        if let Some(d) = depth {
            if d > 0 {
                args.push("--depth");
                depth_string = d.to_string();
                args.push(&depth_string);
            }
        }

        args.push(url);
        let target_str = target_path.to_string_lossy();
        args.push(&target_str);

        self.run_git_command(&args, &self.workspace).await?;

        // Get the current commit hash
        let commit_hash = self
            .run_git_command(&["rev-parse", "HEAD"], &target_path)
            .await?
            .trim()
            .to_string();

        // Get the current branch
        let current_branch = self
            .run_git_command(&["rev-parse", "--abbrev-ref", "HEAD"], &target_path)
            .await?
            .trim()
            .to_string();

        Ok(CloneResult {
            path: target_path.to_string_lossy().to_string(),
            commit_hash,
            branch: current_branch,
        })
    }

    /// Get the status of a git repository
    pub async fn status(&self, path: &str) -> Result<StatusResult, GitError> {
        // R8-H8: Validate path to prevent path traversal
        let repo_path = self.validate_path(path)?;
        info!(path = ?repo_path, "Getting git status");

        if !repo_path.exists() {
            return Err(GitError::RepositoryNotFound(
                repo_path.to_string_lossy().to_string(),
            ));
        }

        // Get branch name
        let branch = self
            .run_git_command(&["rev-parse", "--abbrev-ref", "HEAD"], &repo_path)
            .await?
            .trim()
            .to_string();

        // Get commit hash
        let commit_hash = self
            .run_git_command(&["rev-parse", "HEAD"], &repo_path)
            .await
            .unwrap_or_default()
            .trim()
            .to_string();

        // Get status with porcelain format for parsing
        let status_output = self
            .run_git_command(&["status", "--porcelain", "-b"], &repo_path)
            .await?;

        let mut files = Vec::new();
        let mut ahead = 0;
        let mut behind = 0;

        for line in status_output.lines() {
            if line.starts_with("##") {
                // Parse branch tracking info
                if let Some(tracking) = line.split("...").nth(1) {
                    if let Some(ahead_behind) = tracking.split('[').nth(1) {
                        let parts: Vec<&str> =
                            ahead_behind.trim_end_matches(']').split(", ").collect();
                        for part in parts {
                            if part.starts_with("ahead ") {
                                ahead = part[6..].parse().unwrap_or(0);
                            } else if part.starts_with("behind ") {
                                behind = part[7..].parse().unwrap_or(0);
                            }
                        }
                    }
                }
            } else if line.len() >= 3 {
                // Parse file status
                let staged = &line[0..1];
                let unstaged = &line[1..2];
                let file_path = line[3..].trim().to_string();

                let status = match unstaged {
                    "M" => "modified",
                    "D" => "deleted",
                    "?" => "untracked",
                    "A" => "added",
                    "R" => "renamed",
                    "C" => "copied",
                    "U" => "unmerged",
                    _ => "unknown",
                }
                .to_string();

                let staged_status = match staged {
                    "M" => "modified",
                    "D" => "deleted",
                    "A" => "added",
                    "R" => "renamed",
                    "C" => "copied",
                    "?" => "",
                    _ => "",
                }
                .to_string();

                files.push(FileStatus {
                    path: file_path,
                    status,
                    staged_status,
                });
            }
        }

        let is_clean = files.is_empty();

        Ok(StatusResult {
            branch,
            commit_hash,
            files,
            is_clean,
            ahead,
            behind,
        })
    }

    /// Create a git commit
    pub async fn commit(
        &self,
        path: &str,
        message: &str,
        files: &[String],
        author_name: Option<&str>,
        author_email: Option<&str>,
    ) -> Result<CommitResult, GitError> {
        // R8-H8: Validate path to prevent path traversal
        let repo_path = self.validate_path(path)?;
        info!(path = ?repo_path, message = %message, "Creating git commit");

        if !repo_path.exists() {
            return Err(GitError::RepositoryNotFound(
                repo_path.to_string_lossy().to_string(),
            ));
        }

        // Stage files
        if files.is_empty() {
            // Stage all changes
            self.run_git_command(&["add", "-A"], &repo_path).await?;
        } else {
            // Stage specific files
            for file in files {
                self.run_git_command(&["add", file], &repo_path).await?;
            }
        }

        // Build commit command
        let mut args = vec!["commit", "-m", message];

        // Add author info if provided
        let author_string;
        if let (Some(name), Some(email)) = (author_name, author_email) {
            author_string = format!("{} <{}>", name, email);
            args.push("--author");
            args.push(&author_string);
        }

        self.run_git_command(&args, &repo_path).await?;

        // Get the new commit hash
        let commit_hash = self
            .run_git_command(&["rev-parse", "HEAD"], &repo_path)
            .await?
            .trim()
            .to_string();

        // Get number of files changed
        let diff_stat = self
            .run_git_command(&["diff", "--stat", "HEAD~1..HEAD"], &repo_path)
            .await
            .unwrap_or_default();

        let files_changed = diff_stat.lines().filter(|line| line.contains('|')).count() as i32;

        Ok(CommitResult {
            commit_hash,
            message: message.to_string(),
            files_changed,
        })
    }

    /// Get git diff
    pub async fn diff(
        &self,
        path: &str,
        base_ref: Option<&str>,
        target_ref: Option<&str>,
        files: &[String],
    ) -> Result<DiffResult, GitError> {
        // R8-H8: Validate path to prevent path traversal
        let repo_path = self.validate_path(path)?;
        info!(path = ?repo_path, base_ref = ?base_ref, target_ref = ?target_ref, "Getting git diff");

        if !repo_path.exists() {
            return Err(GitError::RepositoryNotFound(
                repo_path.to_string_lossy().to_string(),
            ));
        }

        // Build diff command
        let mut args = vec!["diff"];

        // R8-H7: Validate refs don't start with '-' to prevent git flag injection
        if let Some(base) = base_ref {
            if base.starts_with('-') {
                return Err(GitError::InvalidPath(format!(
                    "Ref must not start with '-': {}",
                    base
                )));
            }
            args.push(base);
        }

        if let Some(target) = target_ref {
            if target.starts_with('-') {
                return Err(GitError::InvalidPath(format!(
                    "Ref must not start with '-': {}",
                    target
                )));
            }
            args.push(target);
        }

        // Add specific files if provided
        if !files.is_empty() {
            args.push("--");
            for file in files {
                args.push(file);
            }
        }

        let diff_output = self.run_git_command(&args, &repo_path).await?;

        // Get numstat for additions/deletions
        let mut stat_args = vec!["diff", "--numstat"];
        if let Some(base) = base_ref {
            stat_args.push(base);
        }
        if let Some(target) = target_ref {
            stat_args.push(target);
        }
        if !files.is_empty() {
            stat_args.push("--");
            for file in files {
                stat_args.push(file);
            }
        }

        let numstat_output = self.run_git_command(&stat_args, &repo_path).await?;

        let mut diffs = Vec::new();
        let mut total_additions = 0;
        let mut total_deletions = 0;

        // Parse numstat output
        for line in numstat_output.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                let additions: i32 = parts[0].parse().unwrap_or(0);
                let deletions: i32 = parts[1].parse().unwrap_or(0);
                let file_path = parts[2].to_string();

                total_additions += additions;
                total_deletions += deletions;

                // Extract patch for this file from the full diff
                let patch = extract_file_patch(&diff_output, &file_path);

                diffs.push(FileDiff {
                    path: file_path,
                    status: if additions > 0 && deletions > 0 {
                        "modified".to_string()
                    } else if additions > 0 {
                        "added".to_string()
                    } else {
                        "deleted".to_string()
                    },
                    additions,
                    deletions,
                    patch,
                });
            }
        }

        Ok(DiffResult {
            diffs,
            total_additions,
            total_deletions,
        })
    }

    /// Perform branch operations
    pub async fn branch(&self, path: &str, action: BranchAction) -> Result<BranchResult, GitError> {
        // R8-H8: Validate path to prevent path traversal
        let repo_path = self.validate_path(path)?;
        info!(path = ?repo_path, action = ?action, "Git branch operation");

        if !repo_path.exists() {
            return Err(GitError::RepositoryNotFound(
                repo_path.to_string_lossy().to_string(),
            ));
        }

        match action {
            BranchAction::List => {
                // List all branches
                let output = self.run_git_command(&["branch", "-a"], &repo_path).await?;

                let branches: Vec<String> = output
                    .lines()
                    .map(|line| line.trim().trim_start_matches("* ").to_string())
                    .filter(|line| !line.is_empty())
                    .collect();

                let current_branch = self
                    .run_git_command(&["rev-parse", "--abbrev-ref", "HEAD"], &repo_path)
                    .await?
                    .trim()
                    .to_string();

                Ok(BranchResult {
                    current_branch,
                    branches,
                })
            }
            BranchAction::Checkout(name) => {
                self.run_git_command(&["checkout", &name], &repo_path)
                    .await?;

                let branches = self.get_branch_list(&repo_path).await?;

                Ok(BranchResult {
                    current_branch: name,
                    branches,
                })
            }
            BranchAction::Create(name) => {
                self.run_git_command(&["branch", &name], &repo_path).await?;

                let current_branch = self
                    .run_git_command(&["rev-parse", "--abbrev-ref", "HEAD"], &repo_path)
                    .await?
                    .trim()
                    .to_string();

                let branches = self.get_branch_list(&repo_path).await?;

                Ok(BranchResult {
                    current_branch,
                    branches,
                })
            }
            BranchAction::Delete(name) => {
                self.run_git_command(&["branch", "-d", &name], &repo_path)
                    .await?;

                let current_branch = self
                    .run_git_command(&["rev-parse", "--abbrev-ref", "HEAD"], &repo_path)
                    .await?
                    .trim()
                    .to_string();

                let branches = self.get_branch_list(&repo_path).await?;

                Ok(BranchResult {
                    current_branch,
                    branches,
                })
            }
        }
    }

    /// Helper to get list of branches
    async fn get_branch_list(&self, repo_path: &Path) -> Result<Vec<String>, GitError> {
        let output = self.run_git_command(&["branch", "-a"], repo_path).await?;

        Ok(output
            .lines()
            .map(|line| line.trim().trim_start_matches("* ").to_string())
            .filter(|line| !line.is_empty())
            .collect())
    }
}

/// Extract the patch for a specific file from the full diff output
fn extract_file_patch(full_diff: &str, file_path: &str) -> String {
    let mut in_file = false;
    let mut patch_lines = Vec::new();
    let file_header = format!("diff --git a/{}", file_path);
    let alt_file_header = format!("diff --git b/{}", file_path);

    for line in full_diff.lines() {
        if line.starts_with("diff --git") {
            if line.contains(&file_header)
                || line.contains(&alt_file_header)
                || line.ends_with(&format!(" b/{}", file_path))
            {
                in_file = true;
                patch_lines.push(line.to_string());
            } else {
                in_file = false;
            }
        } else if in_file {
            patch_lines.push(line.to_string());
        }
    }

    patch_lines.join("\n")
}
