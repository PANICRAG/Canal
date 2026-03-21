//! ClaudeCode Tool — Invoke local `claude` CLI for autonomous coding & git push.
//!
//! Runs `claude -p "<prompt>" --dangerously-skip-permissions` in a specified
//! working directory, optionally auto-commits and pushes changes afterwards.

use super::{AgentTool, ToolContext, ToolError, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

/// Input for the ClaudeCode tool.
#[derive(Debug, Clone, Deserialize)]
pub struct ClaudeCodeInput {
    /// The prompt / task description to send to Claude Code.
    pub prompt: String,
    /// Working directory where Claude Code will operate.
    pub working_dir: String,
    /// Whether to auto-commit and push after Claude Code finishes.
    #[serde(default)]
    pub auto_push: bool,
    /// Commit message (used when auto_push is true). Defaults to auto-generated.
    #[serde(default)]
    pub commit_message: Option<String>,
    /// Git remote name (default: "origin").
    #[serde(default)]
    pub remote: Option<String>,
    /// Git branch name. If omitted, pushes current branch.
    #[serde(default)]
    pub branch: Option<String>,
    /// Model to use (e.g., "sonnet", "opus"). If omitted, claude uses its default.
    #[serde(default)]
    pub model: Option<String>,
    /// Max turns for Claude Code (default: none, let claude decide).
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Timeout in milliseconds (default: 600000 = 10 min).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Allowed tools for Claude Code (comma-separated). If omitted, all tools allowed.
    #[serde(default)]
    pub allowed_tools: Option<String>,
    /// Disallowed tools for Claude Code (comma-separated).
    #[serde(default)]
    pub disallowed_tools: Option<String>,
    /// Run in an isolated git worktree (creates a temporary branch).
    /// Default: false — only enable for git repos when branch isolation is needed.
    #[serde(default = "default_use_worktree")]
    pub use_worktree: bool,
    /// Whether to pass --dangerously-skip-permissions to Claude Code.
    /// Default: true for backward compatibility, but can be disabled for safer execution.
    #[serde(default = "default_skip_permissions")]
    pub skip_permissions: bool,
}

fn default_skip_permissions() -> bool {
    true
}

fn default_use_worktree() -> bool {
    false
}

impl Default for ClaudeCodeInput {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            working_dir: String::new(),
            auto_push: false,
            commit_message: None,
            remote: None,
            branch: None,
            model: None,
            max_turns: None,
            timeout_ms: None,
            allowed_tools: None,
            disallowed_tools: None,
            use_worktree: false,
            skip_permissions: true,
        }
    }
}

/// Output from the ClaudeCode tool.
#[derive(Debug, Clone, Serialize)]
pub struct ClaudeCodeOutput {
    /// Claude Code's stdout output.
    pub output: String,
    /// Exit code from the claude process.
    pub exit_code: i32,
    /// Whether the process was killed (timeout).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub killed: Option<bool>,
    /// Git operations result (if auto_push was true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_result: Option<String>,
    /// Resolved path of the claude binary that was used.
    pub claude_binary: String,
}

/// ClaudeCode tool for invoking the local `claude` CLI.
pub struct ClaudeCodeTool {
    /// Default timeout in milliseconds (10 minutes).
    pub default_timeout_ms: u64,
    /// Maximum output length before truncation.
    pub max_output_length: usize,
}

impl Default for ClaudeCodeTool {
    fn default() -> Self {
        Self {
            default_timeout_ms: 600_000, // 10 minutes
            max_output_length: 100_000,  // 100KB (claude can be verbose)
        }
    }
}

