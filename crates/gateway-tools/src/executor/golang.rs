//! Go code executor

use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::Language;
use crate::error::{ServiceError as Error, ServiceResult as Result};

/// Simple execution result for Go
#[derive(Debug, Clone)]
pub struct GoResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub execution_time_ms: u64,
    pub language: Language,
}

/// Execution event for streaming
#[derive(Debug, Clone)]
pub enum GoEvent {
    Stdout(String),
    Stderr(String),
    Error(String),
}

/// Go executor for running Go code
pub struct GoExecutor {
    /// Path to go binary
    go_path: String,
    /// Working directory for execution
    work_dir: PathBuf,
    /// Maximum execution time in seconds
    timeout_secs: u64,
    /// Go module mode (on/off/auto)
    go_mod_mode: String,
}

impl GoExecutor {
    /// Create a new Go executor
    pub fn new(work_dir: &str) -> Self {
        Self {
            go_path: std::env::var("GO_PATH").unwrap_or_else(|_| "go".to_string()),
            work_dir: PathBuf::from(work_dir),
            timeout_secs: 60,
            go_mod_mode: "auto".to_string(),
        }
    }

    /// Set custom go path
    pub fn with_go_path(mut self, path: &str) -> Self {
        self.go_path = path.to_string();
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Set Go module mode
    pub fn with_mod_mode(mut self, mode: &str) -> Self {
        self.go_mod_mode = mode.to_string();
        self
    }

    /// Execute Go code using `go run`
    pub async fn execute(&self, code: &str) -> Result<GoResult> {
        // Create temp directory for Go module
        let temp_dir = tempfile::Builder::new()
            .prefix("go-exec-")
            .tempdir()
            .map_err(|e| Error::Internal(format!("Failed to create temp dir: {}", e)))?;

        let main_file = temp_dir.path().join("main.go");

        // Ensure the code has package main
        let code = if !code.contains("package main") {
            format!("package main\n\n{}", code)
        } else {
            code.to_string()
        };

        tokio::fs::write(&main_file, &code)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write code: {}", e)))?;

        // Initialize go module
        let init_output = Command::new(&self.go_path)
            .args(["mod", "init", "temp"])
            .current_dir(temp_dir.path())
            .output()
            .await
            .map_err(|e| Error::ExecutionFailed(format!("Failed to init go module: {}", e)))?;

        if !init_output.status.success() {
            tracing::debug!(
                stderr = %String::from_utf8_lossy(&init_output.stderr),
                "go mod init warning (may be expected)"
            );
        }

        // Run the code
        let mut cmd = Command::new(&self.go_path);
        cmd.args(["run", "main.go"])
            .current_dir(temp_dir.path())
            .env("GO111MODULE", &self.go_mod_mode)
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
                "Go execution timed out after {}s",
                self.timeout_secs
            ))
        })?
        .map_err(|e| Error::ExecutionFailed(format!("Failed to run go: {}", e)))?;

        let execution_time = start_time.elapsed();

        Ok(GoResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            execution_time_ms: execution_time.as_millis() as u64,
            language: Language::Go,
        })
    }

    /// Execute with streaming output
    pub async fn execute_streaming<F>(&self, code: &str, mut on_event: F) -> Result<GoResult>
    where
        F: FnMut(GoEvent) + Send,
    {
        let temp_dir = tempfile::Builder::new()
            .prefix("go-exec-")
            .tempdir()
            .map_err(|e| Error::Internal(format!("Failed to create temp dir: {}", e)))?;

        let main_file = temp_dir.path().join("main.go");

        let code = if !code.contains("package main") {
            format!("package main\n\n{}", code)
        } else {
            code.to_string()
        };

        tokio::fs::write(&main_file, &code)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write code: {}", e)))?;

        // Initialize go module
        Command::new(&self.go_path)
            .args(["mod", "init", "temp"])
            .current_dir(temp_dir.path())
            .output()
            .await
            .ok();

        let mut cmd = Command::new(&self.go_path);
        cmd.args(["run", "main.go"])
            .current_dir(temp_dir.path())
            .env("GO111MODULE", &self.go_mod_mode)
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
                            on_event(GoEvent::Stdout(line));
                        }
                        Ok(None) => break,
                        Err(e) => {
                            on_event(GoEvent::Error(e.to_string()));
                            break;
                        }
                    }
                }
                line = stderr_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            full_stderr.push_str(&line);
                            full_stderr.push('\n');
                            on_event(GoEvent::Stderr(line));
                        }
                        Ok(None) => {}
                        Err(e) => {
                            on_event(GoEvent::Error(e.to_string()));
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

        Ok(GoResult {
            stdout: full_stdout,
            stderr: full_stderr,
            exit_code: status.code().unwrap_or(-1),
            execution_time_ms: execution_time.as_millis() as u64,
            language: Language::Go,
        })
    }

    /// Build Go code to a binary
    pub async fn build(&self, code: &str, output_path: &str) -> Result<String> {
        let temp_dir = tempfile::Builder::new()
            .prefix("go-build-")
            .tempdir()
            .map_err(|e| Error::Internal(format!("Failed to create temp dir: {}", e)))?;

        let main_file = temp_dir.path().join("main.go");

        let code = if !code.contains("package main") {
            format!("package main\n\n{}", code)
        } else {
            code.to_string()
        };

        tokio::fs::write(&main_file, &code)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write code: {}", e)))?;

        // Initialize go module
        Command::new(&self.go_path)
            .args(["mod", "init", "temp"])
            .current_dir(temp_dir.path())
            .output()
            .await
            .ok();

        let output = Command::new(&self.go_path)
            .args(["build", "-o", output_path, "main.go"])
            .current_dir(temp_dir.path())
            .output()
            .await
            .map_err(|e| Error::ExecutionFailed(format!("Failed to build: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::ExecutionFailed(format!("Build failed: {}", stderr)));
        }

        Ok(output_path.to_string())
    }

    /// Get Go version
    pub async fn version(&self) -> Result<String> {
        let output = Command::new(&self.go_path)
            .arg("version")
            .output()
            .await
            .map_err(|e| Error::ExecutionFailed(format!("Failed to get go version: {}", e)))?;

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Install a Go module
    pub async fn go_get(&self, module: &str) -> Result<()> {
        let output = Command::new(&self.go_path)
            .args(["get", module])
            .current_dir(&self.work_dir)
            .output()
            .await
            .map_err(|e| Error::ExecutionFailed(format!("Failed to run go get: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::ExecutionFailed(format!("go get failed: {}", stderr)));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_go() {
        let executor = GoExecutor::new("/tmp");
        let code = r#"
package main

import "fmt"

func main() {
    fmt.Println("Hello, Go!")
}
"#;
        let result = executor.execute(code).await;

        // May fail if Go not installed
        if let Ok(result) = result {
            assert!(result.stdout.contains("Hello, Go!"));
            assert_eq!(result.exit_code, 0);
        }
    }

    #[tokio::test]
    async fn test_auto_add_package() {
        let executor = GoExecutor::new("/tmp");
        // Code without package declaration
        let code = r#"
import "fmt"

func main() {
    fmt.Println("Auto package!")
}
"#;
        let result = executor.execute(code).await;

        if let Ok(result) = result {
            assert!(result.stdout.contains("Auto package!"));
        }
    }
}
