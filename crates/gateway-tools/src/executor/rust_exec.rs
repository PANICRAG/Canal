//! Rust code executor

use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::Language;
use crate::error::{ServiceError as Error, ServiceResult as Result};

/// Simple execution result for Rust
#[derive(Debug, Clone)]
pub struct RustResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub execution_time_ms: u64,
    pub language: Language,
}

/// Execution event for streaming
#[derive(Debug, Clone)]
pub enum RustEvent {
    Stdout(String),
    Stderr(String),
    Error(String),
    Status(String),
}

/// Rust executor for running Rust code
pub struct RustExecutor {
    /// Path to cargo binary
    cargo_path: String,
    /// Path to rustc binary
    rustc_path: String,
    /// Working directory for execution
    work_dir: PathBuf,
    /// Maximum execution time in seconds
    timeout_secs: u64,
    /// Rust edition (2018, 2021)
    edition: String,
}

impl RustExecutor {
    /// Create a new Rust executor
    pub fn new(work_dir: &str) -> Self {
        Self {
            cargo_path: std::env::var("CARGO_PATH").unwrap_or_else(|_| "cargo".to_string()),
            rustc_path: std::env::var("RUSTC_PATH").unwrap_or_else(|_| "rustc".to_string()),
            work_dir: PathBuf::from(work_dir),
            timeout_secs: 120, // Rust compilation can be slow
            edition: "2021".to_string(),
        }
    }