impl ClaudeCodeTool {
    /// Create a new ClaudeCode tool.
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve the claude binary path.
    ///
    /// Checks in order:
    /// 1. `CLAUDE_CODE_PATH` env var
    /// 2. `which claude` (PATH lookup)
    /// 3. Common install locations
    fn resolve_claude_binary() -> Result<String, ToolError> {
        // 1. Env var override
        if let Ok(path) = std::env::var("CLAUDE_CODE_PATH") {
            if std::path::Path::new(&path).exists() {
                return Ok(path);
            }
        }

        // 2. Try `which claude` via quick sync check
        if let Ok(output) = std::process::Command::new("which").arg("claude").output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() && std::path::Path::new(&path).exists() {
                    return Ok(path);
                }
            }
        }

        // 3. Common install locations
        let home = std::env::var("HOME").unwrap_or_default();
        let candidates = [
            format!("{}/.npm/bin/claude", home),
            format!("{}/.local/bin/claude", home),
            format!("{}/.nvm/versions/node/*/bin/claude", home), // glob-ish
            "/usr/local/bin/claude".to_string(),
            "/opt/homebrew/bin/claude".to_string(),
        ];

        for candidate in &candidates {
            if std::path::Path::new(candidate).exists() {
                return Ok(candidate.clone());
            }
        }

        Err(ToolError::NotFound(
            "Claude Code CLI not found. Install it or set CLAUDE_CODE_PATH env var.".to_string(),
        ))
    }

    /// Run git add + commit + push in the working directory.
    async fn git_auto_push(
        cwd: &str,
        commit_message: Option<&str>,
        remote: &str,
        branch: Option<&str>,
    ) -> Result<String, String> {
        let mut results = Vec::new();

        // 1. git add -A
        let add = Command::new("git")
            .args(["add", "-A"])
            .current_dir(cwd)
            .output()
            .await
            .map_err(|e| format!("git add failed: {}", e))?;

        if !add.status.success() {
            return Err(format!(
                "git add failed: {}",
                String::from_utf8_lossy(&add.stderr)
            ));
        }
        results.push("git add -A: ok".to_string());

        // 2. Check if there are changes to commit
        let diff = Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(cwd)
            .status()
            .await
            .map_err(|e| format!("git diff check failed: {}", e))?;

        if diff.success() {
            results.push("No changes to commit".to_string());
            return Ok(results.join("\n"));
        }

        // 3. git commit
        let msg = commit_message.unwrap_or("chore: auto-commit by Canal agent via Claude Code");
        let commit = Command::new("git")
            .args(["commit", "-m", msg])
            .current_dir(cwd)
            .output()
            .await
            .map_err(|e| format!("git commit failed: {}", e))?;

        if !commit.status.success() {
            return Err(format!(
                "git commit failed: {}",
                String::from_utf8_lossy(&commit.stderr)
            ));
        }
        results.push(format!(
            "git commit: {}",
            String::from_utf8_lossy(&commit.stdout).trim()
        ));

        // 4. git push
        let mut push_args = vec!["push", remote];
        if let Some(b) = branch {
            push_args.push(b);
        }

        let push = Command::new("git")
            .args(&push_args)
            .current_dir(cwd)
            .output()
            .await
            .map_err(|e| format!("git push failed: {}", e))?;

        if !push.status.success() {
            return Err(format!(
                "git push failed: {}",
                String::from_utf8_lossy(&push.stderr)
            ));
        }
        results.push(format!(
            "git push: {}",
            String::from_utf8_lossy(&push.stderr).trim() // git push outputs to stderr
        ));

        Ok(results.join("\n"))
    }
}

#[async_trait]
impl AgentTool for ClaudeCodeTool {
    type Input = ClaudeCodeInput;
    type Output = ClaudeCodeOutput;

    fn name(&self) -> &str {
        "ClaudeCode"
    }

