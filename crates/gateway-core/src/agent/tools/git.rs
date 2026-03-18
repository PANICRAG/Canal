//! Git Tools - Git repository operations for agent
//!
//! Provides tools for common git operations:
//! - GitStatusTool - Get repository status
//! - GitDiffTool - Show file diffs
//! - GitLogTool - View commit history
//! - GitBranchTool - List/switch branches
//! - GitCommitTool - Create commits (requires permission)

use super::{AgentTool, ToolContext, ToolError, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

// ============================================================================
// Common Helpers
// ============================================================================

/// Execute a git command and return the output
async fn execute_git_command(
    args: &[&str],
    cwd: &Path,
    timeout_ms: Option<u64>,
) -> ToolResult<(String, i32)> {
    let timeout_duration =
        tokio::time::Duration::from_millis(timeout_ms.unwrap_or(30000).min(60000));

    let mut cmd = Command::new("git");
    cmd.args(args);
    cmd.current_dir(cwd);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let result = tokio::time::timeout(timeout_duration, async {
        let output = cmd
            .output()
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to execute git: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Combine stdout and stderr, preferring stdout if both exist
        let combined_output = if stdout.is_empty() {
            stderr
        } else if stderr.is_empty() {
            stdout
        } else {
            format!("{}\n{}", stdout, stderr)
        };

        Ok((combined_output, output.status.code().unwrap_or(-1)))
    })
    .await;

    match result {
        Ok(res) => res,
        Err(_) => Err(ToolError::Timeout("Git command timed out".to_string())),
    }
}

/// Check if a directory is inside a git repository
async fn is_git_repository(path: &Path) -> bool {
    let result = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(path)
        .output()
        .await;

    matches!(result, Ok(output) if output.status.success())
}

// ============================================================================
// GitStatus Tool
// ============================================================================

/// GitStatus tool input
#[derive(Debug, Clone, Deserialize)]
pub struct GitStatusInput {
    /// Optional path to check status for
    #[serde(default)]
    pub path: Option<String>,
    /// Show short format
    #[serde(default)]
    pub short: bool,
    /// Show branch info
    #[serde(default = "default_true")]
    pub branch: bool,
}

fn default_true() -> bool {
    true
}

/// GitStatus tool output
#[derive(Debug, Clone, Serialize)]
pub struct GitStatusOutput {
    /// Raw status output
    pub status: String,
    /// Current branch name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Whether there are uncommitted changes
    pub has_changes: bool,
    /// Number of staged files
    pub staged_count: u32,
    /// Number of modified files
    pub modified_count: u32,
    /// Number of untracked files
    pub untracked_count: u32,
}

/// Git status tool - shows working tree status
pub struct GitStatusTool;

#[async_trait]
impl AgentTool for GitStatusTool {
    type Input = GitStatusInput;
    type Output = GitStatusOutput;

    fn name(&self) -> &str {
        "GitStatus"
    }

    fn description(&self) -> &str {
        "Get the status of a git repository. Shows staged, modified, and untracked files."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional path within the repository to check status for"
                },
                "short": {
                    "type": "boolean",
                    "description": "Use short format output"
                },
                "branch": {
                    "type": "boolean",
                    "description": "Include branch information (default: true)"
                }
            }
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "git"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        let cwd = if let Some(ref path) = input.path {
            context.resolve_path(path)
        } else {
            context.cwd.clone()
        };

        // Check if it's a git repository
        if !is_git_repository(&cwd).await {
            return Err(ToolError::InvalidInput(format!(
                "Not a git repository: {}",
                cwd.display()
            )));
        }

        // Build args
        let mut args = vec!["status"];
        if input.short {
            args.push("-s");
        }
        if input.branch {
            args.push("-b");
        }

        let (status, exit_code) = execute_git_command(&args, &cwd, None).await?;

        if exit_code != 0 {
            return Err(ToolError::ExecutionError(format!(
                "git status failed: {}",
                status
            )));
        }

        // Get branch name
        let branch = if input.branch {
            let (branch_output, _) =
                execute_git_command(&["rev-parse", "--abbrev-ref", "HEAD"], &cwd, None).await?;
            Some(branch_output.trim().to_string())
        } else {
            None
        };

        // R1-M1: parse_status_counts requires short format; if not already short,
        // do a separate short-format call for accurate counts
        let count_source = if input.short {
            status.clone()
        } else {
            let (short_status, _) = execute_git_command(&["status", "-s"], &cwd, None).await?;
            short_status
        };
        let (staged, modified, untracked) = parse_status_counts(&count_source);

        Ok(GitStatusOutput {
            status,
            branch,
            has_changes: staged > 0 || modified > 0 || untracked > 0,
            staged_count: staged,
            modified_count: modified,
            untracked_count: untracked,
        })
    }
}

