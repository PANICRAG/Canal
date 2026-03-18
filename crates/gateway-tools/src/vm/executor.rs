//! VM Code Executor
//!
//! Provides execution of Python code and browser actions inside Firecracker VMs.
//! Communicates with the VM's API server via HTTP.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use crate::vm::manager::VmInstance;
use async_stream::stream;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, info, instrument};
use uuid::Uuid;

fn default_timeout_ms() -> u64 {
    30000
}

fn default_capture_output() -> bool {
    true
}

/// Execution context for Python code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Environment variables to set
    #[serde(default)]
    pub env_vars: HashMap<String, String>,
    /// Working directory for execution
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Variables from previous executions (for REPL-like sessions)
    #[serde(default)]
    pub session_vars: HashMap<String, serde_json::Value>,
    /// Timeout in milliseconds
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Whether to capture stdout/stderr separately
    #[serde(default = "default_capture_output")]
    pub capture_output: bool,
    /// Restricted imports (for sandboxing)
    #[serde(default)]
    pub allowed_imports: Option<Vec<String>>,
    /// Whether to run in sandbox mode (restricted builtins)
    #[serde(default)]
    pub sandbox_mode: bool,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            env_vars: HashMap::new(),
            working_dir: None,
            session_vars: HashMap::new(),
            timeout_ms: default_timeout_ms(),
            capture_output: default_capture_output(),
            allowed_imports: None,
            sandbox_mode: false,
        }
    }
}

/// Result of Python code execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Unique execution ID
    pub execution_id: String,
    /// Whether execution succeeded
    pub success: bool,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Return value (if any)
    pub return_value: Option<serde_json::Value>,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
    /// Exit code (0 = success)
    pub exit_code: i32,
    /// Error message (if execution failed)
    pub error: Option<String>,
    /// Variables captured after execution (for REPL sessions)
    #[serde(default)]
    pub captured_vars: HashMap<String, serde_json::Value>,
}

/// Browser action types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BrowserAction {
    /// Navigate to a URL
    Navigate {
        url: String,
        #[serde(default = "default_wait_until")]
        wait_until: String,
        #[serde(default = "default_nav_timeout")]
        timeout: u64,
    },
    /// Take a screenshot
    Screenshot {
        #[serde(default)]
        full_page: bool,
        #[serde(default = "default_image_type")]
        image_type: String,
        #[serde(default)]
        quality: Option<u32>,
        #[serde(default)]
        selector: Option<String>,
    },
    /// Click an element
    Click {
        selector: String,
        #[serde(default = "default_button")]
        button: String,
        #[serde(default = "default_click_count")]
        click_count: u32,
        #[serde(default)]
        delay: u32,
    },
    /// Fill a form field
    Fill {
        selector: String,
        value: String,
        #[serde(default = "default_fill_timeout")]
        timeout: u64,
    },
    /// Execute JavaScript
    Execute {
        script: String,
        #[serde(default)]
        arg: Option<serde_json::Value>,
    },
    /// Get accessibility tree snapshot
    Snapshot,
    /// Wait for a selector
    Wait {
        selector: String,
        #[serde(default = "default_wait_timeout")]
        timeout: u64,
        #[serde(default = "default_wait_state")]
        state: String,
    },
    /// Get page content
    Content,
    /// Get cookies
    GetCookies {
        #[serde(default)]
        urls: Option<Vec<String>>,
    },
    /// Add cookies
    AddCookies { cookies: Vec<serde_json::Value> },
    /// Clear cookies
    ClearCookies,
    /// Set viewport
    SetViewport { width: u32, height: u32 },
    /// Navigate back
    Back,
    /// Navigate forward
    Forward,
    /// Reload page
    Reload {
        #[serde(default = "default_wait_until")]
        wait_until: String,
    },
}

fn default_wait_until() -> String {
    "load".to_string()
}

fn default_nav_timeout() -> u64 {
    30000
}

fn default_image_type() -> String {
    "png".to_string()
}

fn default_button() -> String {
    "left".to_string()
}

fn default_click_count() -> u32 {
    1
}

fn default_fill_timeout() -> u64 {
    30000
}

fn default_wait_timeout() -> u64 {
    5000
}

fn default_wait_state() -> String {
    "visible".to_string()
}