    fn description(&self) -> &str {
        r#"Invoke the local Claude Code CLI to autonomously perform coding tasks in a specified directory.

Claude Code runs in non-interactive mode with --dangerously-skip-permissions, meaning it will automatically read, write, edit files, run commands, and make changes without asking for confirmation.

Use this tool when you need to delegate a complex coding task to an autonomous Claude Code instance that has full access to a repository.

Parameters:
- prompt (required): The task description for Claude Code.
- working_dir (required): The directory where Claude Code will operate. Must be an existing directory.
- auto_push (optional, default false): If true, automatically runs `git add -A && git commit && git push` after Claude Code finishes.
- commit_message (optional): Custom commit message for auto_push. Defaults to auto-generated message.
- remote (optional, default "origin"): Git remote name for push.
- branch (optional): Git branch to push. If omitted, pushes current branch.
- model (optional): Model to use (e.g., "sonnet", "opus").
- max_turns (optional): Maximum conversation turns for Claude Code.
- timeout_ms (optional, default 600000): Timeout in milliseconds (max 10 minutes).
- allowed_tools (optional): Comma-separated list of tools Claude Code can use.
- disallowed_tools (optional): Comma-separated list of tools Claude Code cannot use.
- use_worktree (optional, default false): Run in an isolated git worktree branch (requires git repo).

The tool returns Claude Code's JSON output including result, cost, and session metadata."#
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The coding task for Claude Code to perform"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Directory where Claude Code will operate"
                },
                "auto_push": {
                    "type": "boolean",
                    "description": "Auto git add + commit + push after completion",
                    "default": false
                },
                "commit_message": {
                    "type": "string",
                    "description": "Custom commit message (auto_push only)"
                },
                "remote": {
                    "type": "string",
                    "description": "Git remote name (default: origin)"
                },
                "branch": {
                    "type": "string",
                    "description": "Git branch to push (default: current branch)"
                },
                "model": {
                    "type": "string",
                    "description": "Model to use (e.g., sonnet, opus)"
                },
                "max_turns": {
                    "type": "integer",
                    "description": "Max conversation turns"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 600000)"
                },
                "allowed_tools": {
                    "type": "string",
                    "description": "Comma-separated allowed tools for Claude Code"
                },
                "disallowed_tools": {
                    "type": "string",
                    "description": "Comma-separated disallowed tools for Claude Code"
                },
                "use_worktree": {
                    "type": "boolean",
                    "description": "Run in an isolated git worktree branch (default: false, requires git repo)",
                    "default": false
                }
            },
            "required": ["prompt", "working_dir"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "executor"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        // Validate working directory exists
        let working_dir = std::path::Path::new(&input.working_dir);
        if !working_dir.exists() || !working_dir.is_dir() {
            return Err(ToolError::InvalidInput(format!(
                "Working directory does not exist: {}",
                input.working_dir
            )));
        }

        // Resolve claude binary
        let claude_binary = Self::resolve_claude_binary()?;

        // Build command args
        let mut args: Vec<String> = vec![
            "-p".to_string(),
            input.prompt.clone(),
            "--output-format".to_string(),
            "json".to_string(),
        ];
        if input.skip_permissions {
            args.push("--dangerously-skip-permissions".to_string());
        }

        if input.use_worktree {
            args.push("--worktree".to_string());
        }

        if let Some(model) = &input.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        if let Some(max_turns) = input.max_turns {
            args.push("--max-turns".to_string());
            args.push(max_turns.to_string());
        }

        if let Some(allowed) = &input.allowed_tools {
            args.push("--allowedTools".to_string());
            args.push(allowed.clone());
        }

        if let Some(disallowed) = &input.disallowed_tools {
            args.push("--disallowedTools".to_string());
            args.push(disallowed.clone());
        }

        tracing::info!(
            claude_binary = %claude_binary,
            working_dir = %input.working_dir,
            prompt_len = input.prompt.len(),
            auto_push = input.auto_push,
            "Invoking Claude Code CLI"
        );

        // Remove Claude Code nested-session detection env vars.
        // Claude Code checks CLAUDECODE and CLAUDE_CODE_ENTRYPOINT to detect
        // if it's running inside another session. We unset only these vars,
        // inheriting everything else — same approach Claude Code's own Bash tool uses.
        let mut cmd = Command::new(&claude_binary);
        cmd.args(&args);
        cmd.current_dir(&input.working_dir);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.env_remove("CLAUDECODE");
        cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");

        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::ExecutionError(format!("Failed to spawn claude: {}", e)))?;

        let timeout_ms = input
            .timeout_ms
            .unwrap_or(self.default_timeout_ms)
            .min(600_000);
        let timeout_duration = Duration::from_millis(timeout_ms);

        let result = timeout(timeout_duration, async {
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let mut output = String::new();

            if let (Some(stdout), Some(stderr)) = (stdout, stderr) {
                let mut stdout_reader = BufReader::new(stdout).lines();
                let mut stderr_reader = BufReader::new(stderr).lines();

                loop {
                    tokio::select! {
                        line = stdout_reader.next_line() => {
                            match line {
                                Ok(Some(l)) => {
                                    output.push_str(&l);
                                    output.push('\n');
                                }
                                Ok(None) => break,
                                Err(_) => break,
                            }
                        }
                        line = stderr_reader.next_line() => {
                            match line {
                                Ok(Some(l)) => {
                                    output.push_str("[stderr] ");
                                    output.push_str(&l);
                                    output.push('\n');
                                }
                                Ok(None) => {}
                                Err(_) => {}
                            }
                        }
                    }

                    if output.len() > self.max_output_length {
                        output.truncate(self.max_output_length);
                        output.push_str("\n... (output truncated)");
                        break;
                    }
                }
            }

            let status = child.wait().await?;
            Ok::<_, std::io::Error>((output, status.code().unwrap_or(-1)))
        })
        .await;

        let (output, exit_code, killed) = match result {
            Ok(Ok((output, exit_code))) => (output, exit_code, None),
            Ok(Err(e)) => return Err(ToolError::ExecutionError(e.to_string())),
            Err(_) => {
                let _ = child.kill().await;
                (
                    format!("Claude Code timed out after {}ms", timeout_ms),
                    -1,
                    Some(true),
                )
            }
        };

        tracing::info!(
            exit_code = exit_code,
            output_len = output.len(),
            killed = ?killed,
            "Claude Code finished"
        );

        // Auto-push if requested and claude succeeded
        let git_result = if input.auto_push && exit_code == 0 {
            let remote = input.remote.as_deref().unwrap_or("origin");
            match Self::git_auto_push(
                &input.working_dir,
                input.commit_message.as_deref(),
                remote,
                input.branch.as_deref(),
            )
            .await
            {
                Ok(result) => {
                    tracing::info!(result = %result, "Git auto-push completed");
                    Some(result)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Git auto-push failed");
                    Some(format!("Git push failed: {}", e))
                }
            }
        } else {
            None
        };

        Ok(ClaudeCodeOutput {
            output,
            exit_code,
            killed,
            git_result,
            claude_binary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_code_tool_name() {
        let tool = ClaudeCodeTool::new();
        assert_eq!(tool.name(), "ClaudeCode");
    }

    #[test]
    fn test_claude_code_tool_schema() {
        let tool = ClaudeCodeTool::new();
        let schema = tool.input_schema();
        let props = schema.get("properties").unwrap();
        assert!(props.get("prompt").is_some());
        assert!(props.get("working_dir").is_some());
        assert!(props.get("auto_push").is_some());
        assert!(props.get("model").is_some());
        assert!(props.get("max_turns").is_some());

        let required = schema.get("required").unwrap().as_array().unwrap();
        assert_eq!(required.len(), 2);
    }

    #[test]
    fn test_claude_code_tool_is_mutating() {
        let tool = ClaudeCodeTool::new();
        assert!(tool.is_mutating());
        assert!(tool.requires_permission());
    }

    #[tokio::test]
    async fn test_invalid_working_dir() {
        let tool = ClaudeCodeTool::new();
        let ctx = ToolContext::new("s1", "/tmp")
            .with_permission_mode(crate::agent::types::PermissionMode::AcceptEdits);

        let input = ClaudeCodeInput {
            prompt: "test".to_string(),
            working_dir: "/nonexistent/path/xyz".to_string(),
            auto_push: false,
            commit_message: None,
            remote: None,
            branch: None,
            model: None,
            max_turns: None,
            timeout_ms: None,
            allowed_tools: None,
            disallowed_tools: None,
            use_worktree: true,
            skip_permissions: true,
        };

        let result = tool.execute(input, &ctx).await;
        assert!(matches!(result, Err(ToolError::InvalidInput(_))));
    }

    #[test]
    fn test_input_deserialization() {
        let json = serde_json::json!({
            "prompt": "Fix the bug in main.rs",
            "working_dir": "/home/user/project",
            "auto_push": true,
            "model": "sonnet"
        });

        let input: ClaudeCodeInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.prompt, "Fix the bug in main.rs");
        assert_eq!(input.working_dir, "/home/user/project");
        assert!(input.auto_push);
        assert_eq!(input.model.as_deref(), Some("sonnet"));
        assert!(input.commit_message.is_none());
        assert!(input.branch.is_none());
    }

    #[test]
    fn test_input_minimal_deserialization() {
        let json = serde_json::json!({
            "prompt": "hello",
            "working_dir": "/tmp"
        });

        let input: ClaudeCodeInput = serde_json::from_value(json).unwrap();
        assert!(!input.auto_push);
        assert!(input.model.is_none());
        assert!(input.max_turns.is_none());
        assert!(input.timeout_ms.is_none());
        // use_worktree defaults to false
        assert!(!input.use_worktree);
    }

    #[test]
    fn test_use_worktree_default_false() {
        let json = serde_json::json!({
            "prompt": "fix bug",
            "working_dir": "/tmp/project"
        });
        let input: ClaudeCodeInput = serde_json::from_value(json).unwrap();
        assert!(!input.use_worktree);
    }

    #[test]
    fn test_use_worktree_explicit_false() {
        let json = serde_json::json!({
            "prompt": "fix bug",
            "working_dir": "/tmp/project",
            "use_worktree": false
        });
        let input: ClaudeCodeInput = serde_json::from_value(json).unwrap();
        assert!(!input.use_worktree);
    }

    #[test]
    fn test_claude_code_input_default() {
        let input = ClaudeCodeInput::default();
        assert!(input.prompt.is_empty());
        assert!(input.working_dir.is_empty());
        assert!(!input.auto_push);
        assert!(!input.use_worktree);
        assert!(input.model.is_none());
        assert!(input.max_turns.is_none());
        assert!(input.timeout_ms.is_none());
    }

    #[test]
    fn test_schema_includes_use_worktree() {
        let tool = ClaudeCodeTool::new();
        let schema = tool.input_schema();
        let props = schema.get("properties").unwrap();
        assert!(props.get("use_worktree").is_some());
    }
}