/// Parse git status output to count files
/// This parses short format status output (git status -s)
fn parse_status_counts(status: &str) -> (u32, u32, u32) {
    let mut staged = 0u32;
    let mut modified = 0u32;
    let mut untracked = 0u32;

    for line in status.lines() {
        // Short format: XY filename
        // X = index status, Y = worktree status
        // Skip lines that don't look like short format (need at least 3 chars: XY + space + filename)
        if line.len() < 3 {
            continue;
        }

        let chars: Vec<char> = line.chars().collect();
        let index_status = chars[0];
        let worktree_status = chars[1];

        // Skip header lines and other non-status lines
        // Short format always has exactly 2 status chars followed by a space
        if chars.len() >= 3 && chars[2] != ' ' {
            // This is not a short format line (e.g., "On branch main")
            continue;
        }

        // Staged changes (first column: A, M, D, R, C)
        if matches!(index_status, 'A' | 'M' | 'D' | 'R' | 'C') {
            staged += 1;
        }

        // Working tree changes (second column: M, D)
        if matches!(worktree_status, 'M' | 'D') {
            modified += 1;
        }

        // Untracked files
        if index_status == '?' && worktree_status == '?' {
            untracked += 1;
        }
    }

    (staged, modified, untracked)
}

// ============================================================================
// GitDiff Tool
// ============================================================================

/// GitDiff tool input
#[derive(Debug, Clone, Deserialize)]
pub struct GitDiffInput {
    /// File path to diff (optional, shows all if not specified)
    #[serde(default)]
    pub file_path: Option<String>,
    /// Show staged changes (--cached)
    #[serde(default)]
    pub staged: bool,
    /// Compare with a specific commit or branch
    #[serde(default)]
    pub base: Option<String>,
    /// Show only stat summary
    #[serde(default)]
    pub stat: bool,
    /// Maximum lines to return
    #[serde(default)]
    pub max_lines: Option<u32>,
}

/// GitDiff tool output
#[derive(Debug, Clone, Serialize)]
pub struct GitDiffOutput {
    /// The diff output
    pub diff: String,
    /// Number of files changed
    pub files_changed: u32,
    /// Number of insertions
    pub insertions: u32,
    /// Number of deletions
    pub deletions: u32,
    /// Whether output was truncated
    pub truncated: bool,
}

/// Git diff tool - shows changes between commits, commit and working tree, etc.
pub struct GitDiffTool {
    /// Maximum output lines
    pub max_lines: u32,
}

impl Default for GitDiffTool {
    fn default() -> Self {
        Self { max_lines: 1000 }
    }
}

#[async_trait]
impl AgentTool for GitDiffTool {
    type Input = GitDiffInput;
    type Output = GitDiffOutput;

    fn name(&self) -> &str {
        "GitDiff"
    }

