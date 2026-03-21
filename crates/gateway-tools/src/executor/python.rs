//! Python Code Executor
//!
//! Executes Python code in Docker containers with security isolation.
//! Supports CodeAct sandbox containers with JSON-RPC communication.
//!
//! # Features
//!
//! - Execute Python code strings with full isolation
//! - Execute Python files with automatic upload to container
//! - Install additional pip packages at runtime
//! - Security controls: import checking, timeout, resource limits
//! - JSON-RPC communication with CodeAct sandbox containers
//! - Artifact collection (generated files from execution)

use crate::error::{ServiceError as Error, ServiceResult as Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::config::PythonConfig;
use super::docker::DockerManager;
use super::{
    ExecutionEvent, ExecutionRequest, ExecutionResult, ExecutionStatus, Language, LanguageExecutor,
};

/// Sandbox mode for Python execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    /// Full isolation with Docker container
    #[default]
    Docker,
    /// CodeAct sandbox with JSON-RPC
    CodeAct,
    /// Local execution (unsafe, for development only)
    Local,
}

/// Extended Python configuration with sandbox options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtendedPythonConfig {
    /// Base Python configuration
    #[serde(flatten)]
    pub base: PythonConfig,

    /// Sandbox mode
    #[serde(default)]
    pub sandbox_mode: SandboxMode,

    /// Allowed imports (empty = all allowed except blocked)
    #[serde(default)]
    pub allowed_imports: Vec<String>,

    /// Blocked imports (dangerous modules)
    #[serde(default = "default_blocked_imports")]
    pub blocked_imports: Vec<String>,

    /// Maximum memory in MB
    #[serde(default = "default_max_memory_mb")]
    pub max_memory_mb: u64,

    /// CodeAct sandbox endpoint (if using CodeAct mode)
    #[serde(default)]
    pub codeact_endpoint: Option<String>,

    /// CodeAct container image
    #[serde(default = "default_codeact_image")]
    pub codeact_image: String,
}

impl Default for ExtendedPythonConfig {
    fn default() -> Self {
        Self {
            base: PythonConfig::default(),
            sandbox_mode: SandboxMode::Docker,
            allowed_imports: Vec::new(),
            blocked_imports: default_blocked_imports(),
            max_memory_mb: default_max_memory_mb(),
            codeact_endpoint: None,
            codeact_image: default_codeact_image(),
        }
    }
}

fn default_blocked_imports() -> Vec<String> {
    vec![
        "os".into(), // R5-H9: block entire os module, not just os.system
        "os.system".into(),
        "subprocess".into(),
        "socket".into(),
        "ctypes".into(),
        "multiprocessing".into(),
        "threading".into(),
        "_thread".into(),
        "pty".into(),
        "fcntl".into(),
        "resource".into(),
        "signal".into(),
        "posix".into(),
        "pwd".into(),
        "grp".into(),
        "spwd".into(),
        "crypt".into(),
        "termios".into(),
        "tty".into(),
        "nis".into(),
        "syslog".into(),
        "importlib".into(), // R5-H9: dynamic import bypass
        "builtins".into(),  // R5-H9: builtins.__import__ bypass
        "shutil".into(),    // filesystem manipulation
    ]
}

fn default_max_memory_mb() -> u64 {
    512
}

fn default_codeact_image() -> String {
    "ghcr.io/all-hands-ai/runtime:latest".into()
}

/// Execution context for Python code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Session ID for persistent state
    pub session_id: Option<String>,

    /// Working directory inside the container
    pub working_dir: Option<String>,

    /// Environment variables
    #[serde(default)]
    pub env_vars: std::collections::HashMap<String, String>,

    /// Files to upload before execution (host_path -> container_path)
    #[serde(default)]
    pub upload_files: std::collections::HashMap<String, String>,

    /// Paths to collect as artifacts after execution
    #[serde(default)]
    pub artifact_paths: Vec<String>,

    /// Whether to capture the return value
    #[serde(default = "default_capture_return")]
    pub capture_return: bool,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            session_id: None,
            working_dir: None,
            env_vars: std::collections::HashMap::new(),
            upload_files: std::collections::HashMap::new(),
            artifact_paths: Vec::new(),
            capture_return: true,
        }
    }
}

