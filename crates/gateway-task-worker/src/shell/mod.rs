//! Shell command and code execution module

use std::process::Stdio;
use std::time::Instant;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::info;

/// Shell execution errors
#[derive(Error, Debug)]
pub enum ShellError {
    #[error("Command execution failed: {0}")]
    Execution(String),

    #[error("Command timed out after {0} seconds")]
    Timeout(i32),

    #[error("Unsupported language: {0}")]
    UnsupportedLanguage(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Output from code execution
#[derive(Debug, Clone)]
pub enum CodeOutputEvent {
    Stdout(String),
    Stderr(String),
    Complete {
        exit_code: i32,
        execution_time_ms: u64,
    },
}

/// Output from command execution
#[derive(Debug, Clone)]
pub enum CommandOutputEvent {
    Data(Vec<u8>),
    Complete {
        exit_code: i32,
        execution_time_ms: u64,
    },
}

/// Executes shell commands and code
#[derive(Clone)]
pub struct ShellExecutor {
    workspace_dir: String,
}

impl ShellExecutor {
    /// Create a new shell executor
    pub fn new(workspace_dir: &str) -> Self {
        Self {
            workspace_dir: workspace_dir.to_string(),
        }
    }

    /// Execute code in a specific language
    pub async fn execute_code(
        &self,
        code: &str,
        language: &str,
        timeout_seconds: i32,
    ) -> Result<Vec<CodeOutputEvent>, ShellError> {
        let start = Instant::now();
        let mut outputs = Vec::new();
        let timeout_dur = std::time::Duration::from_secs(timeout_seconds.max(1) as u64);

        // Get the command for the language
        let (command, args, temp_file) = self.get_language_command(language)?;

        info!(
            language = %language,
            command = %command,
            code_len = code.len(),
            timeout_seconds = timeout_seconds,
            "Executing code"
        );

        // Write code to temp file if needed
        if let Some(ref file_path) = temp_file {
            tokio::fs::write(file_path, code).await?;
        }

        // Build and execute the command
        let mut cmd = Command::new(&command);
        cmd.args(&args)
            .current_dir(&self.workspace_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;

        let result = tokio::time::timeout(timeout_dur, async {
            // R8-H12: Read stdout and stderr concurrently to prevent deadlock
            // when either buffer fills up while the other isn't being consumed
            let stdout_handle = child.stdout.take().map(|stdout| {
                tokio::spawn(async move {
                    let reader = BufReader::new(stdout);
                    let mut lines = reader.lines();
                    let mut out = Vec::new();
                    while let Ok(Some(line)) = lines.next_line().await {
                        out.push(CodeOutputEvent::Stdout(line + "\n"));
                    }
                    out
                })
            });
            let stderr_handle = child.stderr.take().map(|stderr| {
                tokio::spawn(async move {
                    let reader = BufReader::new(stderr);
                    let mut lines = reader.lines();
                    let mut out = Vec::new();
                    while let Ok(Some(line)) = lines.next_line().await {
                        out.push(CodeOutputEvent::Stderr(line + "\n"));
                    }
                    out
                })
            });

            let mut outs = Vec::new();
            if let Some(handle) = stdout_handle {
                if let Ok(lines) = handle.await {
                    outs.extend(lines);
                }
            }
            if let Some(handle) = stderr_handle {
                if let Ok(lines) = handle.await {
                    outs.extend(lines);
                }
            }
            let status = child.wait().await?;
            Ok::<_, ShellError>((outs, status))
        })
        .await;

        // Clean up temp file
        if let Some(ref file_path) = temp_file {
            let _ = tokio::fs::remove_file(file_path).await;
        }

        match result {
            Ok(Ok((mut outs, status))) => {
                let elapsed = start.elapsed();
                outputs.append(&mut outs);
                outputs.push(CodeOutputEvent::Complete {
                    exit_code: status.code().unwrap_or(-1),
                    execution_time_ms: elapsed.as_millis() as u64,
                });
                Ok(outputs)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                let _ = child.kill().await;
                Err(ShellError::Timeout(timeout_seconds))
            }
        }
    }

    /// Get command and args for a language
    fn get_language_command(
        &self,
        language: &str,
    ) -> Result<(String, Vec<String>, Option<String>), ShellError> {
        match language.to_lowercase().as_str() {
            "python" | "python3" | "py" => {
                let id = uuid::Uuid::new_v4().simple().to_string();
                let temp_file = format!("{}/temp_script_{}.py", self.workspace_dir, id);
                Ok((
                    "python3".to_string(),
                    vec![temp_file.clone()],
                    Some(temp_file),
                ))
            }
            "bash" | "sh" | "shell" => {
                let id = uuid::Uuid::new_v4().simple().to_string();
                let temp_file = format!("{}/temp_script_{}.sh", self.workspace_dir, id);
                Ok(("bash".to_string(), vec![temp_file.clone()], Some(temp_file)))
            }
            "javascript" | "js" | "node" => {
                let id = uuid::Uuid::new_v4().simple().to_string();
                let temp_file = format!("{}/temp_script_{}.js", self.workspace_dir, id);
                Ok(("node".to_string(), vec![temp_file.clone()], Some(temp_file)))
            }
            "typescript" | "ts" => {
                let id = uuid::Uuid::new_v4().simple().to_string();
                let temp_file = format!("{}/temp_script_{}.ts", self.workspace_dir, id);
                Ok((
                    "npx".to_string(),
                    vec!["ts-node".to_string(), temp_file.clone()],
                    Some(temp_file),
                ))
            }
            _ => Err(ShellError::UnsupportedLanguage(language.to_string())),
        }
    }

    /// Execute a shell command
    pub async fn execute_command(
        &self,
        command: &str,
        args: &[String],
        timeout_seconds: i32,
    ) -> Result<Vec<CommandOutputEvent>, ShellError> {
        let start = Instant::now();
        let timeout_dur = std::time::Duration::from_secs(timeout_seconds.max(1) as u64);

        info!(
            command = %command,
            args = ?args,
            timeout_seconds = timeout_seconds,
            "Executing command"
        );

        // Build the command
        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(&self.workspace_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;

        let result = tokio::time::timeout(timeout_dur, async {
            let mut outs = Vec::new();
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            // R8-M12: Read stdout and stderr concurrently to avoid deadlocks
            let (stdout_lines, stderr_lines) = tokio::join!(
                async {
                    let mut lines = Vec::new();
                    if let Some(out) = stdout {
                        let mut reader = BufReader::new(out).lines();
                        while let Ok(Some(line)) = reader.next_line().await {
                            lines.push(line);
                        }
                    }
                    lines
                },
                async {
                    let mut lines = Vec::new();
                    if let Some(err) = stderr {
                        let mut reader = BufReader::new(err).lines();
                        while let Ok(Some(line)) = reader.next_line().await {
                            lines.push(line);
                        }
                    }
                    lines
                }
            );

            for line in stdout_lines {
                outs.push(CommandOutputEvent::Data((line + "\n").into_bytes()));
            }
            for line in stderr_lines {
                outs.push(CommandOutputEvent::Data(line.into_bytes()));
            }

            let status = child.wait().await?;
            Ok::<_, ShellError>((outs, status))
        })
        .await;

        match result {
            Ok(Ok((mut outputs, status))) => {
                let elapsed = start.elapsed();
                outputs.push(CommandOutputEvent::Complete {
                    exit_code: status.code().unwrap_or(-1),
                    execution_time_ms: elapsed.as_millis() as u64,
                });
                Ok(outputs)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                let _ = child.kill().await;
                Err(ShellError::Timeout(timeout_seconds))
            }
        }
    }
}