    fn description(&self) -> &str {
        "Show changes between commits, commit and working tree, etc. Can show staged or unstaged changes."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Optional file path to show diff for"
                },
                "staged": {
                    "type": "boolean",
                    "description": "Show staged changes (--cached)"
                },
                "base": {
                    "type": "string",
                    "description": "Compare with a specific commit, branch, or ref"
                },
                "stat": {
                    "type": "boolean",
                    "description": "Show only diffstat summary"
                },
                "max_lines": {
                    "type": "integer",
                    "description": "Maximum lines to return in output"
                }
            }
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "git"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        let cwd = &context.cwd;

        // Check if it's a git repository
        if !is_git_repository(cwd).await {
            return Err(ToolError::InvalidInput(format!(
                "Not a git repository: {}",
                cwd.display()
            )));
        }

        // Build args for diff
        let mut args = vec!["diff"];

        if input.staged {
            args.push("--cached");
        }

        if input.stat {
            args.push("--stat");
        }

        if let Some(ref base) = input.base {
            args.push(base);
        }

        if let Some(ref file_path) = input.file_path {
            args.push("--");
            args.push(file_path);
        }

        let (diff, exit_code) = execute_git_command(&args, cwd, None).await?;

        if exit_code != 0 {
            return Err(ToolError::ExecutionError(format!(
                "git diff failed: {}",
                diff
            )));
        }

        // Get diff stat for summary
        let (files_changed, insertions, deletions) = if !input.stat {
            let stat_args = if input.staged {
                vec!["diff", "--cached", "--shortstat"]
            } else if let Some(ref base) = input.base {
                vec!["diff", "--shortstat", base.as_str()]
            } else {
                vec!["diff", "--shortstat"]
            };
            let (stat_output, _) = execute_git_command(&stat_args, cwd, None).await?;
            parse_diff_stat(&stat_output)
        } else {
            parse_diff_stat(&diff)
        };

        // Truncate if needed
        let max_lines = input.max_lines.unwrap_or(self.max_lines);
        let lines: Vec<&str> = diff.lines().collect();
        let truncated = lines.len() > max_lines as usize;
        let output_diff = if truncated {
            let mut result: String = lines[..max_lines as usize].join("\n");
            result.push_str("\n... (output truncated)");
            result
        } else {
            diff
        };

        Ok(GitDiffOutput {
            diff: output_diff,
            files_changed,
            insertions,
            deletions,
            truncated,
        })
    }
}

/// Parse git diff --shortstat output
fn parse_diff_stat(stat: &str) -> (u32, u32, u32) {
    let mut files = 0u32;
    let mut insertions = 0u32;
    let mut deletions = 0u32;

    // Example: " 3 files changed, 10 insertions(+), 5 deletions(-)"
    for line in stat.lines() {
        if line.contains("changed") {
            // Parse files changed
            if let Some(num) = line.split_whitespace().next() {
                files = num.parse().unwrap_or(0);
            }

            // Parse insertions
            if let Some(pos) = line.find("insertion") {
                let before: String = line[..pos]
                    .chars()
                    .rev()
                    .take_while(|c| c.is_ascii_digit() || *c == ' ')
                    .collect();
                let num_str: String = before.chars().rev().collect();
                insertions = num_str.trim().parse().unwrap_or(0);
            }

            // Parse deletions
            if let Some(pos) = line.find("deletion") {
                let before: String = line[..pos]
                    .chars()
                    .rev()
                    .take_while(|c| c.is_ascii_digit() || *c == ' ')
                    .collect();
                let num_str: String = before.chars().rev().collect();
                deletions = num_str.trim().parse().unwrap_or(0);
            }
        }
    }

    (files, insertions, deletions)
}

// ============================================================================
// GitLog Tool
// ============================================================================

/// GitLog tool input
#[derive(Debug, Clone, Deserialize)]
pub struct GitLogInput {
    /// Number of commits to show
    #[serde(default)]
    pub count: Option<u32>,
    /// File path to show history for
    #[serde(default)]
    pub file_path: Option<String>,
    /// Branch or ref to show history for
    #[serde(default)]
    pub branch: Option<String>,
    /// Show oneline format
    #[serde(default)]
    pub oneline: bool,
    /// Show graph
    #[serde(default)]
    pub graph: bool,
    /// Search for commits with message containing this string
    #[serde(default)]
    pub grep: Option<String>,
    /// Show commits by author
    #[serde(default)]
    pub author: Option<String>,
}

