//! Firecracker VM Execution Strategy
//!
//! Routes code execution to Firecracker microVMs via the VM manager.
//! Each execution acquires a VM from the pool, runs the code, and releases it.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, instrument, warn};

use crate::vm::{ExecutionContext, VmExecutor, VmManager};
use gateway_tools::error::{ServiceError as Error, ServiceResult as Result};
use gateway_tools::executor::CodeActResult;
use gateway_tools::executor::{AvailableResources, CodeExecutionRequest, ExecutionStrategy};

/// Information about an active Firecracker execution
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ExecutionInfo {
    id: String,
    started_at: DateTime<Utc>,
    cpu_reserved: f64,
    memory_reserved: u64,
    vm_id: Option<String>,
}

/// Firecracker VM-based execution strategy
pub struct FirecrackerExecutionStrategy {
    /// VM manager for acquiring/releasing VMs
    vm_manager: Arc<VmManager>,
    /// Active executions tracking
    active: Arc<RwLock<HashMap<String, ExecutionInfo>>>,
    /// Maximum concurrent executions (bounded by VM pool size)
    max_concurrent: usize,
    /// Total CPU cores across the VM pool
    total_cpu: f64,
    /// Total memory in MB across the VM pool
    total_memory_mb: u64,
}

impl FirecrackerExecutionStrategy {
    /// Create a new Firecracker execution strategy
    pub fn new(vm_manager: Arc<VmManager>) -> Self {
        Self {
            vm_manager,
            active: Arc::new(RwLock::new(HashMap::new())),
            max_concurrent: 10,
            total_cpu: 20.0,
            total_memory_mb: 20480,
        }
    }

    /// Set maximum concurrent executions
    pub fn with_max_concurrent(mut self, max: usize) -> Self {
        self.max_concurrent = max;
        self
    }

    /// Set total resources
    pub fn with_resources(mut self, cpu: f64, memory_mb: u64) -> Self {
        self.total_cpu = cpu;
        self.total_memory_mb = memory_mb;
        self
    }

    /// Execute code in a Firecracker VM
    async fn do_execute(&self, request: &CodeExecutionRequest) -> Result<CodeActResult> {
        // Acquire a VM from the pool
        let instance = self
            .vm_manager
            .acquire()
            .await
            .map_err(|e| Error::Internal(format!("Failed to acquire VM: {}", e)))?;

        let vm_id = instance.id.clone();
        debug!(
            vm_id = %vm_id,
            language = %request.language,
            code_len = request.code.len(),
            "Executing code in Firecracker VM"
        );

        // Create executor for this VM
        let executor = VmExecutor::new(&instance, Duration::from_millis(request.timeout_ms));

        // Build execution context
        let context = ExecutionContext {
            env_vars: request.env.clone(),
            working_dir: request.working_dir.clone(),
            session_vars: HashMap::new(),
            timeout_ms: request.timeout_ms,
            capture_output: true,
            allowed_imports: None,
            sandbox_mode: false,
        };

        // Execute based on language
        let exec_result = match request.language.as_str() {
            "python" | "python3" => executor.execute_python(&request.code, context).await,
            "bash" | "shell" | "sh" => {
                // For bash, wrap in a python subprocess call or use the executor directly
                let bash_code = format!(
                    "import subprocess; result = subprocess.run({}, shell=True, capture_output=True, text=True); print(result.stdout); import sys; sys.stderr.write(result.stderr); sys.exit(result.returncode)",
                    serde_json::to_string(&request.code).unwrap_or_default()
                );
                executor.execute_python(&bash_code, context).await
            }
            _ => {
                // Default: try Python
                executor.execute_python(&request.code, context).await
            }
        };

        // Release the VM back to the pool
        if let Err(e) = self.vm_manager.release(instance).await {
            warn!(vm_id = %vm_id, error = %e, "Failed to release VM");
        }

        // Convert VM ExecutionResult to CodeActResult
        match exec_result {
            Ok(vm_result) => {
                let output_text = if vm_result.stdout.is_empty() && !vm_result.stderr.is_empty() {
                    vm_result.stderr.clone()
                } else {
                    vm_result.stdout.clone()
                };

                let mut result = if vm_result.success {
                    CodeActResult::success(&request.id, &output_text)
                } else {
                    let error_msg = vm_result.error.unwrap_or_else(|| vm_result.stderr.clone());
                    let error = super::result::CodeActError {
                        error_type: super::result::ErrorType::RuntimeError,
                        message: error_msg,
                        details: None,
                        traceback: None,
                        line_number: None,
                        column: None,
                        code_snippet: None,
                    };
                    CodeActResult::error(&request.id, error)
                };

                result.raw_stdout = vm_result.stdout;
                result.raw_stderr = vm_result.stderr;
                result.exit_code = vm_result.exit_code;
                result.return_value = vm_result.return_value;
                result.variables = vm_result.captured_vars;
                result
                    .metadata
                    .insert("executor".to_string(), serde_json::json!("firecracker_vm"));
                result
                    .metadata
                    .insert("vm_id".to_string(), serde_json::json!(vm_id));
                result.timing.execution_ms = vm_result.duration_ms;

                Ok(result)
            }
            Err(e) => {
                let error = super::result::CodeActError {
                    error_type: super::result::ErrorType::RuntimeError,
                    message: format!("VM execution failed: {}", e),
                    details: None,
                    traceback: None,
                    line_number: None,
                    column: None,
                    code_snippet: None,
                };
                Ok(CodeActResult::error(&request.id, error))
            }
        }
    }

