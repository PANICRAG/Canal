//! Code Orchestration Runtime - Sandbox execution with tool proxy bridge
//!
//! Orchestrates the complete lifecycle of programmatic tool calling:
//! 1. Start ToolProxyBridge (HTTP server)
//! 2. Generate SDK preamble via ToolCodeGenerator
//! 3. Combine preamble + LLM code
//! 4. Execute in Docker sandbox via CodeExecutor
//! 5. Collect tool call records
//! 6. Shutdown ToolProxyBridge

use std::sync::Arc;
use std::time::Instant;

use super::codegen::ToolCodeGenerator;
use super::tool_proxy::ToolProxyBridge;
use super::types::{CodeOrchestrationConfig, CodeOrchestrationRequest, CodeOrchestrationResult};
use crate::agent::tools::{ToolContext, ToolRegistry};
use crate::error::{Error, Result};
use crate::executor::{CodeExecutor, ExecutionRequest as ExecRequest, Language};

/// Runtime for executing code orchestration requests
///
/// Manages the complete lifecycle of a programmatic tool calling session,
/// including proxy bridge, SDK generation, sandbox execution, and cleanup.
pub struct CodeOrchestrationRuntime {
    tool_registry: Arc<ToolRegistry>,
    code_executor: Option<Arc<CodeExecutor>>,
    config: CodeOrchestrationConfig,
}

impl CodeOrchestrationRuntime {
    /// Create a new CodeOrchestrationRuntime
    pub fn new(tool_registry: Arc<ToolRegistry>, config: CodeOrchestrationConfig) -> Self {
        Self {
            tool_registry,
            code_executor: None,
            config,
        }
    }

    /// Set the code executor (for Docker sandbox execution)
    pub fn with_code_executor(mut self, executor: Arc<CodeExecutor>) -> Self {
        self.code_executor = Some(executor);
        self
    }

    /// Execute a code orchestration request
    ///
    /// This is the main entry point that handles the complete lifecycle:
    /// 1. Validates the request
    /// 2. Starts the tool proxy bridge
    /// 3. Generates SDK preamble
    /// 4. Executes code in sandbox
    /// 5. Collects results and tool call records
    /// 6. Cleans up
    pub async fn execute(
        &self,
        request: CodeOrchestrationRequest,
        tool_context: ToolContext,
    ) -> Result<CodeOrchestrationResult> {
        if !self.config.enabled {
            return Err(Error::CodeOrchestration(
                "Code orchestration is not enabled".to_string(),
            ));
        }

        let start_time = Instant::now();

        // Validate language
        if request.language != "python" && request.language != "javascript" {
            return Err(Error::UnsupportedLanguage(format!(
                "Code orchestration only supports 'python' and 'javascript', got '{}'",
                request.language
            )));
        }

        // Step 1: Start the tool proxy bridge
        let mut proxy =
            ToolProxyBridge::new(self.tool_registry.clone(), self.config.max_tool_calls);
        let port = proxy.start(tool_context).await?;

        // Step 2: Generate SDK preamble
        let context_json =
            serde_json::to_string(&request.context_data).unwrap_or_else(|_| "{}".to_string());

        let tool_metadata = self.tool_registry.get_tool_metadata();
        let preamble = ToolCodeGenerator::generate_preamble(
            &request.language,
            port,
            &context_json,
            &tool_metadata,
        )
        .map_err(|e| Error::CodeOrchestration(e))?;

        // Step 3: Combine preamble + user code
        let full_code = format!("{}\n{}", preamble, request.code);

        // Step 4: Execute in sandbox
        let exec_result = self.execute_in_sandbox(&full_code, &request).await;

        // Step 5: Collect tool call records
        let tool_calls = proxy.get_recorded_calls().await;

        // Step 6: Cleanup
        proxy.shutdown();

        let duration_ms = start_time.elapsed().as_millis() as u64;

        // Build result
        match exec_result {
            Ok(exec) => {
                // Try to parse stdout as JSON for return_value
                let return_value = if exec.exit_code == 0 {
                    // Try to parse the last non-empty line as JSON
                    let trimmed = exec.stdout.trim();
                    if !trimmed.is_empty() {
                        // Try parsing the entire stdout as JSON
                        serde_json::from_str(trimmed).ok()
                    } else {
                        None
                    }
                } else {
                    None
                };

                let error = if exec.exit_code != 0 {
                    Some(format!(
                        "Code exited with code {}. stderr: {}",
                        exec.exit_code,
                        exec.stderr.chars().take(500).collect::<String>()
                    ))
                } else {
                    None
                };

                Ok(CodeOrchestrationResult {
                    success: exec.exit_code == 0,
                    return_value,
                    stdout: exec.stdout,
                    stderr: exec.stderr,
                    exit_code: exec.exit_code,
                    tool_calls,
                    duration_ms,
                    error,
                })
            }
            Err(e) => Ok(CodeOrchestrationResult {
                success: false,
                return_value: None,
                stdout: String::new(),
                stderr: e.to_string(),
                exit_code: -1,
                tool_calls,
                duration_ms,
                error: Some(e.to_string()),
            }),
        }
    }