/// Result of a browser action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserResult {
    /// Request ID from the VM
    pub request_id: String,
    /// Whether the action succeeded
    pub success: bool,
    /// Result data (varies by action)
    pub data: Option<serde_json::Value>,
    /// Error message (if failed)
    pub error: Option<String>,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Output chunk for streaming execution output
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputChunk {
    /// Standard output data
    Stdout { data: String },
    /// Standard error data
    Stderr { data: String },
    /// Return value
    Result { value: serde_json::Value },
    /// Execution completed
    Done { exit_code: i32, duration_ms: u64 },
    /// Error occurred
    Error { message: String },
    /// Heartbeat (for long-running executions)
    Heartbeat { elapsed_ms: u64 },
}

/// Execution status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionStatus {
    /// Execution is pending
    Pending,
    /// Execution is running
    Running,
    /// Execution completed successfully
    Completed,
    /// Execution failed
    Failed,
    /// Execution was cancelled
    Cancelled,
    /// Execution timed out
    TimedOut,
}

/// VM Executor for running code inside Firecracker VMs
pub struct VmExecutor {
    /// HTTP client for VM communication
    client: reqwest::Client,
    /// Base URL of the VM's API server
    vm_url: String,
    /// Default timeout for operations
    timeout: Duration,
    /// Active executions being tracked
    active_executions: Arc<RwLock<HashMap<String, ExecutionStatus>>>,
    /// Cancellation tokens for active executions
    cancellation_tokens: Arc<RwLock<HashMap<String, tokio::sync::watch::Sender<bool>>>>,
}

impl VmExecutor {
    /// Create a new VM executor for a specific VM instance
    pub fn new(instance: &VmInstance, timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(5))
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .expect("Failed to create HTTP client");

        let vm_url = instance.http_url();

        Self {
            client,
            vm_url,
            timeout,
            active_executions: Arc::new(RwLock::new(HashMap::new())),
            cancellation_tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create an executor with a custom HTTP client
    pub fn with_client(vm_url: String, client: reqwest::Client, timeout: Duration) -> Self {
        Self {
            client,
            vm_url,
            timeout,
            active_executions: Arc::new(RwLock::new(HashMap::new())),
            cancellation_tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Execute Python code in the VM
    #[instrument(skip(self, code, context), fields(vm_url = %self.vm_url))]
    pub async fn execute_python(
        &self,
        code: &str,
        context: ExecutionContext,
    ) -> Result<ExecutionResult> {
        let execution_id = Uuid::new_v4().to_string();
        let url = format!("{}/api/execute", self.vm_url);
        let start = std::time::Instant::now();

        info!(
            execution_id = %execution_id,
            code_length = code.len(),
            "Executing Python code"
        );

        // Track this execution
        {
            let mut executions = self.active_executions.write().await;
            executions.insert(execution_id.clone(), ExecutionStatus::Running);
        }

        // Create cancellation token
        let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.insert(execution_id.clone(), cancel_tx);
        }

        #[derive(Serialize)]
        struct ExecuteRequest<'a> {
            execution_id: &'a str,
            code: &'a str,
            #[serde(flatten)]
            context: &'a ExecutionContext,
        }

        #[derive(Deserialize)]
        struct ExecuteResponse {
            success: bool,
            stdout: Option<String>,
            stderr: Option<String>,
            return_value: Option<serde_json::Value>,
            exit_code: Option<i32>,
            error: Option<String>,
            #[serde(default)]
            captured_vars: HashMap<String, serde_json::Value>,
        }

        let timeout_duration = Duration::from_millis(context.timeout_ms);

        let result = tokio::select! {
            _ = cancel_rx.changed() => {
                Err(Error::ExecutionFailed("Execution cancelled".to_string()))
            }
            result = async {
                tokio::time::timeout(
                    timeout_duration,
                    self.client
                        .post(&url)
                        .json(&ExecuteRequest {
                            execution_id: &execution_id,
                            code,
                            context: &context,
                        })
                        .send()
                ).await
            } => {
                match result {
                    Ok(Ok(response)) => {
                        if !response.status().is_success() {
                            let status = response.status();
                            let body = response.text().await.unwrap_or_default();
                            Err(Error::ExecutionFailed(format!(
                                "VM execution failed with status {}: {}",
                                status, body
                            )))
                        } else {
                            let resp: ExecuteResponse = response.json().await
                                .map_err(|e| Error::Internal(format!("Failed to parse response: {}", e)))?;

                            let duration_ms = start.elapsed().as_millis() as u64;

                            Ok(ExecutionResult {
                                execution_id: execution_id.clone(),
                                success: resp.success,
                                stdout: resp.stdout.unwrap_or_default(),
                                stderr: resp.stderr.unwrap_or_default(),
                                return_value: resp.return_value,
                                duration_ms,
                                exit_code: resp.exit_code.unwrap_or(if resp.success { 0 } else { 1 }),
                                error: resp.error,
                                captured_vars: resp.captured_vars,
                            })
                        }
                    }
                    Ok(Err(e)) => Err(Error::Http(e.to_string())),
                    Err(_) => Err(Error::Timeout(format!(
                        "Execution timed out after {}ms",
                        context.timeout_ms
                    ))),
                }
            }
        };

        // Update execution status
        {
            let mut executions = self.active_executions.write().await;
            match &result {
                Ok(_) => {
                    executions.insert(execution_id.clone(), ExecutionStatus::Completed);
                }
                Err(Error::Timeout(_)) => {
                    executions.insert(execution_id.clone(), ExecutionStatus::TimedOut);
                }
                Err(_) => {
                    executions.insert(execution_id.clone(), ExecutionStatus::Failed);
                }
            }
        }

        // Remove cancellation token
        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.remove(&execution_id);
        }

        result
    }