/// Commit entry in log output
#[derive(Debug, Clone, Serialize)]
pub struct GitLogCommit {
    /// Commit hash
    pub hash: String,
    /// Short hash
    pub short_hash: String,
    /// Author name
    pub author: String,
    /// Author email
    pub email: String,
    /// Commit date
    pub date: String,
    /// Commit message (first line)
    pub message: String,
}

/// GitLog tool output
#[derive(Debug, Clone, Serialize)]
pub struct GitLogOutput {
    /// Raw log output
    pub log: String,
    /// Parsed commits (when not using custom format)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<GitLogCommit>,
    /// Total commits returned
    pub count: u32,
}

/// Git log tool - shows commit history
pub struct GitLogTool {
    /// Default number of commits to show
    pub default_count: u32,
}

impl Default for GitLogTool {
    fn default() -> Self {
        Self { default_count: 10 }
    }
}

#[async_trait]
impl AgentTool for GitLogTool {
    type Input = GitLogInput;
    type Output = GitLogOutput;

    fn name(&self) -> &str {
        "GitLog"
    }

    fn description(&self) -> &str {
        "Show commit history. Can filter by file, branch, author, or message content."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "count": {
                    "type": "integer",
                    "description": "Number of commits to show (default: 10)"
                },
                "file_path": {
                    "type": "string",
                    "description": "Show history for a specific file"
                },
                "branch": {
                    "type": "string",
                    "description": "Show history for a specific branch or ref"
                },
                "oneline": {
                    "type": "boolean",
                    "description": "Use single-line format"
                },
                "graph": {
                    "type": "boolean",
                    "description": "Show ASCII graph of branch structure"
                },
                "grep": {
                    "type": "string",
                    "description": "Search for commits with message containing this string"
                },
                "author": {
                    "type": "string",
                    "description": "Show commits by a specific author"
                }
            }
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "git"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        let cwd = &context.cwd;

        // Check if it's a git repository
        if !is_git_repository(cwd).await {
            return Err(ToolError::InvalidInput(format!(
                "Not a git repository: {}",
                cwd.display()
            )));
        }

        let count = input.count.unwrap_or(self.default_count).min(100);

        // Build args
        let mut args = vec!["log"];

        let count_arg = format!("-{}", count);
        args.push(&count_arg);

        // Use a structured format for parsing
        let format_arg: String;
        if input.oneline {
            args.push("--oneline");
        } else {
            // Use a custom format for parsing
            format_arg = "--format=%H|%h|%an|%ae|%ai|%s".to_string();
            args.push(&format_arg);
        }

        if input.graph {
            args.push("--graph");
        }

        let grep_arg: String;
        if let Some(ref grep) = input.grep {
            grep_arg = format!("--grep={}", grep);
            args.push(&grep_arg);
        }

        let author_arg: String;
        if let Some(ref author) = input.author {
            author_arg = format!("--author={}", author);
            args.push(&author_arg);
        }

        if let Some(ref branch) = input.branch {
            args.push(branch);
        }

        if let Some(ref file_path) = input.file_path {
            args.push("--");
            args.push(file_path);
        }

        let (log, exit_code) = execute_git_command(&args, cwd, None).await?;

        if exit_code != 0 {
            return Err(ToolError::ExecutionError(format!(
                "git log failed: {}",
                log
            )));
        }

        // Parse commits if using structured format
        let commits = if !input.oneline && !input.graph {
            parse_log_output(&log)
        } else {
            Vec::new()
        };

        let count = commits.len() as u32;

        Ok(GitLogOutput {
            log,
            commits,
            count,
        })
    }
}

/// Parse structured log output
fn parse_log_output(output: &str) -> Vec<GitLogCommit> {
    output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(6, '|').collect();
            if parts.len() >= 6 {
                Some(GitLogCommit {
                    hash: parts[0].to_string(),
                    short_hash: parts[1].to_string(),
                    author: parts[2].to_string(),
                    email: parts[3].to_string(),
                    date: parts[4].to_string(),
                    message: parts[5].to_string(),
                })
            } else {
                None
            }
        })
        .collect()
}