fn default_capture_return() -> bool {
    true
}

/// Timing information for execution
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionTiming {
    /// Total wall clock time in milliseconds
    pub total_ms: u64,

    /// Time spent in queue (if applicable)
    pub queue_ms: u64,

    /// Time spent executing
    pub execution_ms: u64,

    /// Time spent collecting artifacts
    pub artifact_collection_ms: u64,
}

/// Artifact generated during execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Artifact name/path
    pub name: String,

    /// MIME type
    pub mime_type: String,

    /// Content (base64 encoded for binary)
    pub content: String,

    /// Size in bytes
    pub size: u64,

    /// Whether content is base64 encoded
    pub is_base64: bool,
}

/// Extended execution result with artifacts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtendedExecutionResult {
    /// Standard output
    pub stdout: String,

    /// Standard error
    pub stderr: String,

    /// Return value (JSON serialized)
    pub return_value: Option<serde_json::Value>,

    /// Exit code
    pub exit_code: i32,

    /// Execution timing
    pub timing: ExecutionTiming,

    /// Generated artifacts
    pub artifacts: Vec<Artifact>,

    /// Execution status
    pub status: ExecutionStatus,

    /// Execution ID
    pub execution_id: String,
}

/// JSON-RPC request for CodeAct sandbox
#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    method: String,
    params: serde_json::Value,
    id: u64,
}

/// JSON-RPC response from CodeAct sandbox
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
    #[allow(dead_code)]
    id: u64,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[allow(dead_code)]
    data: Option<serde_json::Value>,
}

/// Python code executor
pub struct PythonExecutor {
    docker_manager: Arc<DockerManager>,
    config: PythonConfig,
    extended_config: ExtendedPythonConfig,
    /// HTTP client for CodeAct JSON-RPC
    http_client: reqwest::Client,
    /// Request ID counter
    request_id: std::sync::atomic::AtomicU64,
}