    /// Execute a browser action in the VM
    #[instrument(skip(self, action), fields(vm_url = %self.vm_url))]
    pub async fn execute_browser(&self, action: BrowserAction) -> Result<BrowserResult> {
        let start = std::time::Instant::now();
        let request_id = Uuid::new_v4().to_string();

        let (endpoint, params) = match &action {
            BrowserAction::Navigate {
                url,
                wait_until,
                timeout,
            } => (
                "navigate",
                serde_json::json!({
                    "url": url,
                    "wait_until": wait_until,
                    "timeout": timeout
                }),
            ),
            BrowserAction::Screenshot {
                full_page,
                image_type,
                quality,
                selector,
            } => (
                "screenshot",
                serde_json::json!({
                    "full_page": full_page,
                    "type": image_type,
                    "quality": quality,
                    "selector": selector
                }),
            ),
            BrowserAction::Click {
                selector,
                button,
                click_count,
                delay,
            } => (
                "click",
                serde_json::json!({
                    "selector": selector,
                    "button": button,
                    "click_count": click_count,
                    "delay": delay
                }),
            ),
            BrowserAction::Fill {
                selector,
                value,
                timeout,
            } => (
                "fill",
                serde_json::json!({
                    "selector": selector,
                    "value": value,
                    "timeout": timeout
                }),
            ),
            BrowserAction::Execute { script, arg } => (
                "execute",
                serde_json::json!({
                    "script": script,
                    "arg": arg
                }),
            ),
            BrowserAction::Snapshot => ("snapshot", serde_json::json!({})),
            BrowserAction::Wait {
                selector,
                timeout,
                state,
            } => (
                "wait",
                serde_json::json!({
                    "selector": selector,
                    "timeout": timeout,
                    "state": state
                }),
            ),
            BrowserAction::Content => ("content", serde_json::json!({})),
            BrowserAction::GetCookies { urls } => (
                "cookies/get",
                serde_json::json!({
                    "urls": urls
                }),
            ),
            BrowserAction::AddCookies { cookies } => (
                "cookies/add",
                serde_json::json!({
                    "cookies": cookies
                }),
            ),
            BrowserAction::ClearCookies => ("cookies/clear", serde_json::json!({})),
            BrowserAction::SetViewport { width, height } => (
                "viewport",
                serde_json::json!({
                    "width": width,
                    "height": height
                }),
            ),
            BrowserAction::Back => ("back", serde_json::json!({})),
            BrowserAction::Forward => ("forward", serde_json::json!({})),
            BrowserAction::Reload { wait_until } => (
                "reload",
                serde_json::json!({
                    "wait_until": wait_until
                }),
            ),
        };

        let url = format!("{}/api/browser/{}", self.vm_url, endpoint);

        debug!(
            request_id = %request_id,
            endpoint = endpoint,
            "Executing browser action"
        );

        #[derive(Serialize)]
        struct BrowserRequest {
            request_id: String,
            command: String,
            params: serde_json::Value,
        }

        #[derive(Deserialize)]
        struct BrowserResponse {
            request_id: String,
            success: bool,
            data: Option<serde_json::Value>,
            error: Option<String>,
        }

        let response = tokio::time::timeout(
            self.timeout,
            self.client
                .post(&url)
                .json(&BrowserRequest {
                    request_id: request_id.clone(),
                    command: endpoint.to_string(),
                    params,
                })
                .send(),
        )
        .await
        .map_err(|_| Error::Timeout(format!("Browser action timed out after {:?}", self.timeout)))?
        .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::ExecutionFailed(format!(
                "Browser action failed with status {}: {}",
                status, body
            )));
        }

        let resp: BrowserResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse browser response: {}", e)))?;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(BrowserResult {
            request_id: resp.request_id,
            success: resp.success,
            data: resp.data,
            error: resp.error,
            duration_ms,
        })
    }

    /// Get execution status
    pub async fn get_status(&self, execution_id: &str) -> Option<ExecutionStatus> {
        let executions = self.active_executions.read().await;
        executions.get(execution_id).cloned()
    }

    /// Cancel an active execution
    #[instrument(skip(self), fields(vm_url = %self.vm_url))]
    pub async fn cancel(&self, execution_id: &str) -> Result<bool> {
        info!(execution_id = %execution_id, "Cancelling execution");

        // Send cancellation signal
        let cancelled = {
            let tokens = self.cancellation_tokens.read().await;
            if let Some(tx) = tokens.get(execution_id) {
                let _ = tx.send(true);
                true
            } else {
                false
            }
        };

        if cancelled {
            // Update status
            let mut executions = self.active_executions.write().await;
            executions.insert(execution_id.to_string(), ExecutionStatus::Cancelled);
        }

        Ok(cancelled)
    }

    /// Stream output from an execution (via polling)
    pub fn stream_output(
        &self,
        execution_id: &str,
    ) -> Pin<Box<dyn Stream<Item = OutputChunk> + Send + '_>> {
        let execution_id = execution_id.to_string();
        let vm_url = self.vm_url.clone();
        let client = self.client.clone();
        let active_executions = Arc::clone(&self.active_executions);

        Box::pin(stream! {
            let start = std::time::Instant::now();
            let poll_interval = Duration::from_millis(100);
            let mut last_stdout_len = 0usize;
            let mut last_stderr_len = 0usize;

            loop {
                // Check execution status
                let status = {
                    let executions = active_executions.read().await;
                    executions.get(&execution_id).cloned()
                };

                let url = format!("{}/api/execute/output/{}", vm_url, execution_id);

                match client.get(&url).send().await {
                    Ok(response) if response.status().is_success() => {
                        #[derive(Deserialize)]
                        struct OutputResponse {
                            stdout: String,
                            stderr: String,
                            return_value: Option<serde_json::Value>,
                            exit_code: Option<i32>,
                            completed: bool,
                        }

                        if let Ok(output) = response.json::<OutputResponse>().await {
                            // Yield new stdout
                            if output.stdout.len() > last_stdout_len {
                                let new_stdout = &output.stdout[last_stdout_len..];
                                yield OutputChunk::Stdout { data: new_stdout.to_string() };
                                last_stdout_len = output.stdout.len();
                            }

                            // Yield new stderr
                            if output.stderr.len() > last_stderr_len {
                                let new_stderr = &output.stderr[last_stderr_len..];
                                yield OutputChunk::Stderr { data: new_stderr.to_string() };
                                last_stderr_len = output.stderr.len();
                            }

                            // Check if completed
                            if output.completed {
                                if let Some(value) = output.return_value {
                                    yield OutputChunk::Result { value };
                                }
                                yield OutputChunk::Done {
                                    exit_code: output.exit_code.unwrap_or(0),
                                    duration_ms: start.elapsed().as_millis() as u64,
                                };
                                break;
                            }
                        }
                    }
                    Ok(response) => {
                        let error_msg = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
                        yield OutputChunk::Error { message: error_msg };
                        break;
                    }
                    Err(e) => {
                        // Network error - check if execution is still running
                        if matches!(status, Some(ExecutionStatus::Completed) | Some(ExecutionStatus::Failed) | Some(ExecutionStatus::Cancelled) | Some(ExecutionStatus::TimedOut)) {
                            yield OutputChunk::Error { message: format!("Connection error: {}", e) };
                            break;
                        }
                    }
                }

                // Yield heartbeat periodically
                let elapsed = start.elapsed().as_millis() as u64;
                if elapsed % 5000 < 100 {
                    yield OutputChunk::Heartbeat { elapsed_ms: elapsed };
                }

                // Check for completion/cancellation
                match status {
                    Some(ExecutionStatus::Completed) => {
                        yield OutputChunk::Done {
                            exit_code: 0,
                            duration_ms: start.elapsed().as_millis() as u64,
                        };
                        break;
                    }
                    Some(ExecutionStatus::Failed) => {
                        yield OutputChunk::Error { message: "Execution failed".to_string() };
                        break;
                    }
                    Some(ExecutionStatus::Cancelled) => {
                        yield OutputChunk::Error { message: "Execution cancelled".to_string() };
                        break;
                    }
                    Some(ExecutionStatus::TimedOut) => {
                        yield OutputChunk::Error { message: "Execution timed out".to_string() };
                        break;
                    }
                    _ => {}
                }

                tokio::time::sleep(poll_interval).await;
            }
        })
    }

    /// Get output from an execution (non-streaming)
    #[instrument(skip(self), fields(vm_url = %self.vm_url))]
    pub async fn get_output(&self, execution_id: &str) -> Result<ExecutionResult> {
        let url = format!("{}/api/execute/output/{}", self.vm_url, execution_id);

        #[derive(Deserialize)]
        struct OutputResponse {
            stdout: String,
            stderr: String,
            return_value: Option<serde_json::Value>,
            exit_code: Option<i32>,
            completed: bool,
            error: Option<String>,
            duration_ms: Option<u64>,
            #[serde(default)]
            captured_vars: HashMap<String, serde_json::Value>,
        }

        let response = tokio::time::timeout(self.timeout, self.client.get(&url).send())
            .await
            .map_err(|_| Error::Timeout("Get output timed out".to_string()))?
            .map_err(|e| Error::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::ExecutionFailed(format!(
                "Get output failed with status {}: {}",
                status, body
            )));
        }

        let output: OutputResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse output response: {}", e)))?;

        Ok(ExecutionResult {
            execution_id: execution_id.to_string(),
            success: output.completed && output.error.is_none(),
            stdout: output.stdout,
            stderr: output.stderr,
            return_value: output.return_value,
            duration_ms: output.duration_ms.unwrap_or(0),
            exit_code: output
                .exit_code
                .unwrap_or(if output.completed { 0 } else { 1 }),
            error: output.error,
            captured_vars: output.captured_vars,
        })
    }

    /// Check if the VM's API server is healthy
    #[instrument(skip(self), fields(vm_url = %self.vm_url))]
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/api/health", self.vm_url);

        let response = tokio::time::timeout(Duration::from_secs(5), self.client.get(&url).send())
            .await
            .map_err(|_| Error::Timeout("Health check timed out".to_string()))?
            .map_err(|e| Error::Http(e.to_string()))?;

        Ok(response.status().is_success())
    }

    /// Get the VM URL
    pub fn vm_url(&self) -> &str {
        &self.vm_url
    }

    /// Get current timeout
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Get count of active executions
    pub async fn active_execution_count(&self) -> usize {
        let executions = self.active_executions.read().await;
        executions
            .values()
            .filter(|s| matches!(s, ExecutionStatus::Running | ExecutionStatus::Pending))
            .count()
    }

    /// Clean up completed executions older than the specified duration
    pub async fn cleanup_old_executions(&self, _max_age: Duration) {
        // Note: In production, we'd track timestamps for each execution
        // For now, just clear completed/failed/cancelled executions
        let mut executions = self.active_executions.write().await;
        executions.retain(|_, status| {
            matches!(status, ExecutionStatus::Running | ExecutionStatus::Pending)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn create_test_instance() -> VmInstance {
        VmInstance {
            id: "test-vm".to_string(),
            ip: Ipv4Addr::new(172, 16, 0, 2),
            port: 8080,
            vnc_port: 5900,
            status: crate::vm::manager::VmStatus::Running,
            created_at: std::time::Instant::now(),
            index: 0,
        }
    }

    #[test]
    fn test_executor_creation() {
        let instance = create_test_instance();
        let executor = VmExecutor::new(&instance, Duration::from_secs(30));

        assert_eq!(executor.vm_url(), "http://172.16.0.2:8080");
        assert_eq!(executor.timeout(), Duration::from_secs(30));
    }

    #[test]
    fn test_execution_context_defaults() {
        let context = ExecutionContext::default();

        assert_eq!(context.timeout_ms, 30000);
        assert!(context.capture_output);
        assert!(!context.sandbox_mode);
        assert!(context.env_vars.is_empty());
        assert!(context.working_dir.is_none());
    }

    #[test]
    fn test_browser_action_serialization() {
        let action = BrowserAction::Navigate {
            url: "https://example.com".to_string(),
            wait_until: "load".to_string(),
            timeout: 30000,
        };

        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("navigate"));
        assert!(json.contains("example.com"));
    }

    #[test]
    fn test_execution_result_serialization() {
        let result = ExecutionResult {
            execution_id: "exec-123".to_string(),
            success: true,
            stdout: "Hello, World!".to_string(),
            stderr: "".to_string(),
            return_value: Some(serde_json::json!(42)),
            duration_ms: 100,
            exit_code: 0,
            error: None,
            captured_vars: HashMap::new(),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("exec-123"));
        assert!(json.contains("Hello, World!"));
        assert!(json.contains("42"));
    }

    #[test]
    fn test_output_chunk_variants() {
        let stdout = OutputChunk::Stdout {
            data: "test output".to_string(),
        };
        let json = serde_json::to_string(&stdout).unwrap();
        assert!(json.contains("stdout"));
        assert!(json.contains("test output"));

        let done = OutputChunk::Done {
            exit_code: 0,
            duration_ms: 1000,
        };
        let json = serde_json::to_string(&done).unwrap();
        assert!(json.contains("done"));
        assert!(json.contains("exit_code"));
    }

    #[tokio::test]
    async fn test_execution_status_tracking() {
        let instance = create_test_instance();
        let executor = VmExecutor::new(&instance, Duration::from_secs(30));

        // Initially no executions
        assert_eq!(executor.active_execution_count().await, 0);

        // Status should return None for unknown execution
        assert!(executor.get_status("unknown").await.is_none());
    }

    #[test]
    fn test_browser_action_all_variants() {
        // Test all browser action variants can be created
        let actions: Vec<BrowserAction> = vec![
            BrowserAction::Navigate {
                url: "https://example.com".to_string(),
                wait_until: "load".to_string(),
                timeout: 30000,
            },
            BrowserAction::Screenshot {
                full_page: true,
                image_type: "png".to_string(),
                quality: None,
                selector: None,
            },
            BrowserAction::Click {
                selector: "#button".to_string(),
                button: "left".to_string(),
                click_count: 1,
                delay: 0,
            },
            BrowserAction::Fill {
                selector: "input".to_string(),
                value: "test".to_string(),
                timeout: 5000,
            },
            BrowserAction::Execute {
                script: "return 1".to_string(),
                arg: None,
            },
            BrowserAction::Snapshot,
            BrowserAction::Wait {
                selector: "#element".to_string(),
                timeout: 5000,
                state: "visible".to_string(),
            },
            BrowserAction::Content,
            BrowserAction::GetCookies { urls: None },
            BrowserAction::AddCookies {
                cookies: vec![serde_json::json!({"name": "test", "value": "value"})],
            },
            BrowserAction::ClearCookies,
            BrowserAction::SetViewport {
                width: 1920,
                height: 1080,
            },
            BrowserAction::Back,
            BrowserAction::Forward,
            BrowserAction::Reload {
                wait_until: "load".to_string(),
            },
        ];

        for action in actions {
            let json = serde_json::to_string(&action).unwrap();
            assert!(!json.is_empty());
        }
    }
}