// ============================================================================
// GitBranch Tool
// ============================================================================

/// GitBranch tool input
#[derive(Debug, Clone, Deserialize)]
pub struct GitBranchInput {
    /// List all branches including remotes
    #[serde(default)]
    pub all: bool,
    /// List remote branches only
    #[serde(default)]
    pub remote: bool,
    /// Create a new branch with this name
    #[serde(default)]
    pub create: Option<String>,
    /// Delete a branch with this name
    #[serde(default)]
    pub delete: Option<String>,
    /// Switch to this branch
    #[serde(default)]
    pub checkout: Option<String>,
}

/// Branch entry
#[derive(Debug, Clone, Serialize)]
pub struct GitBranchEntry {
    /// Branch name
    pub name: String,
    /// Whether this is the current branch
    pub current: bool,
    /// Whether this is a remote branch
    pub remote: bool,
}

/// GitBranch tool output
#[derive(Debug, Clone, Serialize)]
pub struct GitBranchOutput {
    /// Raw output
    pub output: String,
    /// Current branch
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_branch: Option<String>,
    /// List of branches
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub branches: Vec<GitBranchEntry>,
    /// Action performed (list, create, delete, checkout)
    pub action: String,
}

/// Git branch tool - list, create, delete, or switch branches
pub struct GitBranchTool;

#[async_trait]
impl AgentTool for GitBranchTool {
    type Input = GitBranchInput;
    type Output = GitBranchOutput;

    fn name(&self) -> &str {
        "GitBranch"
    }

    fn description(&self) -> &str {
        "List, create, delete, or switch git branches. Write operations (create, delete, checkout) require permission."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "all": {
                    "type": "boolean",
                    "description": "List all branches including remotes"
                },
                "remote": {
                    "type": "boolean",
                    "description": "List remote branches only"
                },
                "create": {
                    "type": "string",
                    "description": "Create a new branch with this name"
                },
                "delete": {
                    "type": "string",
                    "description": "Delete a branch with this name"
                },
                "checkout": {
                    "type": "string",
                    "description": "Switch to this branch"
                }
            }
        })
    }

    fn requires_permission(&self) -> bool {
        // Branch operations (create, delete, checkout) are mutating — require permission
        true
    }

    fn is_mutating(&self) -> bool {
        // Branch operations (create, delete, checkout) modify repository state
        true
    }

    fn namespace(&self) -> &str {
        "git"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        let cwd = &context.cwd;

        // Check if it's a git repository
        if !is_git_repository(cwd).await {
            return Err(ToolError::InvalidInput(format!(
                "Not a git repository: {}",
                cwd.display()
            )));
        }

        // Handle write operations with permission check
        if input.create.is_some() || input.delete.is_some() || input.checkout.is_some() {
            if !context.allows_mutations() {
                return Err(ToolError::PermissionDenied(
                    "Branch modifications not allowed in current permission mode".to_string(),
                ));
            }
        }

        // Create branch
        if let Some(ref branch_name) = input.create {
            let (output, exit_code) =
                execute_git_command(&["branch", branch_name], cwd, None).await?;
            if exit_code != 0 {
                return Err(ToolError::ExecutionError(format!(
                    "Failed to create branch: {}",
                    output
                )));
            }
            return Ok(GitBranchOutput {
                output: format!("Created branch '{}'", branch_name),
                current_branch: None,
                branches: Vec::new(),
                action: "create".to_string(),
            });
        }

        // Delete branch
        if let Some(ref branch_name) = input.delete {
            let (output, exit_code) =
                execute_git_command(&["branch", "-d", branch_name], cwd, None).await?;
            if exit_code != 0 {
                return Err(ToolError::ExecutionError(format!(
                    "Failed to delete branch: {}",
                    output
                )));
            }
            return Ok(GitBranchOutput {
                output: format!("Deleted branch '{}'", branch_name),
                current_branch: None,
                branches: Vec::new(),
                action: "delete".to_string(),
            });
        }

        // Checkout branch
        if let Some(ref branch_name) = input.checkout {
            let (output, exit_code) =
                execute_git_command(&["checkout", branch_name], cwd, None).await?;
            if exit_code != 0 {
                return Err(ToolError::ExecutionError(format!(
                    "Failed to checkout branch: {}",
                    output
                )));
            }
            return Ok(GitBranchOutput {
                output: format!("Switched to branch '{}'", branch_name),
                current_branch: Some(branch_name.clone()),
                branches: Vec::new(),
                action: "checkout".to_string(),
            });
        }

        // List branches
        let mut args = vec!["branch"];
        if input.all {
            args.push("-a");
        } else if input.remote {
            args.push("-r");
        }

        let (output, exit_code) = execute_git_command(&args, cwd, None).await?;

        if exit_code != 0 {
            return Err(ToolError::ExecutionError(format!(
                "git branch failed: {}",
                output
            )));
        }

        // Parse branches
        let (branches, current) = parse_branch_output(&output, input.remote);

        Ok(GitBranchOutput {
            output,
            current_branch: current,
            branches,
            action: "list".to_string(),
        })
    }
}