    /// Set custom cargo path
    pub fn with_cargo_path(mut self, path: &str) -> Self {
        self.cargo_path = path.to_string();
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Set Rust edition
    pub fn with_edition(mut self, edition: &str) -> Self {
        self.edition = edition.to_string();
        self
    }

    /// Execute Rust code using rustc directly (single file)
    pub async fn execute(&self, code: &str) -> Result<RustResult> {
        let temp_dir = tempfile::Builder::new()
            .prefix("rust-exec-")
            .tempdir()
            .map_err(|e| Error::Internal(format!("Failed to create temp dir: {}", e)))?;

        let main_file = temp_dir.path().join("main.rs");
        let binary_path = temp_dir.path().join("main");

        // Ensure the code has fn main
        let code = if !code.contains("fn main") {
            format!("fn main() {{\n{}\n}}", code)
        } else {
            code.to_string()
        };

        tokio::fs::write(&main_file, &code)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write code: {}", e)))?;

        // Compile the code
        let compile_output = Command::new(&self.rustc_path)
            .args(["--edition", &self.edition])
            .arg(&main_file)
            .arg("-o")
            .arg(&binary_path)
            .output()
            .await
            .map_err(|e| Error::ExecutionFailed(format!("Failed to compile: {}", e)))?;

        if !compile_output.status.success() {
            let stderr = String::from_utf8_lossy(&compile_output.stderr);
            return Ok(RustResult {
                stdout: String::new(),
                stderr: stderr.to_string(),
                exit_code: compile_output.status.code().unwrap_or(-1),
                execution_time_ms: 0,
                language: Language::Rust,
            });
        }

        // Run the binary
        let mut cmd = Command::new(&binary_path);
        cmd.current_dir(&self.work_dir)
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
                "Rust execution timed out after {}s",
                self.timeout_secs
            ))
        })?
        .map_err(|e| Error::ExecutionFailed(format!("Failed to run binary: {}", e)))?;

        let execution_time = start_time.elapsed();

        Ok(RustResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            execution_time_ms: execution_time.as_millis() as u64,
            language: Language::Rust,
        })
    }

    /// Execute Rust code as a Cargo project (supports dependencies)
    pub async fn execute_with_cargo(
        &self,
        code: &str,
        dependencies: &[(&str, &str)], // (name, version)
    ) -> Result<RustResult> {
        let temp_dir = tempfile::Builder::new()
            .prefix("rust-cargo-")
            .tempdir()
            .map_err(|e| Error::Internal(format!("Failed to create temp dir: {}", e)))?;

        // Create Cargo.toml
        let mut deps_str = String::new();
        for (name, version) in dependencies {
            deps_str.push_str(&format!("{} = \"{}\"\n", name, version));
        }

        let cargo_toml = format!(
            r#"[package]
name = "temp_exec"
version = "0.1.0"
edition = "{}"

[dependencies]
{}
"#,
            self.edition, deps_str
        );

        let cargo_file = temp_dir.path().join("Cargo.toml");
        tokio::fs::write(&cargo_file, cargo_toml)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write Cargo.toml: {}", e)))?;

        // Create src directory and main.rs
        let src_dir = temp_dir.path().join("src");
        tokio::fs::create_dir(&src_dir)
            .await
            .map_err(|e| Error::Internal(format!("Failed to create src dir: {}", e)))?;

        let main_file = src_dir.join("main.rs");

        let code = if !code.contains("fn main") {
            format!("fn main() {{\n{}\n}}", code)
        } else {
            code.to_string()
        };

        tokio::fs::write(&main_file, &code)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write main.rs: {}", e)))?;

        // Build and run with cargo
        let mut cmd = Command::new(&self.cargo_path);
        cmd.args(["run", "--release"])
            .current_dir(temp_dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let start_time = std::time::Instant::now();

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            cmd.output(),
        )
        .await
        .map_err(|_| Error::Timeout(format!("Cargo run timed out after {}s", self.timeout_secs)))?
        .map_err(|e| Error::ExecutionFailed(format!("Failed to run cargo: {}", e)))?;

        let execution_time = start_time.elapsed();

        Ok(RustResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            execution_time_ms: execution_time.as_millis() as u64,
            language: Language::Rust,
        })
    }

    /// Execute with streaming output
    pub async fn execute_streaming<F>(&self, code: &str, mut on_event: F) -> Result<RustResult>
    where
        F: FnMut(RustEvent) + Send,
    {
        let temp_dir = tempfile::Builder::new()
            .prefix("rust-exec-")
            .tempdir()
            .map_err(|e| Error::Internal(format!("Failed to create temp dir: {}", e)))?;

        let main_file = temp_dir.path().join("main.rs");
        let binary_path = temp_dir.path().join("main");

        let code = if !code.contains("fn main") {
            format!("fn main() {{\n{}\n}}", code)
        } else {
            code.to_string()
        };

        tokio::fs::write(&main_file, code.as_bytes())
            .await
            .map_err(|e| Error::Internal(format!("Failed to write code: {}", e)))?;

        on_event(RustEvent::Status("Compiling...".to_string()));

        // Compile
        let compile_output = Command::new(&self.rustc_path)
            .args(["--edition", &self.edition])
            .arg(&main_file)
            .arg("-o")
            .arg(&binary_path)
            .output()
            .await
            .map_err(|e| Error::ExecutionFailed(format!("Failed to compile: {}", e)))?;

        if !compile_output.status.success() {
            let stderr = String::from_utf8_lossy(&compile_output.stderr);
            on_event(RustEvent::Stderr(stderr.to_string()));
            return Ok(RustResult {
                stdout: String::new(),
                stderr: stderr.to_string(),
                exit_code: compile_output.status.code().unwrap_or(-1),
                execution_time_ms: 0,
                language: Language::Rust,
            });
        }

        on_event(RustEvent::Status("Running...".to_string()));

        let mut cmd = Command::new(&binary_path);
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
                            on_event(RustEvent::Stdout(line));
                        }
                        Ok(None) => break,
                        Err(e) => {
                            on_event(RustEvent::Error(e.to_string()));
                            break;
                        }
                    }
                }
                line = stderr_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            full_stderr.push_str(&line);
                            full_stderr.push('\n');
                            on_event(RustEvent::Stderr(line));
                        }
                        Ok(None) => {}
                        Err(e) => {
                            on_event(RustEvent::Error(e.to_string()));
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

        Ok(RustResult {
            stdout: full_stdout,
            stderr: full_stderr,
            exit_code: status.code().unwrap_or(-1),
            execution_time_ms: execution_time.as_millis() as u64,
            language: Language::Rust,
        })
    }

    /// Get Rust version
    pub async fn version(&self) -> Result<String> {
        let output = Command::new(&self.rustc_path)
            .arg("--version")
            .output()
            .await
            .map_err(|e| Error::ExecutionFailed(format!("Failed to get rustc version: {}", e)))?;

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Check syntax without running
    pub async fn check(&self, code: &str) -> Result<Vec<String>> {
        let temp_file = tempfile::Builder::new()
            .suffix(".rs")
            .tempfile()
            .map_err(|e| Error::Internal(format!("Failed to create temp file: {}", e)))?;

        let code = if !code.contains("fn main") {
            format!("fn main() {{\n{}\n}}", code)
        } else {
            code.to_string()
        };

        tokio::fs::write(temp_file.path(), code.as_bytes())
            .await
            .map_err(|e| Error::Internal(format!("Failed to write code: {}", e)))?;

        let output = Command::new(&self.rustc_path)
            .args([
                "--edition",
                &self.edition,
                "--emit=metadata",
                "-o",
                "/dev/null",
            ])
            .arg(temp_file.path())
            .output()
            .await
            .map_err(|e| Error::ExecutionFailed(format!("Failed to check: {}", e)))?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        let errors: Vec<String> = stderr
            .lines()
            .filter(|line| line.contains("error"))
            .map(|s| s.to_string())
            .collect();

        Ok(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_rust() {
        let executor = RustExecutor::new("/tmp");
        let code = r#"
fn main() {
    println!("Hello, Rust!");
}
"#;
        let result = executor.execute(code).await;

        // May fail if Rust not installed
        if let Ok(result) = result {
            assert!(result.stdout.contains("Hello, Rust!"));
            assert_eq!(result.exit_code, 0);
        }
    }

    #[tokio::test]
    async fn test_auto_wrap_main() {
        let executor = RustExecutor::new("/tmp");
        // Code without fn main
        let code = r#"println!("Auto wrapped!");"#;
        let result = executor.execute(code).await;

        if let Ok(result) = result {
            assert!(result.stdout.contains("Auto wrapped!"));
        }
    }
}