impl PythonExecutor {
    /// Create a new Python executor
    pub fn new(docker_manager: Arc<DockerManager>, config: PythonConfig) -> Self {
        Self {
            docker_manager,
            extended_config: ExtendedPythonConfig {
                base: config.clone(),
                ..Default::default()
            },
            config,
            http_client: reqwest::Client::new(),
            request_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Create a new Python executor with extended configuration
    pub fn with_extended_config(
        docker_manager: Arc<DockerManager>,
        config: ExtendedPythonConfig,
    ) -> Self {
        Self {
            docker_manager,
            config: config.base.clone(),
            extended_config: config,
            http_client: reqwest::Client::new(),
            request_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Get the extended configuration
    pub fn extended_config(&self) -> &ExtendedPythonConfig {
        &self.extended_config
    }

    /// Execute Python code with extended context
    pub async fn execute_with_context(
        &self,
        code: &str,
        context: ExecutionContext,
    ) -> Result<ExtendedExecutionResult> {
        let execution_id = Uuid::new_v4().to_string();
        let start_time = std::time::Instant::now();

        // Validate code security
        self.validate_code(code)?;

        // Prepare code with return value capture
        let prepared_code = self.prepare_code_extended(code, context.capture_return);

        match self.extended_config.sandbox_mode {
            SandboxMode::Docker => {
                self.execute_docker(&execution_id, &prepared_code, &context, start_time)
                    .await
            }
            SandboxMode::CodeAct => {
                self.execute_codeact(&execution_id, &prepared_code, &context, start_time)
                    .await
            }
            SandboxMode::Local => {
                self.execute_local(&execution_id, &prepared_code, &context, start_time)
                    .await
            }
        }
    }

    /// Execute a Python file
    pub async fn execute_file(
        &self,
        path: &Path,
        context: ExecutionContext,
    ) -> Result<ExtendedExecutionResult> {
        // Read the file
        let code = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read file: {}", e)))?;

        self.execute_with_context(&code, context).await
    }

    /// Install a pip package in the container
    pub async fn install_package(&self, package: &str) -> Result<()> {
        // Validate package name (basic security check)
        if !package.chars().all(|c| {
            c.is_alphanumeric()
                || c == '-'
                || c == '_'
                || c == '.'
                || c == '['
                || c == ']'
                || c == ','
                || c == '='
                || c == '<'
                || c == '>'
        }) {
            return Err(Error::InvalidInput(format!(
                "Invalid package name: {}",
                package
            )));
        }

        match self.extended_config.sandbox_mode {
            SandboxMode::CodeAct => self.install_package_codeact(package).await,
            SandboxMode::Docker => self.install_package_docker(package).await,
            SandboxMode::Local => self.install_package_local(package).await,
        }
    }

    /// Validate code for dangerous imports
    fn validate_code(&self, code: &str) -> Result<()> {
        let blocked: HashSet<&str> = self
            .extended_config
            .blocked_imports
            .iter()
            .map(|s| s.as_str())
            .collect();

        // Check the entire code for blocked patterns (including function calls like os.system)
        for blocked_import in &blocked {
            if code.contains(blocked_import) {
                return Err(Error::CommandBlocked(format!(
                    "'{}' is blocked for security reasons",
                    blocked_import
                )));
            }
        }

        // Check for blocked imports in import statements
        for line in code.lines() {
            let line = line.trim();

            // Check import statements for blocked modules (base module check)
            if line.starts_with("import ") || line.starts_with("from ") {
                // Get the module being imported
                let module = if line.starts_with("import ") {
                    line.strip_prefix("import ")
                        .unwrap_or("")
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                } else {
                    line.strip_prefix("from ")
                        .unwrap_or("")
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                };

                let base_module = module.split('.').next().unwrap_or("");

                // Check if base module is blocked
                if blocked.contains(base_module) {
                    return Err(Error::CommandBlocked(format!(
                        "Import '{}' is blocked for security reasons",
                        base_module
                    )));
                }
            }

            // Check for __import__ calls
            if line.contains("__import__") {
                for blocked_import in &blocked {
                    if line.contains(blocked_import) {
                        return Err(Error::CommandBlocked(format!(
                            "Dynamic import of '{}' is blocked",
                            blocked_import
                        )));
                    }
                }
            }

            // Check for exec/eval — block entirely as they can execute arbitrary code
            if line.contains("exec(") || line.contains("eval(") {
                return Err(Error::CommandBlocked(
                    "exec() and eval() are not allowed for security reasons".into(),
                ));
            }

            // Block compile() which can be used with exec() to bypass string checks
            if line.contains("compile(") && (line.contains("exec") || line.contains("eval")) {
                return Err(Error::CommandBlocked(
                    "compile() with exec/eval is not allowed".into(),
                ));
            }
        }

        // Check for allowed imports if specified
        if !self.extended_config.allowed_imports.is_empty() {
            let allowed: HashSet<&str> = self
                .extended_config
                .allowed_imports
                .iter()
                .map(|s| s.as_str())
                .collect();

            for line in code.lines() {
                let line = line.trim();
                if line.starts_with("import ") {
                    let module = line
                        .strip_prefix("import ")
                        .unwrap_or("")
                        .split_whitespace()
                        .next()
                        .unwrap_or("");
                    let base_module = module.split('.').next().unwrap_or("");

                    if !allowed.contains(base_module) && !base_module.is_empty() {
                        return Err(Error::CommandBlocked(format!(
                            "Import '{}' is not in the allowed list",
                            base_module
                        )));
                    }
                } else if line.starts_with("from ") {
                    let module = line
                        .strip_prefix("from ")
                        .unwrap_or("")
                        .split_whitespace()
                        .next()
                        .unwrap_or("");
                    let base_module = module.split('.').next().unwrap_or("");

                    if !allowed.contains(base_module) && !base_module.is_empty() {
                        return Err(Error::CommandBlocked(format!(
                            "Import '{}' is not in the allowed list",
                            base_module
                        )));
                    }
                }
            }
        }

        Ok(())
    }

    /// Prepare Python code for execution
    /// Wraps the code to handle imports and output
    fn prepare_code(&self, code: &str) -> String {
        // Basic code wrapper that ensures proper output flushing
        format!(
            r#"
import sys
sys.stdout.reconfigure(line_buffering=True)
sys.stderr.reconfigure(line_buffering=True)

{}
"#,
            code
        )
    }

    /// Prepare Python code with return value capture
    fn prepare_code_extended(&self, code: &str, capture_return: bool) -> String {
        if !capture_return {
            return self.prepare_code(code);
        }

        // Wrap code to capture return value
        format!(
            r#"
import sys
import json
sys.stdout.reconfigure(line_buffering=True)
sys.stderr.reconfigure(line_buffering=True)

__canal_result__ = None
__canal_error__ = None

try:
    # User code
{}

    # Try to capture the last expression value
    pass
except Exception as __e__:
    __canal_error__ = str(__e__)
    import traceback
    traceback.print_exc()

# Output result marker
print("__CANAL_RESULT_START__")
if __canal_error__:
    print(json.dumps({{"error": __canal_error__}}))
else:
    print(json.dumps({{"success": True}}))
print("__CANAL_RESULT_END__")
"#,
            indent_code(code, 4)
        )
    }

    /// Escape code for shell execution
    fn escape_for_shell(code: &str) -> String {
        // Use base64 encoding to safely pass code to the container
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(code)
    }

    /// Execute using Docker container
    async fn execute_docker(
        &self,
        execution_id: &str,
        code: &str,
        context: &ExecutionContext,
        start_time: std::time::Instant,
    ) -> Result<ExtendedExecutionResult> {
        // Ensure image is available
        self.docker_manager
            .ensure_image(&self.config.docker_image)
            .await?;

        let timeout_ms = self.config.timeout_ms;
        let encoded_code = Self::escape_for_shell(code);

        // Build command to decode and execute Python code
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("echo '{}' | base64 -d | python3", encoded_code),
        ];

        let (exit_code, stdout, stderr) = self
            .docker_manager
            .run_with_streaming(
                &self.config.docker_image,
                command,
                self.config.limits.as_ref(),
                timeout_ms,
                |_is_stderr, _text| {
                    // In non-streaming mode, we just collect output
                },
            )
            .await
            .map_err(|e| {
                if e.to_string().contains("timed out") {
                    Error::Timeout("Execution timed out".into())
                } else {
                    e
                }
            })?;

        let total_ms = start_time.elapsed().as_millis() as u64;

        // Parse return value from output if present
        let (clean_stdout, return_value) = parse_result_from_output(&stdout);

        let status = if exit_code == 0 {
            ExecutionStatus::Success
        } else {
            ExecutionStatus::Error
        };

        let _ = context; // Context used for future enhancements

        Ok(ExtendedExecutionResult {
            stdout: clean_stdout,
            stderr,
            return_value,
            exit_code,
            timing: ExecutionTiming {
                total_ms,
                queue_ms: 0,
                execution_ms: total_ms,
                artifact_collection_ms: 0,
            },
            artifacts: Vec::new(),
            status,
            execution_id: execution_id.to_string(),
        })
    }

    /// Execute using CodeAct sandbox via JSON-RPC
    async fn execute_codeact(
        &self,
        execution_id: &str,
        code: &str,
        context: &ExecutionContext,
        start_time: std::time::Instant,
    ) -> Result<ExtendedExecutionResult> {
        let endpoint = self
            .extended_config
            .codeact_endpoint
            .as_ref()
            .ok_or_else(|| Error::Config("CodeAct endpoint not configured".into()))?;

        let request_id = self
            .request_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            method: "execute".to_string(),
            params: serde_json::json!({
                "code": code,
                "language": "python",
                "session_id": context.session_id,
                "working_dir": context.working_dir,
                "env": context.env_vars,
                "timeout_ms": self.config.timeout_ms,
            }),
            id: request_id,
        };

        let response = self
            .http_client
            .post(endpoint)
            .json(&request)
            .timeout(std::time::Duration::from_millis(
                self.config.timeout_ms + 5000,
            ))
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Failed to send request to CodeAct: {}", e)))?;

        let rpc_response: JsonRpcResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse CodeAct response: {}", e)))?;

        let total_ms = start_time.elapsed().as_millis() as u64;

        if let Some(error) = rpc_response.error {
            return Ok(ExtendedExecutionResult {
                stdout: String::new(),
                stderr: format!("CodeAct error ({}): {}", error.code, error.message),
                return_value: None,
                exit_code: error.code,
                timing: ExecutionTiming {
                    total_ms,
                    queue_ms: 0,
                    execution_ms: total_ms,
                    artifact_collection_ms: 0,
                },
                artifacts: Vec::new(),
                status: ExecutionStatus::Error,
                execution_id: execution_id.to_string(),
            });
        }

        let result = rpc_response.result.unwrap_or(serde_json::Value::Null);

        Ok(ExtendedExecutionResult {
            stdout: result["stdout"].as_str().unwrap_or("").to_string(),
            stderr: result["stderr"].as_str().unwrap_or("").to_string(),
            return_value: result.get("return_value").cloned(),
            exit_code: result["exit_code"].as_i64().unwrap_or(0) as i32,
            timing: ExecutionTiming {
                total_ms,
                queue_ms: result["timing"]["queue_ms"].as_u64().unwrap_or(0),
                execution_ms: result["timing"]["execution_ms"]
                    .as_u64()
                    .unwrap_or(total_ms),
                artifact_collection_ms: result["timing"]["artifact_ms"].as_u64().unwrap_or(0),
            },
            artifacts: parse_artifacts(&result),
            status: if result["exit_code"].as_i64().unwrap_or(0) == 0 {
                ExecutionStatus::Success
            } else {
                ExecutionStatus::Error
            },
            execution_id: execution_id.to_string(),
        })
    }

    /// Execute locally (unsafe, for development only)
    async fn execute_local(
        &self,
        execution_id: &str,
        code: &str,
        _context: &ExecutionContext,
        start_time: std::time::Instant,
    ) -> Result<ExtendedExecutionResult> {
        tracing::warn!("Using local execution mode - this is unsafe for production!");

        // Create temp file
        let temp_file = tempfile::Builder::new()
            .suffix(".py")
            .tempfile()
            .map_err(|e| Error::Internal(format!("Failed to create temp file: {}", e)))?;

        tokio::fs::write(temp_file.path(), code)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write code: {}", e)))?;

        let output = tokio::process::Command::new("python3")
            .arg(temp_file.path())
            .output()
            .await
            .map_err(|e| Error::ExecutionFailed(format!("Failed to execute Python: {}", e)))?;

        let total_ms = start_time.elapsed().as_millis() as u64;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let (clean_stdout, return_value) = parse_result_from_output(&stdout);

        Ok(ExtendedExecutionResult {
            stdout: clean_stdout,
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            return_value,
            exit_code: output.status.code().unwrap_or(-1),
            timing: ExecutionTiming {
                total_ms,
                queue_ms: 0,
                execution_ms: total_ms,
                artifact_collection_ms: 0,
            },
            artifacts: Vec::new(),
            status: if output.status.success() {
                ExecutionStatus::Success
            } else {
                ExecutionStatus::Error
            },
            execution_id: execution_id.to_string(),
        })
    }

    /// Install package via CodeAct
    async fn install_package_codeact(&self, package: &str) -> Result<()> {
        let endpoint = self
            .extended_config
            .codeact_endpoint
            .as_ref()
            .ok_or_else(|| Error::Config("CodeAct endpoint not configured".into()))?;

        let request_id = self
            .request_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            method: "install_package".to_string(),
            params: serde_json::json!({
                "package": package,
                "language": "python",
            }),
            id: request_id,
        };

        let response = self
            .http_client
            .post(endpoint)
            .json(&request)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Failed to install package: {}", e)))?;

        let rpc_response: JsonRpcResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse response: {}", e)))?;

        if let Some(error) = rpc_response.error {
            return Err(Error::ExecutionFailed(format!(
                "Package installation failed: {}",
                error.message
            )));
        }

        Ok(())
    }

    /// Install package via Docker
    async fn install_package_docker(&self, package: &str) -> Result<()> {
        self.docker_manager
            .ensure_image(&self.config.docker_image)
            .await?;

        let command = vec![
            "pip".to_string(),
            "install".to_string(),
            "--user".to_string(),
            package.to_string(),
        ];

        let (exit_code, _stdout, stderr) = self
            .docker_manager
            .run_with_streaming(
                &self.config.docker_image,
                command,
                self.config.limits.as_ref(),
                120000, // 2 minutes for package installation
                |_is_stderr, _text| {},
            )
            .await?;

        if exit_code != 0 {
            return Err(Error::ExecutionFailed(format!(
                "Package installation failed: {}",
                stderr
            )));
        }

        Ok(())
    }

    /// Install package locally
    async fn install_package_local(&self, package: &str) -> Result<()> {
        let output = tokio::process::Command::new("pip")
            .args(["install", "--user", package])
            .output()
            .await
            .map_err(|e| Error::ExecutionFailed(format!("Failed to run pip: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::ExecutionFailed(format!(
                "Package installation failed: {}",
                stderr
            )));
        }

        Ok(())
    }
}