/// Parse git branch output
fn parse_branch_output(output: &str, remote_only: bool) -> (Vec<GitBranchEntry>, Option<String>) {
    let mut branches = Vec::new();
    let mut current = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let is_current = trimmed.starts_with('*');
        let name = trimmed.trim_start_matches('*').trim();

        // Skip HEAD pointer lines
        if name.contains("->") {
            continue;
        }

        let is_remote = name.starts_with("remotes/") || name.contains('/');

        if is_current {
            current = Some(name.to_string());
        }

        branches.push(GitBranchEntry {
            name: name.to_string(),
            current: is_current,
            remote: is_remote || remote_only,
        });
    }

    (branches, current)
}

// ============================================================================
// GitCommit Tool
// ============================================================================

/// GitCommit tool input
#[derive(Debug, Clone, Deserialize)]
pub struct GitCommitInput {
    /// Commit message
    pub message: String,
    /// Stage all modified files before committing
    #[serde(default)]
    pub all: bool,
    /// Files to stage before committing
    #[serde(default)]
    pub files: Vec<String>,
    /// Amend the previous commit
    #[serde(default)]
    pub amend: bool,
}

/// GitCommit tool output
#[derive(Debug, Clone, Serialize)]
pub struct GitCommitOutput {
    /// Commit output message
    pub output: String,
    /// Commit hash
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_hash: Option<String>,
    /// Files committed
    pub files_committed: u32,
}

/// Git commit tool - create commits (requires permission)
pub struct GitCommitTool;

#[async_trait]
impl AgentTool for GitCommitTool {
    type Input = GitCommitInput;
    type Output = GitCommitOutput;

    fn name(&self) -> &str {
        "GitCommit"
    }

