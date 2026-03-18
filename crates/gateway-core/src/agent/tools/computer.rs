//! Computer Tool - CodeAct Code Execution
//!
//! This tool allows the agent to execute code (Python, Bash) in a sandboxed environment.
//! It is the primary tool for CodeAct-style agents that use code as their action space.

use super::{AgentTool, ToolContext, ToolError, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::executor::{CodeActConfig, CodeActEngine, CodeActRequest, Language};

/// Computer tool input
#[derive(Debug, Clone, Deserialize)]
pub struct ComputerInput {
    /// Code to execute
    pub code: String,
    /// Programming language (python, bash, javascript)
    #[serde(default = "default_language")]
    pub language: String,
    /// Timeout in milliseconds
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Whether to capture and return expression result
    #[serde(default)]
    pub capture_return: bool,
}

fn default_language() -> String {
    "python".to_string()
}

/// Computer tool output
#[derive(Debug, Clone, Serialize)]
pub struct ComputerOutput {
    /// Standard output from the execution
    pub stdout: String,
    /// Standard error output
    pub stderr: String,
    /// Exit code (0 = success)
    pub exit_code: i32,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
    /// Whether execution was successful
    pub success: bool,
}

/// Computer tool for executing code in a sandboxed environment
pub struct ComputerTool {
    /// CodeAct engine for execution
    engine: Arc<RwLock<Option<CodeActEngine>>>,
    /// Configuration
    config: CodeActConfig,
    /// Default timeout in milliseconds
    default_timeout_ms: u64,
}

impl Default for ComputerTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ComputerTool {
    /// Create a new computer tool
    pub fn new() -> Self {
        Self {
            engine: Arc::new(RwLock::new(None)),
            config: CodeActConfig::default(),
            default_timeout_ms: 30_000,
        }
    }

    /// Create with custom configuration
    pub fn with_config(config: CodeActConfig) -> Self {
        let default_timeout_ms = config.default_timeout_ms;
        Self {
            engine: Arc::new(RwLock::new(None)),
            config,
            default_timeout_ms,
        }
    }

    /// Set the CodeAct engine
    pub fn with_engine(mut self, engine: CodeActEngine) -> Self {
        self.engine = Arc::new(RwLock::new(Some(engine)));
        self
    }

    /// Get or initialize the engine.
    ///
    /// Uses write lock during initialization to prevent TOCTOU race
    /// where two concurrent callers both see None and double-init.
    async fn get_engine(&self) -> Result<Arc<RwLock<Option<CodeActEngine>>>, ToolError> {
        // Fast path: check with read lock
        {
            let engine_guard = self.engine.read().await;
            if engine_guard.is_some() {
                return Ok(self.engine.clone());
            }
        }

        // Slow path: acquire write lock, then double-check
        let mut engine_guard = self.engine.write().await;
        if engine_guard.is_some() {
            // Another task initialized while we waited for the write lock
            return Ok(self.engine.clone());
        }

        // Initialize engine while holding write lock
        let engine = CodeActEngine::new(self.config.clone()).await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to initialize CodeAct engine: {}", e))
        })?;

        // Start the engine
        engine.start().await.map_err(|e| {
            ToolError::ExecutionError(format!("Failed to start CodeAct engine: {}", e))
        })?;

        *engine_guard = Some(engine);

        Ok(self.engine.clone())
    }

    /// Parse language string to Language enum
    fn parse_language(lang: &str) -> Language {
        match lang.to_lowercase().as_str() {
            "python" | "py" => Language::Python,
            "bash" | "sh" | "shell" => Language::Bash,
            "javascript" | "js" => Language::JavaScript,
            "typescript" | "ts" => Language::TypeScript,
            "go" | "golang" => Language::Go,
            "rust" | "rs" => Language::Rust,
            _ => Language::Python, // Default to Python
        }
    }
}

#[async_trait]
impl AgentTool for ComputerTool {
    type Input = ComputerInput;
    type Output = ComputerOutput;

    fn name(&self) -> &str {
        "computer"
    }

