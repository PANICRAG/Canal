//! Node.js code executor

use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::Language;
use crate::error::{ServiceError as Error, ServiceResult as Result};

/// Simple execution result for Node.js
#[derive(Debug, Clone)]
pub struct NodeJsResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub execution_time_ms: u64,
    pub language: Language,
}

/// Execution event for streaming
#[derive(Debug, Clone)]
pub enum NodeJsEvent {
    Stdout(String),
    Stderr(String),
    Error(String),
}

/// Node.js executor for JavaScript/TypeScript code
pub struct NodeJsExecutor {
    /// Path to node binary
    node_path: String,
    /// Path to npx binary for TypeScript
    npx_path: String,
    /// Working directory for execution
    work_dir: String,
    /// Maximum execution time in seconds
    timeout_secs: u64,
}

impl NodeJsExecutor {
    /// Create a new Node.js executor
    pub fn new(work_dir: &str) -> Self {
        Self {
            node_path: std::env::var("NODE_PATH").unwrap_or_else(|_| "node".to_string()),
            npx_path: std::env::var("NPX_PATH").unwrap_or_else(|_| "npx".to_string()),
            work_dir: work_dir.to_string(),
            timeout_secs: 30,
        }
    }

    /// Set custom node path
    pub fn with_node_path(mut self, path: &str) -> Self {
        self.node_path = path.to_string();
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Execute JavaScript code
    pub async fn execute_javascript(&self, code: &str, args: &[String]) -> Result<NodeJsResult> {
        let temp_file = tempfile::Builder::new()
            .suffix(".js")
            .tempfile()
            .map_err(|e| Error::Internal(format!("Failed to create temp file: {}", e)))?;

        tokio::fs::write(temp_file.path(), code)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write code: {}", e)))?;

        let mut cmd = Command::new(&self.node_path);
        cmd.arg(temp_file.path())
            .args(args)
            .current_dir(&self.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let start_time = std::time::Instant::now();

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            cmd.output(),
        )
        .await
        .map_err(|_| {
            Error::Timeout(format!(
                "Node.js execution timed out after {}s",
                self.timeout_secs
            ))
        })?
        .map_err(|e| Error::ExecutionFailed(format!("Failed to run node: {}", e)))?;

        let execution_time = start_time.elapsed();

        Ok(NodeJsResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            execution_time_ms: execution_time.as_millis() as u64,
            language: Language::JavaScript,
        })
    }

    /// Execute TypeScript code using ts-node
    pub async fn execute_typescript(&self, code: &str, args: &[String]) -> Result<NodeJsResult> {
        let temp_file = tempfile::Builder::new()
            .suffix(".ts")
            .tempfile()
            .map_err(|e| Error::Internal(format!("Failed to create temp file: {}", e)))?;

        tokio::fs::write(temp_file.path(), code)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write code: {}", e)))?;

        let mut cmd = Command::new(&self.npx_path);
        cmd.args(["ts-node", "--esm"])
            .arg(temp_file.path())
            .args(args)
            .current_dir(&self.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let start_time = std::time::Instant::now();

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            cmd.output(),
        )
        .await
        .map_err(|_| {
            Error::Timeout(format!(
                "TypeScript execution timed out after {}s",
                self.timeout_secs
            ))
        })?
        .map_err(|e| Error::ExecutionFailed(format!("Failed to run ts-node: {}", e)))?;

        let execution_time = start_time.elapsed();

        Ok(NodeJsResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            execution_time_ms: execution_time.as_millis() as u64,
            language: Language::TypeScript,
        })
    }

    /// Execute with streaming output
    pub async fn execute_streaming<F>(
        &self,
        code: &str,
        language: Language,
        mut on_event: F,
    ) -> Result<NodeJsResult>
    where
        F: FnMut(NodeJsEvent) + Send,
    {
        let (suffix, use_ts_node) = match language {
            Language::JavaScript => (".js", false),
            Language::TypeScript => (".ts", true),
            _ => return Err(Error::UnsupportedLanguage(format!("{:?}", language))),
        };

        let temp_file = tempfile::Builder::new()
            .suffix(suffix)
            .tempfile()
            .map_err(|e| Error::Internal(format!("Failed to create temp file: {}", e)))?;

        tokio::fs::write(temp_file.path(), code)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write code: {}", e)))?;

        let mut cmd = if use_ts_node {
            let mut c = Command::new(&self.npx_path);
            c.args(["ts-node", "--esm"]);
            c.arg(temp_file.path());
            c
        } else {
            let mut c = Command::new(&self.node_path);
            c.arg(temp_file.path());
            c
        };

        cmd.current_dir(&self.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let start_time = std::time::Instant::now();
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::ExecutionFailed(format!("Failed to spawn process: {}", e)))?;

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut full_stdout = String::new();
        let mut full_stderr = String::new();

        loop {
            tokio::select! {
                line = stdout_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            full_stdout.push_str(&line);
                            full_stdout.push('\n');
                            on_event(NodeJsEvent::Stdout(line));
                        }
                        Ok(None) => break,
                        Err(e) => {
                            on_event(NodeJsEvent::Error(e.to_string()));
                            break;
                        }
                    }
                }
                line = stderr_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            full_stderr.push_str(&line);
                            full_stderr.push('\n');
                            on_event(NodeJsEvent::Stderr(line));
                        }
                        Ok(None) => {}
                        Err(e) => {
                            on_event(NodeJsEvent::Error(e.to_string()));
                        }
                    }
                }
            }
        }

        let status = child
            .wait()
            .await
            .map_err(|e| Error::ExecutionFailed(format!("Failed to wait for process: {}", e)))?;

        let execution_time = start_time.elapsed();

        Ok(NodeJsResult {
            stdout: full_stdout,
            stderr: full_stderr,
            exit_code: status.code().unwrap_or(-1),
            execution_time_ms: execution_time.as_millis() as u64,
            language,
        })
    }

    /// Install npm packages
    pub async fn npm_install(&self, packages: &[&str]) -> Result<()> {
        let mut cmd = Command::new("npm");
        cmd.arg("install")
            .args(packages)
            .current_dir(&self.work_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = cmd
            .output()
            .await
            .map_err(|e| Error::ExecutionFailed(format!("Failed to run npm install: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::ExecutionFailed(format!(
                "npm install failed: {}",
                stderr
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_javascript() {
        let executor = NodeJsExecutor::new("/tmp");
        let result = executor
            .execute_javascript("console.log('Hello, World!')", &[])
            .await;

        // May fail if Node.js not installed
        if let Ok(result) = result {
            assert!(result.stdout.contains("Hello, World!"));
            assert_eq!(result.exit_code, 0);
        }
    }
}