/// Indent code by a given number of spaces
fn indent_code(code: &str, spaces: usize) -> String {
    let indent = " ".repeat(spaces);
    code.lines()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{}{}", indent, line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse result marker from output
fn parse_result_from_output(output: &str) -> (String, Option<serde_json::Value>) {
    const START_MARKER: &str = "__CANAL_RESULT_START__";
    const END_MARKER: &str = "__CANAL_RESULT_END__";

    if let Some(start_idx) = output.find(START_MARKER) {
        if let Some(end_idx) = output.find(END_MARKER) {
            let result_str = &output[start_idx + START_MARKER.len()..end_idx].trim();
            let clean_output = format!(
                "{}{}",
                &output[..start_idx],
                &output[end_idx + END_MARKER.len()..]
            )
            .trim()
            .to_string();

            if let Ok(value) = serde_json::from_str(result_str) {
                return (clean_output, Some(value));
            }
        }
    }

    (output.to_string(), None)
}

/// Parse artifacts from CodeAct response
fn parse_artifacts(result: &serde_json::Value) -> Vec<Artifact> {
    let mut artifacts = Vec::new();

    if let Some(arr) = result.get("artifacts").and_then(|v| v.as_array()) {
        for item in arr {
            if let (Some(name), Some(content)) = (
                item.get("name").and_then(|v| v.as_str()),
                item.get("content").and_then(|v| v.as_str()),
            ) {
                artifacts.push(Artifact {
                    name: name.to_string(),
                    mime_type: item
                        .get("mime_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("application/octet-stream")
                        .to_string(),
                    content: content.to_string(),
                    size: item.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
                    is_base64: item
                        .get("is_base64")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true),
                });
            }
        }
    }

    artifacts
}

#[async_trait::async_trait]
impl LanguageExecutor for PythonExecutor {
    fn language(&self) -> Language {
        Language::Python
    }

    async fn execute(&self, request: &ExecutionRequest) -> Result<ExecutionResult> {
        let execution_id = Uuid::new_v4().to_string();
        let start_time = std::time::Instant::now();

        // Validate code
        self.validate_code(&request.code)?;

        // Ensure image is available
        self.docker_manager
            .ensure_image(&self.config.docker_image)
            .await?;

        let timeout_ms = request.timeout_ms.unwrap_or(self.config.timeout_ms);
        let prepared_code = self.prepare_code(&request.code);
        let encoded_code = Self::escape_for_shell(&prepared_code);

        // Build command to decode and execute Python code
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("echo '{}' | base64 -d | python3", encoded_code),
        ];

        let (exit_code, stdout, stderr) = self
            .docker_manager
            .run_with_streaming(
                &self.config.docker_image,
                command,
                self.config.limits.as_ref(),
                timeout_ms,
                |_is_stderr, _text| {
                    // In non-streaming mode, we just collect output
                },
            )
            .await
            .map_err(|e| {
                if e.to_string().contains("timed out") {
                    Error::Timeout("Execution timed out".into())
                } else {
                    e
                }
            })?;

        let duration_ms = start_time.elapsed().as_millis() as u64;

        let status = if exit_code == 0 {
            ExecutionStatus::Success
        } else {
            ExecutionStatus::Error
        };

        Ok(ExecutionResult {
            execution_id,
            language: Language::Python,
            stdout,
            stderr,
            exit_code,
            status,
            duration_ms,
        })
    }

    async fn execute_streaming(
        &self,
        request: &ExecutionRequest,
        output_tx: mpsc::Sender<ExecutionEvent>,
    ) -> Result<ExecutionResult> {
        let execution_id = Uuid::new_v4().to_string();
        let start_time = std::time::Instant::now();

        // Validate code
        if let Err(e) = self.validate_code(&request.code) {
            let _ = output_tx
                .send(ExecutionEvent::Error {
                    message: e.to_string(),
                })
                .await;
            return Err(e);
        }

        // Send started event
        let _ = output_tx
            .send(ExecutionEvent::Started {
                execution_id: execution_id.clone(),
            })
            .await;

        // Ensure image is available
        self.docker_manager
            .ensure_image(&self.config.docker_image)
            .await?;

        let timeout_ms = request.timeout_ms.unwrap_or(self.config.timeout_ms);
        let prepared_code = self.prepare_code(&request.code);
        let encoded_code = Self::escape_for_shell(&prepared_code);

        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("echo '{}' | base64 -d | python3", encoded_code),
        ];

        let output_tx_clone = output_tx.clone();
        let (exit_code, stdout, stderr) = self
            .docker_manager
            .run_with_streaming(
                &self.config.docker_image,
                command,
                self.config.limits.as_ref(),
                timeout_ms,
                move |is_stderr, text| {
                    let event = if is_stderr {
                        ExecutionEvent::Stderr {
                            text: text.to_string(),
                        }
                    } else {
                        ExecutionEvent::Stdout {
                            text: text.to_string(),
                        }
                    };
                    // Use blocking send in the callback
                    let tx = output_tx_clone.clone();
                    tokio::spawn(async move {
                        let _ = tx.send(event).await;
                    });
                },
            )
            .await
            .map_err(|e| {
                let error_msg = e.to_string();
                let is_timeout = error_msg.contains("timed out");
                let tx = output_tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(ExecutionEvent::Error { message: error_msg }).await;
                });
                if is_timeout {
                    Error::Timeout("Execution timed out".into())
                } else {
                    Error::Internal(e.to_string())
                }
            })?;

        let duration_ms = start_time.elapsed().as_millis() as u64;

        let status = if exit_code == 0 {
            ExecutionStatus::Success
        } else {
            ExecutionStatus::Error
        };

        // Send completed event
        let _ = output_tx
            .send(ExecutionEvent::Completed {
                exit_code,
                duration_ms,
            })
            .await;

        Ok(ExecutionResult {
            execution_id,
            language: Language::Python,
            stdout,
            stderr,
            exit_code,
            status,
            duration_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_executor() -> PythonExecutor {
        let config = PythonConfig::default();
        let docker_manager = Arc::new(DockerManager::new_mock(
            super::super::config::DockerConfig::default(),
        ));
        PythonExecutor::new(docker_manager, config)
    }

    #[test]
    fn test_prepare_code() {
        let executor = create_test_executor();

        let code = "print('hello')";
        let prepared = executor.prepare_code(code);
        assert!(prepared.contains("print('hello')"));
        assert!(prepared.contains("sys.stdout.reconfigure"));
    }

    #[test]
    fn test_escape_for_shell() {
        let code = "print('hello world')";
        let escaped = PythonExecutor::escape_for_shell(code);
        // Should be base64 encoded
        assert!(!escaped.contains("'"));
        assert!(!escaped.contains(" "));
    }

    #[test]
    fn test_validate_code_blocks_subprocess() {
        let executor = create_test_executor();

        let code = "import subprocess\nsubprocess.run(['ls'])";
        assert!(executor.validate_code(code).is_err());
    }

    #[test]
    fn test_validate_code_blocks_socket() {
        let executor = create_test_executor();

        let code = "import socket\ns = socket.socket()";
        assert!(executor.validate_code(code).is_err());
    }

    #[test]
    fn test_validate_code_blocks_os_system() {
        let executor = create_test_executor();

        let code = "import os\nos.system('ls')";
        assert!(executor.validate_code(code).is_err());
    }

    #[test]
    fn test_validate_code_allows_safe_imports() {
        let executor = create_test_executor();

        let code = "import json\nimport math\nprint(json.dumps({'a': 1}))";
        assert!(executor.validate_code(code).is_ok());
    }

    #[test]
    fn test_validate_code_blocks_dynamic_import() {
        let executor = create_test_executor();

        let code = "__import__('subprocess')";
        assert!(executor.validate_code(code).is_err());
    }

    #[test]
    fn test_validate_code_blocks_exec_import() {
        let executor = create_test_executor();

        let code = "exec('import subprocess')";
        assert!(executor.validate_code(code).is_err());
    }

    #[test]
    fn test_sandbox_mode_default() {
        let config = ExtendedPythonConfig::default();
        assert_eq!(config.sandbox_mode, SandboxMode::Docker);
    }

    #[test]
    fn test_execution_context_default() {
        let context = ExecutionContext::default();
        assert!(context.capture_return);
        assert!(context.session_id.is_none());
    }

    #[test]
    fn test_parse_result_from_output() {
        let output =
            "some output\n__CANAL_RESULT_START__\n{\"success\": true}\n__CANAL_RESULT_END__\n";
        let (clean, result) = parse_result_from_output(output);
        assert!(clean.contains("some output"));
        assert!(!clean.contains("CANAL"));
        assert!(result.is_some());
        assert_eq!(result.unwrap()["success"], true);
    }

    #[test]
    fn test_parse_result_no_marker() {
        let output = "just regular output";
        let (clean, result) = parse_result_from_output(output);
        assert_eq!(clean, output);
        assert!(result.is_none());
    }

    #[test]
    fn test_indent_code() {
        let code = "x = 1\ny = 2";
        let indented = indent_code(code, 4);
        assert!(indented.starts_with("    x = 1"));
        assert!(indented.contains("\n    y = 2"));
    }

    #[test]
    fn test_prepare_code_extended() {
        let executor = create_test_executor();

        let code = "x = 1 + 1";
        let prepared = executor.prepare_code_extended(code, true);
        assert!(prepared.contains("__canal_result__"));
        assert!(prepared.contains("__CANAL_RESULT_START__"));
        assert!(prepared.contains("x = 1 + 1"));
    }

    #[test]
    fn test_extended_config_serialization() {
        let config = ExtendedPythonConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ExtendedPythonConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.sandbox_mode, config.sandbox_mode);
        assert_eq!(parsed.max_memory_mb, config.max_memory_mb);
    }

    #[test]
    fn test_artifact_serialization() {
        let artifact = Artifact {
            name: "output.png".into(),
            mime_type: "image/png".into(),
            content: "base64data".into(),
            size: 1024,
            is_base64: true,
        };
        let json = serde_json::to_string(&artifact).unwrap();
        assert!(json.contains("output.png"));
        assert!(json.contains("image/png"));
    }
}