    fn description(&self) -> &str {
        r#"Execute code in a sandboxed environment.

Use this tool to run Python or Bash code to accomplish tasks. This is your primary way to take action.

Examples:
- Fetch data from an API
- Process and analyze data
- Read and write files
- Run shell commands
- Automate tasks

The code runs in an isolated container with:
- Python 3.11 with common libraries (requests, pandas, numpy, beautifulsoup4, etc.)
- Bash shell with standard Unix tools
- Access to /workspace directory for file operations
- Network access for HTTP requests

Tips:
- Use Python for data processing, API calls, and complex logic
- Use Bash for simple file operations and shell commands
- Print results to see output
- Handle errors gracefully with try/except"#
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "The code to execute"
                },
                "language": {
                    "type": "string",
                    "enum": ["python", "bash", "javascript"],
                    "default": "python",
                    "description": "Programming language (default: python)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 30000)"
                }
            },
            "required": ["code"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "codeact"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        let language = Self::parse_language(&input.language);
        let timeout_ms = input.timeout_ms.unwrap_or(self.default_timeout_ms);

        // Build CodeAct request
        let mut request = match language {
            Language::Python => CodeActRequest::python(&input.code),
            Language::Bash => CodeActRequest::bash(&input.code),
            other => {
                // R1-M2: Log unsupported language instead of silent fallback
                tracing::warn!(language = ?other, "Unsupported language for CodeAct, falling back to Python");
                CodeActRequest::python(&input.code)
            }
        };

        request = request.with_timeout(timeout_ms);
        if input.capture_return {
            request = request.with_capture_return();
        }

        // Get or initialize engine
        let engine_arc: Arc<RwLock<Option<CodeActEngine>>> = self.get_engine().await?;
        let engine_guard: tokio::sync::RwLockReadGuard<'_, Option<CodeActEngine>> =
            engine_arc.read().await;

        let engine: &CodeActEngine = engine_guard
            .as_ref()
            .ok_or_else(|| ToolError::ExecutionError("CodeAct engine not available".to_string()))?;

        // Execute
        let result: crate::executor::ExecutionResult = engine
            .execute(request)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Code execution failed: {}", e)))?;

        Ok(ComputerOutput {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
            duration_ms: result.duration_ms,
            success: result.exit_code == 0,
        })
    }
}

/// Fallback computer tool that uses local execution (no Docker)
/// Used when Docker/containers are not available
pub struct LocalComputerTool {
    /// Default timeout in milliseconds
    default_timeout_ms: u64,
    /// Maximum output length
    max_output_length: usize,
}

impl Default for LocalComputerTool {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalComputerTool {
    /// Create a new local computer tool
    pub fn new() -> Self {
        Self {
            default_timeout_ms: 30_000,
            max_output_length: 50_000,
        }
    }
}

#[async_trait]
impl AgentTool for LocalComputerTool {
    type Input = ComputerInput;
    type Output = ComputerOutput;

    fn name(&self) -> &str {
        "computer"
    }

    fn description(&self) -> &str {
        r#"Execute code locally.

Use this tool to run Python or Bash code to accomplish tasks.

Examples:
- Fetch data from an API
- Process and analyze data
- Read and write files
- Run shell commands

Note: This runs code locally on the host system. Use with caution."#
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "The code to execute"
                },
                "language": {
                    "type": "string",
                    "enum": ["python", "bash"],
                    "default": "python",
                    "description": "Programming language (default: python)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 30000)"
                }
            },
            "required": ["code"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "codeact"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::process::Command;
        use tokio::time::{timeout, Duration};

        // R1-C3: Validate code input against dangerous patterns before execution
        let code_lower = input.code.to_lowercase();
        let code_normalized: String = code_lower.split_whitespace().collect::<Vec<_>>().join(" ");
        let code_stripped: String = code_normalized
            .chars()
            .filter(|c| !matches!(c, '\'' | '"' | '\\'))
            .collect();

        let dangerous_patterns = [
            "rm -rf /",
            "mkfs",
            "dd if=/dev",
            "shred",
            "wipefs",
            ":(){ :|:& };:",
            "chmod -r 000 /",
            "> /dev/sd",
            "> /dev/nvme",
        ];
        for pattern in &dangerous_patterns {
            if code_normalized.contains(pattern) || code_stripped.contains(pattern) {
                return Err(ToolError::InvalidInput(format!(
                    "Code blocked for safety: contains dangerous pattern '{}'",
                    pattern
                )));
            }
        }

        // Block dangerous first-token commands in bash mode
        if matches!(
            input.language.to_lowercase().as_str(),
            "bash" | "sh" | "shell"
        ) {
            let first_token = code_normalized.split_whitespace().next().unwrap_or("");
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
                return Err(ToolError::InvalidInput(format!(
                    "Command '{}' is blocked for safety",
                    first_token
                )));
            }
        }

