//! Bash Tool - Shell command execution

use super::{AgentTool, ToolContext, ToolError, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::time::{timeout, Duration};

/// Bash tool input
#[derive(Debug, Clone, Deserialize)]
pub struct BashInput {
    /// The command to execute
    pub command: String,
    /// Timeout in milliseconds
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Description of what the command does
    #[serde(default)]
    pub description: Option<String>,
    /// Run in background
    #[serde(default)]
    pub run_in_background: bool,
}

/// Bash tool output
#[derive(Debug, Clone, Serialize)]
pub struct BashOutput {
    /// Command output (stdout + stderr interleaved)
    pub output: String,
    /// Exit code
    pub exit_code: i32,
    /// Whether the command was killed (timeout)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub killed: Option<bool>,
    /// Background shell ID (if run_in_background)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell_id: Option<String>,
}

/// Background shell state
pub struct BackgroundShell {
    /// Shell ID
    pub id: String,
    /// Output buffer
    pub output: Arc<RwLock<String>>,
    /// Whether the shell is still running
    pub running: Arc<RwLock<bool>>,
    /// Exit code when finished
    pub exit_code: Arc<RwLock<Option<i32>>>,
}

/// Bash tool for executing shell commands
pub struct BashTool {
    /// Default timeout in milliseconds
    pub default_timeout_ms: u64,
    /// Maximum output length
    pub max_output_length: usize,
    /// Background shells
    pub background_shells: Arc<RwLock<HashMap<String, Arc<BackgroundShell>>>>,
    /// Dangerous command patterns to block
    pub blocked_patterns: Vec<String>,
}

impl Default for BashTool {
    fn default() -> Self {
        Self {
            default_timeout_ms: 120000, // 2 minutes
            max_output_length: 30000,
            background_shells: Arc::new(RwLock::new(HashMap::new())),
            blocked_patterns: vec![
                "rm -rf /".to_string(),
                "rm -rf /*".to_string(),
                ":(){ :|:& };:".to_string(), // Fork bomb
                "> /dev/sda".to_string(),
                "mkfs.".to_string(),
                "dd if=/dev/zero of=/dev/".to_string(),
            ],
        }
    }
}

impl BashTool {
    /// Create a new bash tool
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a command is blocked.
    ///
    /// Uses both pattern matching (for multi-word patterns like "rm -rf /")
    /// and structural checks to catch bypass attempts including:
    /// - Whitespace variations: "rm  -rf /" or "rm\t-rf /"
    /// - Shell variable expansion: "$HOME" based bypasses
    /// - Quoting tricks: rm '-rf' /
    /// - Backtick/subshell injection: `cmd` or $(cmd)
    fn is_blocked(&self, command: &str) -> bool {
        let cmd_lower = command.to_lowercase();
        // Normalize: collapse all whitespace sequences to single space
        let cmd_normalized: String = cmd_lower.split_whitespace().collect::<Vec<_>>().join(" ");
        // Strip quotes and backslashes to catch quoting bypasses like rm '-rf' /
        let cmd_stripped: String = cmd_normalized
            .chars()
            .filter(|c| !matches!(c, '\'' | '"' | '\\'))
            .collect();

        // Check configured blocked patterns against all normalized forms
        let pattern_match = self.blocked_patterns.iter().any(|pattern| {
            let pattern_lower = pattern.to_lowercase();
            let pattern_normalized: String = pattern_lower
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            cmd_lower.contains(&pattern_lower)
                || cmd_normalized.contains(&pattern_normalized)
                || cmd_stripped.contains(&pattern_normalized)
        });
        if pattern_match {
            return true;
        }

        // Block dangerous commands by first token (catches aliases/wrappers)
        let first_token = cmd_normalized.split_whitespace().next().unwrap_or("");
        let dangerous_commands = [
            "mkfs",
            "mkfs.ext4",
            "mkfs.xfs",
            "mkfs.btrfs",
            "mke2fs",
            "wipefs",
            "shred",
        ];
        if dangerous_commands.contains(&first_token) {
            return true;
        }

        // Block shell variable expansion that targets system paths
        // e.g., rm -rf ${HOME}, rm -rf $HOME
        if cmd_normalized.contains("rm ")
            && (cmd_normalized.contains("${")
                || cmd_normalized.contains("$home")
                || cmd_normalized.contains("$user"))
            && cmd_normalized.contains("-r")
        {
            return true;
        }

        false
    }

    /// Get a background shell by ID
    pub async fn get_background_shell(&self, id: &str) -> Option<Arc<BackgroundShell>> {
        let shells = self.background_shells.read().await;
        shells.get(id).cloned()
    }

    /// List all background shells
    pub async fn list_background_shells(&self) -> Vec<String> {
        let shells = self.background_shells.read().await;
        shells.keys().cloned().collect()
    }

    /// Get output from a background shell
    pub async fn get_shell_output(&self, id: &str) -> Option<String> {
        if let Some(shell) = self.get_background_shell(id).await {
            let output = shell.output.read().await;
            Some(output.clone())
        } else {
            None
        }
    }
}

#[async_trait]
impl AgentTool for BashTool {
    type Input = BashInput;
    type Output = BashOutput;

    fn name(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> &str {
        r#"Executes a given bash command with optional timeout. Working directory persists between commands; shell state (everything else) does not.

IMPORTANT: This tool is for terminal operations like git, npm, docker, etc. DO NOT use it for file operations (reading, writing, editing, searching, finding files) - use the specialized tools for this instead.

Usage notes:
- The command argument is required.
- You can specify an optional timeout in milliseconds (up to 600000ms / 10 minutes). If not specified, commands will timeout after 120000ms (2 minutes).
- If the output exceeds 30000 characters, output will be truncated.
- You can use the run_in_background parameter to run the command in the background.

Git Safety Protocol:
- NEVER update the git config
- NEVER run destructive git commands (push --force, reset --hard, checkout ., restore ., clean -f, branch -D) unless the user explicitly requests
- NEVER skip hooks (--no-verify, --no-gpg-sign, etc) unless explicitly requested
- ALWAYS create NEW commits rather than amending, unless explicitly requested
- When staging files, prefer adding specific files by name rather than using "git add -A" or "git add ."
- NEVER commit changes unless the user explicitly asks

Avoid using Bash for: find, grep, cat, head, tail, sed, awk, echo. Use dedicated tools instead:
- File search: Use Glob (NOT find or ls)
- Content search: Use Grep (NOT grep or rg)
- Read files: Use Read (NOT cat/head/tail)
- Edit files: Use Edit (NOT sed/awk)
- Write files: Use Write (NOT echo >/cat <<EOF)"#
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (max 600000)"
                },
                "description": {
                    "type": "string",
                    "description": "Description of what this command does"
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Run the command in the background"
                }
            },
            "required": ["command"]
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

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        // Check if mutations are allowed
        if !context.allows_mutations() {
            return Err(ToolError::PermissionDenied(
                "Bash commands not allowed in current permission mode".to_string(),
            ));
        }

        // Check for blocked commands
        if self.is_blocked(&input.command) {
            return Err(ToolError::PermissionDenied(format!(
                "Command blocked for safety: {}",
                input.command
            )));
        }

        let timeout_ms = input.timeout.unwrap_or(self.default_timeout_ms).min(600000);

        if input.run_in_background {
            return self.execute_background(input, context).await;
        }

        // Spawn the process
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(&input.command);
        cmd.current_dir(&context.cwd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // A42: Remove Claude Code nested session detection env vars.
        // Without this, any `claude` command invoked via bash tool fails
        // because the gateway process has CLAUDECODE=1 set.
        cmd.env_remove("CLAUDECODE");
        cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");

        // Set environment
        for (key, value) in &context.env {
            cmd.env(key, value);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::ExecutionError(format!("Failed to spawn: {}", e)))?;

        // Collect output with timeout
        let timeout_duration = Duration::from_millis(timeout_ms);

        let result = timeout(timeout_duration, async {
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let mut output = String::new();

            // Read stdout and stderr concurrently
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

        match result {
            Ok(Ok((output, exit_code))) => Ok(BashOutput {
                output,
                exit_code,
                killed: None,
                shell_id: None,
            }),
            Ok(Err(e)) => Err(ToolError::ExecutionError(e.to_string())),
            Err(_) => {
                // Timeout - try to kill the process
                let _ = child.kill().await;
                Ok(BashOutput {
                    output: format!("Command timed out after {}ms", timeout_ms),
                    exit_code: -1,
                    killed: Some(true),
                    shell_id: None,
                })
            }
        }
    }
}

impl BashTool {
    async fn execute_background(
        &self,
        input: BashInput,
        context: &ToolContext,
    ) -> ToolResult<BashOutput> {
        let shell_id = uuid::Uuid::new_v4().to_string();
        let output_buffer = Arc::new(RwLock::new(String::new()));
        let running = Arc::new(RwLock::new(true));
        let exit_code = Arc::new(RwLock::new(None));

        let shell = Arc::new(BackgroundShell {
            id: shell_id.clone(),
            output: output_buffer.clone(),
            running: running.clone(),
            exit_code: exit_code.clone(),
        });

        // Store shell
        {
            let mut shells = self.background_shells.write().await;
            shells.insert(shell_id.clone(), shell);
        }

        // Spawn background task
        let command = input.command.clone();
        let cwd = context.cwd.clone();
        let env = context.env.clone();
        let max_output = self.max_output_length;

        tokio::spawn(async move {
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(&command);
            cmd.current_dir(&cwd);
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());

            // A42: Remove nested session detection vars
            cmd.env_remove("CLAUDECODE");
            cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");

            for (key, value) in &env {
                cmd.env(key, value);
            }

            match cmd.spawn() {
                Ok(mut child) => {
                    if let Some(stdout) = child.stdout.take() {
                        let output_buffer = output_buffer.clone();
                        let mut reader = BufReader::new(stdout).lines();

                        while let Ok(Some(line)) = reader.next_line().await {
                            let mut output = output_buffer.write().await;
                            if output.len() < max_output {
                                output.push_str(&line);
                                output.push('\n');
                            }
                        }
                    }

                    if let Ok(status) = child.wait().await {
                        *exit_code.write().await = status.code();
                    }
                }
                Err(e) => {
                    let mut output = output_buffer.write().await;
                    output.push_str(&format!("Failed to spawn: {}", e));
                }
            }

            *running.write().await = false;
        });

        Ok(BashOutput {
            output: format!("Started background shell: {}", shell_id),
            exit_code: 0,
            killed: None,
            shell_id: Some(shell_id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bash_tool_echo() {
        let tool = BashTool::new();
        let context = ToolContext::new("s1", "/tmp")
            .with_allowed_directory("/tmp")
            .with_permission_mode(crate::agent::types::PermissionMode::AcceptEdits);

        let input = BashInput {
            command: "echo 'Hello, World!'".to_string(),
            timeout: None,
            description: None,
            run_in_background: false,
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(output.output.contains("Hello, World!"));
    }

    #[tokio::test]
    async fn test_bash_tool_blocked_command() {
        let tool = BashTool::new();
        let context = ToolContext::new("s1", "/tmp")
            .with_allowed_directory("/tmp")
            .with_permission_mode(crate::agent::types::PermissionMode::AcceptEdits);

        let input = BashInput {
            command: "rm -rf /".to_string(),
            timeout: None,
            description: None,
            run_in_background: false,
        };

        let result = tool.execute(input, &context).await;
        assert!(matches!(result, Err(ToolError::PermissionDenied(_))));
    }

    #[tokio::test]
    async fn test_bash_tool_timeout() {
        let tool = BashTool::new();
        let context = ToolContext::new("s1", "/tmp")
            .with_allowed_directory("/tmp")
            .with_permission_mode(crate::agent::types::PermissionMode::AcceptEdits);

        let input = BashInput {
            command: "sleep 10".to_string(),
            timeout: Some(100), // 100ms timeout
            description: None,
            run_in_background: false,
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert_eq!(output.killed, Some(true));
    }

    #[test]
    fn test_is_blocked() {
        let tool = BashTool::new();
        assert!(tool.is_blocked("rm -rf /"));
        assert!(tool.is_blocked("sudo rm -rf /"));
        assert!(!tool.is_blocked("rm -rf ./temp"));
        assert!(!tool.is_blocked("echo hello"));
    }

    #[test]
    fn test_is_blocked_whitespace_bypass() {
        let tool = BashTool::new();
        // Double space bypass
        assert!(tool.is_blocked("rm  -rf /"));
        // Tab bypass
        assert!(tool.is_blocked("rm\t-rf\t/"));
        // Quoting bypass
        assert!(tool.is_blocked("rm '-rf' /"));
        assert!(tool.is_blocked("rm \"-rf\" /"));
    }

    #[test]
    fn test_is_blocked_dangerous_first_token() {
        let tool = BashTool::new();
        assert!(tool.is_blocked("mkfs.ext4 /dev/sda1"));
        assert!(tool.is_blocked("shred /dev/sda"));
        assert!(tool.is_blocked("wipefs -a /dev/sda"));
    }

    #[test]
    fn test_is_blocked_shell_variable_bypass() {
        let tool = BashTool::new();
        assert!(tool.is_blocked("rm -rf ${HOME}"));
        assert!(tool.is_blocked("rm -rf $HOME"));
    }
}