    /// Get reserved resources
    async fn reserved_resources(&self) -> (f64, u64) {
        let active = self.active.read().await;
        let cpu: f64 = active.values().map(|e| e.cpu_reserved).sum();
        let memory: u64 = active.values().map(|e| e.memory_reserved).sum();
        (cpu, memory)
    }
}

#[async_trait]
impl ExecutionStrategy for FirecrackerExecutionStrategy {
    fn name(&self) -> &str {
        "firecracker_vm"
    }

    async fn is_available(&self) -> bool {
        let active = self.active.read().await;
        if active.len() >= self.max_concurrent {
            return false;
        }
        // Check if the VM manager has available VMs
        let stats = self.vm_manager.stats().await;
        stats.available > 0
    }

    async fn active_executions(&self) -> usize {
        self.active.read().await.len()
    }

    async fn available_resources(&self) -> AvailableResources {
        let (reserved_cpu, reserved_memory) = self.reserved_resources().await;
        let _active_count = self.active.read().await.len();
        let stats = self.vm_manager.stats().await;
        let total = stats.available + stats.in_use;

        AvailableResources {
            cpu_cores: (self.total_cpu - reserved_cpu).max(0.0),
            memory_mb: self.total_memory_mb.saturating_sub(reserved_memory),
            execution_slots: stats.available,
            utilization_percent: if total > 0 {
                (stats.in_use as f64 / total as f64) * 100.0
            } else {
                100.0
            },
        }
    }

    #[instrument(skip(self, request), fields(request_id = %request.id))]
    async fn execute(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        if !self.is_available().await {
            return Err(Error::Internal(
                "Firecracker executor at capacity or no VMs available".into(),
            ));
        }

        let execution_id = request.id.clone();
        let info = ExecutionInfo {
            id: execution_id.clone(),
            started_at: Utc::now(),
            cpu_reserved: request.required_cpu,
            memory_reserved: request.required_memory_mb,
            vm_id: None,
        };

        // Register execution
        {
            let mut active = self.active.write().await;
            active.insert(execution_id.clone(), info);
        }

        let start = Instant::now();

        let result = tokio::time::timeout(
            Duration::from_millis(request.timeout_ms),
            self.do_execute(&request),
        )
        .await;

        // Unregister execution
        {
            let mut active = self.active.write().await;
            active.remove(&execution_id);
        }

        match result {
            Ok(Ok(mut result)) => {
                result.timing.total_ms = start.elapsed().as_millis() as u64;
                Ok(result)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Ok(CodeActResult::timeout(&execution_id, request.timeout_ms)),
        }
    }

    async fn cancel(&self, execution_id: &str) -> Result<()> {
        let mut active = self.active.write().await;
        if active.remove(execution_id).is_some() {
            debug!("Cancelled Firecracker execution: {}", execution_id);
            Ok(())
        } else {
            Err(Error::NotFound(format!(
                "Execution not found: {}",
                execution_id
            )))
        }
    }

    async fn health_check(&self) -> Result<bool> {
        let stats = self.vm_manager.stats().await;
        let total = stats.available + stats.in_use;
        Ok(total > 0)
    }
}