        let timeout_ms = input.timeout_ms.unwrap_or(self.default_timeout_ms);
        let start = std::time::Instant::now();

        // Determine command based on language
        let lang_lower = input.language.to_lowercase();
        let (cmd, args) = match lang_lower.as_str() {
            "bash" | "sh" | "shell" => ("bash", vec!["-c", &input.code]),
            "python" | "py" => ("python3", vec!["-c", &input.code]),
            other => {
                // R1-M2: Log unsupported language instead of silent fallback
                tracing::warn!(
                    language = other,
                    "Unsupported language for local executor, falling back to Python"
                );
                ("python3", vec!["-c", &input.code])
            }
        };

        // Spawn process
        let mut child = Command::new(cmd)
            .args(&args)
            .current_dir(&context.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ToolError::ExecutionError(format!("Failed to spawn process: {}", e)))?;

        // Collect output with timeout
        let result = timeout(Duration::from_millis(timeout_ms), async {
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let mut stdout_str = String::new();
            let mut stderr_str = String::new();

            if let Some(stdout) = stdout {
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    stdout_str.push_str(&line);
                    stdout_str.push('\n');
                    if stdout_str.len() > self.max_output_length {
                        stdout_str.truncate(self.max_output_length);
                        stdout_str.push_str("\n... (output truncated)");
                        break;
                    }
                }
            }

            if let Some(stderr) = stderr {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    stderr_str.push_str(&line);
                    stderr_str.push('\n');
                    if stderr_str.len() > self.max_output_length {
                        stderr_str.truncate(self.max_output_length);
                        stderr_str.push_str("\n... (output truncated)");
                        break;
                    }
                }
            }

            let status = child.wait().await?;
            Ok::<_, std::io::Error>((stdout_str, stderr_str, status.code().unwrap_or(-1)))
        })
        .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok((stdout, stderr, exit_code))) => Ok(ComputerOutput {
                stdout,
                stderr,
                exit_code,
                duration_ms,
                success: exit_code == 0,
            }),
            Ok(Err(e)) => Err(ToolError::ExecutionError(e.to_string())),
            Err(_) => {
                // Timeout - kill process
                let _ = child.kill().await;
                Ok(ComputerOutput {
                    stdout: String::new(),
                    stderr: format!("Execution timed out after {}ms", timeout_ms),
                    exit_code: -1,
                    duration_ms,
                    success: false,
                })
            }
        }
    }
}

/// Unified Computer Tool - Routes execution through UnifiedCodeActRouter
///
/// This tool wraps the UnifiedCodeActRouter to provide the agent with
/// code execution that automatically routes to the best available backend
/// (K8s, Docker, Firecracker, or local fallback).
pub struct UnifiedComputerTool {
    /// The unified router for code execution
    router: Arc<crate::executor::UnifiedCodeActRouter>,
    /// Local fallback when router has no backends available
    local_fallback: LocalComputerTool,
    /// Default timeout in milliseconds
    default_timeout_ms: u64,
}

impl UnifiedComputerTool {
    /// Create a new unified computer tool
    pub fn new(router: Arc<crate::executor::UnifiedCodeActRouter>) -> Self {
        Self {
            router,
            local_fallback: LocalComputerTool::new(),
            default_timeout_ms: 30_000,
        }
    }

    /// Set default timeout
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.default_timeout_ms = timeout_ms;
        self
    }
}

#[async_trait]
impl AgentTool for UnifiedComputerTool {
    type Input = ComputerInput;
    type Output = ComputerOutput;

    fn name(&self) -> &str {
        "computer"
    }