    /// Execute code in the sandbox (Docker or local fallback)
    async fn execute_in_sandbox(
        &self,
        full_code: &str,
        request: &CodeOrchestrationRequest,
    ) -> Result<SandboxResult> {
        if let Some(ref executor) = self.code_executor {
            // Use Docker sandbox
            let language = match request.language.as_str() {
                "python" => Language::Python,
                "javascript" => Language::JavaScript,
                _ => return Err(Error::UnsupportedLanguage(request.language.clone())),
            };

            let exec_request = ExecRequest {
                code: full_code.to_string(),
                language,
                timeout_ms: Some(request.timeout.as_millis() as u64),
                stream: false,
                working_dir: None,
            };

            let result = tokio::time::timeout(request.timeout, executor.execute(exec_request))
                .await
                .map_err(|_| Error::CodeOrchestrationTimeout(request.timeout.as_millis() as u64))?
                .map_err(|e| {
                    Error::CodeOrchestration(format!("Sandbox execution failed: {}", e))
                })?;

            Ok(SandboxResult {
                stdout: result.stdout,
                stderr: result.stderr,
                exit_code: result.exit_code,
            })
        } else {
            // Fallback: execute locally using std::process::Command
            // This is less secure but works when Docker is unavailable
            self.execute_local(full_code, request).await
        }
    }

    /// Local execution fallback (when Docker is unavailable)
    async fn execute_local(
        &self,
        full_code: &str,
        request: &CodeOrchestrationRequest,
    ) -> Result<SandboxResult> {
        let (cmd, args) = match request.language.as_str() {
            "python" => ("python3", vec!["-c", full_code]),
            "javascript" => ("node", vec!["-e", full_code]),
            _ => return Err(Error::UnsupportedLanguage(request.language.clone())),
        };

        let output = tokio::time::timeout(
            request.timeout,
            tokio::process::Command::new(cmd).args(&args).output(),
        )
        .await
        .map_err(|_| Error::CodeOrchestrationTimeout(request.timeout.as_millis() as u64))?
        .map_err(|e| Error::CodeOrchestration(format!("Local execution failed: {}", e)))?;

        Ok(SandboxResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}

/// Simplified execution result
struct SandboxResult {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_creation() {
        let registry = Arc::new(ToolRegistry::new());
        let config = CodeOrchestrationConfig::default();
        let runtime = CodeOrchestrationRuntime::new(registry, config);

        assert!(runtime.code_executor.is_none());
    }

    #[test]
    fn test_runtime_disabled() {
        let registry = Arc::new(ToolRegistry::new());
        let config = CodeOrchestrationConfig::default(); // enabled=false by default
        let runtime = CodeOrchestrationRuntime::new(registry, config);

        let _request = CodeOrchestrationRequest::python("print('hello')");
        let _context = ToolContext::new("test", std::path::Path::new("/tmp"));

        // Can't run async test in sync block, but we verify config
        assert!(!runtime.config.enabled);
    }
}
