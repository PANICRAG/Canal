//! Shell Hook Runner - Execute shell commands as hooks

use crate::agent::types::{HookContext, HookEvent, HookResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use super::HookCallback;

/// Shell hook runner executes shell commands as hooks
pub struct ShellHookRunner {
    /// The shell command to execute
    command: String,
    /// Working directory
    cwd: Option<String>,
    /// Environment variables
    env: Vec<(String, String)>,
    /// Timeout in milliseconds
    timeout_ms: u64,
    /// Events this hook handles
    events: Vec<HookEvent>,
    /// Hook name
    name: String,
}

/// Input passed to shell hooks via stdin
#[derive(Debug, Serialize)]
pub struct ShellHookInput {
    /// Event name
    pub hook_event_name: String,
    /// Session ID
    pub session_id: String,
    /// Current working directory
    pub cwd: String,
    /// Tool name (for tool hooks)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool input (for tool hooks)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<serde_json::Value>,
    /// Tool response (for post-tool hooks)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_response: Option<serde_json::Value>,
    /// Additional data
    #[serde(flatten)]
    pub data: serde_json::Value,
}

/// Output from shell hooks via stdout
#[derive(Debug, Deserialize)]
pub struct ShellHookOutput {
    /// Whether to continue
    #[serde(default = "default_true")]
    pub continue_execution: bool,
    /// Whether to cancel
    #[serde(default)]
    pub cancel: bool,
    /// Cancel reason
    #[serde(default)]
    pub cancel_reason: Option<String>,
    /// Modified data
    #[serde(default)]
    pub modified_data: Option<serde_json::Value>,
    /// Whether to suppress original output
    #[serde(default)]
    pub suppress_output: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ShellHookOutput {
    fn default() -> Self {
        Self {
            continue_execution: true,
            cancel: false,
            cancel_reason: None,
            modified_data: None,
            suppress_output: false,
        }
    }
}

impl ShellHookRunner {
    /// Create a new shell hook runner
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            cwd: None,
            env: Vec::new(),
            timeout_ms: 60000,
            events: Vec::new(),
        }
    }

    /// Set working directory
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Add environment variable
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Set timeout
    pub fn timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Set events to handle
    pub fn events(mut self, events: Vec<HookEvent>) -> Self {
        self.events = events;
        self
    }

    /// Execute the shell command
    async fn execute_command(&self, input: &ShellHookInput) -> Result<ShellHookOutput, String> {
        let input_json = serde_json::to_string(input).map_err(|e| e.to_string())?;

        // Spawn the process
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(&self.command);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Set working directory
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }

        // Set environment variables
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn: {}", e))?;

        // Write input to stdin
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input_json.as_bytes())
                .await
                .map_err(|e| format!("Failed to write stdin: {}", e))?;
        }

        // Wait for completion with timeout
        let timeout_duration = Duration::from_millis(self.timeout_ms);
        let output = timeout(timeout_duration, child.wait_with_output())
            .await
            .map_err(|_| format!("Hook timed out after {}ms", self.timeout_ms))?
            .map_err(|e| format!("Failed to wait: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "Hook exited with status {}: {}",
                output.status, stderr
            ));
        }

        // Parse output
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim().is_empty() {
            return Ok(ShellHookOutput::default());
        }

        serde_json::from_str(&stdout).map_err(|e| format!("Failed to parse output: {}", e))
    }
}

#[async_trait]
impl HookCallback for ShellHookRunner {
    async fn on_event(
        &self,
        event: HookEvent,
        data: serde_json::Value,
        context: &HookContext,
    ) -> HookResult {
        let input = ShellHookInput {
            hook_event_name: format!("{:?}", event),
            session_id: context.session_id.clone(),
            cwd: context.cwd.clone().unwrap_or_else(|| ".".to_string()),
            tool_name: data
                .get("tool_name")
                .and_then(|v| v.as_str())
                .map(String::from),
            tool_input: data.get("tool_input").cloned(),
            tool_response: data.get("tool_response").cloned(),
            data,
        };

        match self.execute_command(&input).await {
            Ok(output) => {
                if output.cancel {
                    HookResult::cancel(
                        output
                            .cancel_reason
                            .unwrap_or_else(|| "Cancelled by hook".to_string()),
                    )
                } else if let Some(modified) = output.modified_data {
                    HookResult::continue_with(modified)
                } else {
                    HookResult::continue_()
                }
            }
            Err(e) => {
                // Log error but continue execution
                tracing::warn!("Shell hook '{}' failed: {}", self.name, e);
                HookResult::continue_()
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn handles_event(&self, event: HookEvent) -> bool {
        // R1-M: Empty events list should NOT match all events (fail-closed)
        !self.events.is_empty() && self.events.contains(&event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_hook_input_serialization() {
        let input = ShellHookInput {
            hook_event_name: "PreToolUse".to_string(),
            session_id: "test-session".to_string(),
            cwd: "/tmp".to_string(),
            tool_name: Some("Bash".to_string()),
            tool_input: Some(serde_json::json!({"command": "ls"})),
            tool_response: None,
            data: serde_json::json!({}),
        };

        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("PreToolUse"));
        assert!(json.contains("Bash"));
    }

    #[test]
    fn test_shell_hook_output_deserialization() {
        let json = r#"{"continue_execution": true, "cancel": false}"#;
        let output: ShellHookOutput = serde_json::from_str(json).unwrap();
        assert!(output.continue_execution);
        assert!(!output.cancel);

        let json = r#"{"cancel": true, "cancel_reason": "Not allowed"}"#;
        let output: ShellHookOutput = serde_json::from_str(json).unwrap();
        assert!(output.cancel);
        assert_eq!(output.cancel_reason, Some("Not allowed".to_string()));
    }

    #[tokio::test]
    async fn test_shell_hook_runner_echo() {
        let runner =
            ShellHookRunner::new("test", "echo '{\"continue_execution\": true}'").timeout_ms(5000);

        let context = HookContext::default();
        let result = runner
            .on_event(HookEvent::PreToolUse, serde_json::json!({}), &context)
            .await;

        assert!(result.is_continue());
    }

    #[test]
    fn test_shell_hook_runner_builder() {
        let runner = ShellHookRunner::new("test", "echo test")
            .cwd("/tmp")
            .env("FOO", "bar")
            .timeout_ms(5000)
            .events(vec![HookEvent::PreToolUse, HookEvent::PostToolUse]);

        assert_eq!(runner.name, "test");
        assert_eq!(runner.cwd, Some("/tmp".to_string()));
        assert_eq!(runner.timeout_ms, 5000);
        assert_eq!(runner.events.len(), 2);
    }
}