    fn description(&self) -> &str {
        "Create a git commit. Can optionally stage files before committing. Requires permission."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The commit message"
                },
                "all": {
                    "type": "boolean",
                    "description": "Stage all modified files before committing (-a flag)"
                },
                "files": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Specific files to stage before committing"
                },
                "amend": {
                    "type": "boolean",
                    "description": "Amend the previous commit"
                }
            },
            "required": ["message"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "git"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        let cwd = &context.cwd;

        // Check permission
        if !context.allows_mutations() {
            return Err(ToolError::PermissionDenied(
                "Git commit not allowed in current permission mode".to_string(),
            ));
        }

        // Check if it's a git repository
        if !is_git_repository(cwd).await {
            return Err(ToolError::InvalidInput(format!(
                "Not a git repository: {}",
                cwd.display()
            )));
        }

        // Stage specific files if provided
        if !input.files.is_empty() {
            let mut add_args = vec!["add"];
            for file in &input.files {
                add_args.push(file);
            }
            let (output, exit_code) = execute_git_command(&add_args, cwd, None).await?;
            if exit_code != 0 {
                return Err(ToolError::ExecutionError(format!(
                    "Failed to stage files: {}",
                    output
                )));
            }
        }

        // Build commit args
        let mut args = vec!["commit"];

        if input.all {
            args.push("-a");
        }

        if input.amend {
            args.push("--amend");
        }

        args.push("-m");
        args.push(&input.message);

        let (output, exit_code) = execute_git_command(&args, cwd, None).await?;

        if exit_code != 0 {
            return Err(ToolError::ExecutionError(format!(
                "git commit failed: {}",
                output
            )));
        }

        // Get the commit hash
        let (hash_output, _) =
            execute_git_command(&["rev-parse", "--short", "HEAD"], cwd, None).await?;
        let commit_hash = Some(hash_output.trim().to_string());

        // Parse number of files from output
        let files_committed = parse_commit_file_count(&output);

        Ok(GitCommitOutput {
            output,
            commit_hash,
            files_committed,
        })
    }
}

/// Parse the number of files from commit output
fn parse_commit_file_count(output: &str) -> u32 {
    // Look for patterns like "2 files changed" or "1 file changed"
    for line in output.lines() {
        if line.contains("file") && line.contains("changed") {
            if let Some(num) = line.split_whitespace().next() {
                return num.parse().unwrap_or(0);
            }
        }
    }
    0
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_status_counts() {
        // Short format status output (git status -s)
        // XY filename where X=index status, Y=worktree status
        let status = r#"M  file1.rs
A  file2.rs
 M file3.rs
?? new_file.rs
"#;
        let (staged, modified, untracked) = parse_status_counts(status);
        assert_eq!(staged, 2); // M (modified staged) and A (added)
        assert_eq!(modified, 1); // M in worktree column
        assert_eq!(untracked, 1); // ?? for untracked
    }

    #[test]
    fn test_parse_diff_stat() {
        let stat = " 3 files changed, 45 insertions(+), 12 deletions(-)";
        let (files, insertions, deletions) = parse_diff_stat(stat);
        assert_eq!(files, 3);
        assert_eq!(insertions, 45);
        assert_eq!(deletions, 12);
    }

    #[test]
    fn test_parse_log_output() {
        let log = "abc123|abc|John Doe|john@example.com|2024-01-15 10:30:00 -0500|Initial commit\n\
                   def456|def|Jane Doe|jane@example.com|2024-01-14 09:00:00 -0500|Add feature";
        let commits = parse_log_output(log);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].short_hash, "abc");
        assert_eq!(commits[0].author, "John Doe");
        assert_eq!(commits[1].message, "Add feature");
    }

    #[test]
    fn test_parse_branch_output() {
        let output = "  feature-branch\n* main\n  develop";
        let (branches, current) = parse_branch_output(output, false);
        assert_eq!(branches.len(), 3);
        assert_eq!(current, Some("main".to_string()));
        assert!(branches.iter().any(|b| b.name == "main" && b.current));
    }

    #[test]
    fn test_tool_metadata() {
        let status_tool = GitStatusTool;
        assert_eq!(status_tool.name(), "GitStatus");
        assert!(!status_tool.requires_permission());
        assert!(!status_tool.is_mutating());

        let commit_tool = GitCommitTool;
        assert_eq!(commit_tool.name(), "GitCommit");
        assert!(commit_tool.requires_permission());
        assert!(commit_tool.is_mutating());
    }

    #[test]
    fn test_parse_commit_file_count() {
        let output =
            "[main abc1234] Test commit\n 2 files changed, 10 insertions(+), 3 deletions(-)";
        assert_eq!(parse_commit_file_count(output), 2);

        let output2 = "[main def5678] Another commit\n 1 file changed, 5 insertions(+)";
        assert_eq!(parse_commit_file_count(output2), 1);
    }
}