    fn description(&self) -> &str {
        r#"Execute code in an isolated environment.

Use this tool to run Python or Bash code to accomplish tasks. This is your primary way to take action.

Examples:
- Fetch data from an API
- Process and analyze data
- Read and write files
- Run shell commands
- Automate tasks

The code runs in an isolated environment (container or VM) with:
- Python 3.11 with common libraries (requests, pandas, numpy, beautifulsoup4, etc.)
- Bash shell with standard Unix tools
- Access to /workspace directory for file operations
- Network access for HTTP requests

Tips:
- Use Python for data processing, API calls, and complex logic
- Use Bash for simple file operations and shell commands
- Print results to see output
- Handle errors gracefully with try/except"#
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "The code to execute"
                },
                "language": {
                    "type": "string",
                    "enum": ["python", "bash", "javascript"],
                    "default": "python",
                    "description": "Programming language (default: python)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 30000)"
                }
            },
            "required": ["code"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "codeact"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        use crate::executor::router::CodeExecutionRequest;

        let timeout_ms = input.timeout_ms.unwrap_or(self.default_timeout_ms);

        // Check if router has any available backends
        let status = self.router.get_status().await;
        if !status.any_available {
            // Fallback to local execution
            return self.local_fallback.execute(input, context).await;
        }

        // Build a CodeExecutionRequest for the router
        let request = CodeExecutionRequest::new(&input.code, &input.language)
            .with_timeout(timeout_ms)
            .with_session(context.session_id.clone());

        // Execute via the router
        match self.router.execute(request).await {
            Ok(result) => {
                let success = result.is_success();
                let exit_code = result.exit_code;
                let total_ms = result.timing.total_ms;
                let stdout = result.raw_stdout.clone();
                let stderr = result.raw_stderr.clone();
                let output = result.output.clone().unwrap_or_default();

                Ok(ComputerOutput {
                    stdout: if stdout.is_empty() { output } else { stdout },
                    stderr,
                    exit_code,
                    duration_ms: total_ms,
                    success,
                })
            }
            Err(e) => {
                // If router fails, try local fallback
                tracing::warn!("Router execution failed, falling back to local: {}", e);
                self.local_fallback.execute(input, context).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::PermissionMode;

    #[tokio::test]
    async fn test_local_computer_tool_python() {
        let tool = LocalComputerTool::new();
        let context = ToolContext::new("test-session", "/tmp")
            .with_permission_mode(PermissionMode::AcceptEdits);

        let input = ComputerInput {
            code: "print('Hello from CodeAct!')".to_string(),
            language: "python".to_string(),
            timeout_ms: None,
            capture_return: false,
        };

        let result = tool.execute(input, &context).await.unwrap();
        assert!(result.success);
        assert!(result.stdout.contains("Hello from CodeAct!"));
    }

    #[tokio::test]
    async fn test_local_computer_tool_bash() {
        let tool = LocalComputerTool::new();
        let context = ToolContext::new("test-session", "/tmp")
            .with_permission_mode(PermissionMode::AcceptEdits);

        let input = ComputerInput {
            code: "echo 'Hello from Bash'".to_string(),
            language: "bash".to_string(),
            timeout_ms: None,
            capture_return: false,
        };

        let result = tool.execute(input, &context).await.unwrap();
        assert!(result.success);
        assert!(result.stdout.contains("Hello from Bash"));
    }

    #[tokio::test]
    async fn test_local_computer_tool_timeout() {
        let tool = LocalComputerTool::new();
        let context = ToolContext::new("test-session", "/tmp")
            .with_permission_mode(PermissionMode::AcceptEdits);

        let input = ComputerInput {
            code: "import time; time.sleep(10)".to_string(),
            language: "python".to_string(),
            timeout_ms: Some(100), // 100ms timeout
            capture_return: false,
        };

        let result = tool.execute(input, &context).await.unwrap();
        assert!(!result.success);
        assert!(result.stderr.contains("timed out"));
    }

    #[tokio::test]
    async fn test_local_computer_tool_blocks_dangerous_bash() {
        let tool = LocalComputerTool::new();
        let context = ToolContext::new("test-session", "/tmp")
            .with_permission_mode(PermissionMode::AcceptEdits);

        let input = ComputerInput {
            code: "rm -rf /".to_string(),
            language: "bash".to_string(),
            timeout_ms: None,
            capture_return: false,
        };

        let result = tool.execute(input, &context).await;
        assert!(result.is_err(), "dangerous command should be blocked");
    }

    #[tokio::test]
    async fn test_local_computer_tool_blocks_dangerous_python() {
        let tool = LocalComputerTool::new();
        let context = ToolContext::new("test-session", "/tmp")
            .with_permission_mode(PermissionMode::AcceptEdits);

        let input = ComputerInput {
            code: "import os; os.system('dd if=/dev/zero of=/dev/sda')".to_string(),
            language: "python".to_string(),
            timeout_ms: None,
            capture_return: false,
        };

        let result = tool.execute(input, &context).await;
        assert!(result.is_err(), "dangerous code should be blocked");
    }
}
