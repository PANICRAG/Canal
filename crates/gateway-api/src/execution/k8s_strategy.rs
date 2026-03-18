//! K8s Execution Strategy
//!
//! Routes code execution to Kubernetes worker pods via gRPC.
//! Uses ContainerOrchestrator to manage pod lifecycle and
//! TaskWorkerClient for actual code execution.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, instrument};
use uuid::Uuid;

use gateway_core::executor::result::CodeActResult;
use gateway_core::executor::router::{AvailableResources, CodeExecutionRequest, ExecutionStrategy};
use gateway_orchestrator::{
    proto::worker::{code_output, CodeRequest},
    ContainerOrchestrator, TaskWorkerClient,
};
use gateway_tools::error::{ServiceError as Error, ServiceResult as Result};

/// Information about an active K8s execution
#[derive(Debug, Clone)]
struct ExecutionInfo {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    started_at: DateTime<Utc>,
    cpu_reserved: f64,
    memory_reserved: u64,
    #[allow(dead_code)]
    container_id: Option<Uuid>,
}

/// K8s-based execution strategy that routes code to worker pods
pub struct K8sExecutionStrategy {
    /// Container orchestrator for pod lifecycle management
    orchestrator: Arc<ContainerOrchestrator>,
    /// Active executions tracking
    active: Arc<RwLock<HashMap<String, ExecutionInfo>>>,
    /// Default user ID for anonymous executions
    default_user_id: Uuid,
    /// Maximum concurrent executions
    max_concurrent: usize,
    /// Total CPU cores available across the cluster
    total_cpu: f64,
    /// Total memory in MB available across the cluster
    total_memory_mb: u64,
    /// Cached gRPC clients per container
    clients: Arc<RwLock<HashMap<Uuid, TaskWorkerClient>>>,
}

impl K8sExecutionStrategy {
    /// Create a new K8s execution strategy
    pub fn new(orchestrator: Arc<ContainerOrchestrator>, default_user_id: Uuid) -> Self {
        Self {
            orchestrator,
            active: Arc::new(RwLock::new(HashMap::new())),
            default_user_id,
            max_concurrent: 50,
            total_cpu: 100.0,
            total_memory_mb: 204800,
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set maximum concurrent executions
    #[allow(dead_code)]
    pub fn with_max_concurrent(mut self, max: usize) -> Self {
        self.max_concurrent = max;
        self
    }

    /// Set total cluster resources
    #[allow(dead_code)]
    pub fn with_resources(mut self, cpu: f64, memory_mb: u64) -> Self {
        self.total_cpu = cpu;
        self.total_memory_mb = memory_mb;
        self
    }

    /// Get or create a gRPC client for a container
    async fn get_or_create_client(&self, session_id: Uuid) -> Result<(Uuid, TaskWorkerClient)> {
        // Get or create container for this session
        let container = self
            .orchestrator
            .get_or_create_for_session(session_id, self.default_user_id)
            .await
            .map_err(|e| Error::Internal(format!("Failed to get container: {}", e)))?;

        let container_id = container.id;

        // Check if we already have a client
        {
            let clients = self.clients.read().await;
            if let Some(client) = clients.get(&container_id) {
                return Ok((container_id, client.clone()));
            }
        }

        // Wait for container to be ready
        let container = self
            .orchestrator
            .wait_for_ready(&container_id)
            .await
            .map_err(|e| Error::Internal(format!("Container not ready: {}", e)))?;

        let endpoint = container
            .grpc_endpoint
            .as_ref()
            .ok_or_else(|| Error::Internal("Container has no gRPC endpoint".into()))?;

        // Create gRPC client
        let client = TaskWorkerClient::connect(endpoint)
            .await
            .map_err(|e| Error::Internal(format!("Failed to connect to worker: {}", e)))?;

        // Cache the client
        {
            let mut clients = self.clients.write().await;
            clients.insert(container_id, client.clone());
        }

        Ok((container_id, client))
    }

    /// Execute code via gRPC worker
    async fn do_execute(&self, request: &CodeExecutionRequest) -> Result<CodeActResult> {
        let session_id = request
            .session_id
            .as_ref()
            .and_then(|s| Uuid::parse_str(s).ok())
            .unwrap_or_else(Uuid::new_v4);

        let (container_id, client) = self.get_or_create_client(session_id).await?;

        debug!(
            container_id = %container_id,
            language = %request.language,
            code_len = request.code.len(),
            "Executing code via K8s worker"
        );

        // Convert timeout from ms to seconds for the proto
        let timeout_seconds = (request.timeout_ms / 1000).max(1) as i32;

        // Convert to gRPC request matching proto definition:
        //   message CodeRequest {
        //       string session_id = 1;
        //       string code = 2;
        //       string language = 3;
        //       int32 timeout_seconds = 4;
        //       map<string, string> env = 5;
        //       string working_dir = 6;
        //   }
        let grpc_request = CodeRequest {
            session_id: session_id.to_string(),
            code: request.code.clone(),
            language: request.language.clone(),
            timeout_seconds,
            env: request.env.clone(),
            working_dir: request.working_dir.clone().unwrap_or_default(),
        };

        // Execute code via streaming gRPC
        let mut stream = client
            .execute_code(grpc_request)
            .await
            .map_err(|e| Error::Internal(format!("gRPC execution failed: {}", e)))?;

        // Collect stream output
        // CodeOutput uses oneof:
        //   oneof output { string stdout = 1; string stderr = 2; CodeComplete complete = 3; CodeError error = 4; }
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = 0i32;
        let mut success = true;

        while let Some(item) = stream.next().await {
            match item {
                Ok(output) => match output.output {
                    Some(code_output::Output::Stdout(data)) => {
                        stdout.push_str(&data);
                    }
                    Some(code_output::Output::Stderr(data)) => {
                        stderr.push_str(&data);
                    }
                    Some(code_output::Output::Complete(complete)) => {
                        exit_code = complete.exit_code;
                        if exit_code != 0 {
                            success = false;
                        }
                    }
                    Some(code_output::Output::Error(err)) => {
                        stderr.push_str(&err.message);
                        success = false;
                    }
                    None => {
                        // Empty output frame, skip
                    }
                },
                Err(status) => {
                    error!(status = %status, "gRPC stream error");
                    return Err(Error::Internal(format!("gRPC stream error: {}", status)));
                }
            }
        }

        // Update orchestrator activity
        let _ = self.orchestrator.update_activity(&container_id).await;

        // Build CodeActResult
        let output_text = if stdout.is_empty() && !stderr.is_empty() {
            stderr.clone()
        } else {
            stdout.clone()
        };

        let mut result = if success {
            CodeActResult::success(&request.id, &output_text)
        } else {
            let error = gateway_core::executor::result::CodeActError {
                error_type: gateway_core::executor::result::ErrorType::RuntimeError,
                message: stderr.clone(),
                details: None,
                traceback: None,
                line_number: None,
                column: None,
                code_snippet: None,
            };
            CodeActResult::error(&request.id, error)
        };

        result.raw_stdout = stdout;
        result.raw_stderr = stderr;
        result.exit_code = exit_code;
        result
            .metadata
            .insert("executor".to_string(), serde_json::json!("k8s_worker"));
        result.metadata.insert(
            "container_id".to_string(),
            serde_json::json!(container_id.to_string()),
        );

        Ok(result)
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
impl ExecutionStrategy for K8sExecutionStrategy {
    fn name(&self) -> &str {
        "k8s_worker"
    }

    async fn is_available(&self) -> bool {
        let active = self.active.read().await;
        active.len() < self.max_concurrent
    }

    async fn active_executions(&self) -> usize {
        self.active.read().await.len()
    }

    async fn available_resources(&self) -> AvailableResources {
        let (reserved_cpu, reserved_memory) = self.reserved_resources().await;
        let active_count = self.active.read().await.len();

        AvailableResources {
            cpu_cores: (self.total_cpu - reserved_cpu).max(0.0),
            memory_mb: self.total_memory_mb.saturating_sub(reserved_memory),
            execution_slots: self.max_concurrent.saturating_sub(active_count),
            utilization_percent: (active_count as f64 / self.max_concurrent as f64) * 100.0,
        }
    }

    #[instrument(skip(self, request), fields(request_id = %request.id))]
    async fn execute(&self, request: CodeExecutionRequest) -> Result<CodeActResult> {
        if !self.is_available().await {
            return Err(Error::Internal("K8s executor at capacity".into()));
        }

        let execution_id = request.id.clone();
        let info = ExecutionInfo {
            id: execution_id.clone(),
            started_at: Utc::now(),
            cpu_reserved: request.required_cpu,
            memory_reserved: request.required_memory_mb,
            container_id: None,
        };

        // Register execution
        {
            let mut active = self.active.write().await;
            active.insert(execution_id.clone(), info);
        }

        let start = std::time::Instant::now();

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(request.timeout_ms),
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
            debug!("Cancelled K8s execution: {}", execution_id);
            Ok(())
        } else {
            Err(Error::NotFound(format!(
                "Execution not found: {}",
                execution_id
            )))
        }
    }

    async fn health_check(&self) -> Result<bool> {
        // Try to connect to a worker pod to verify the K8s cluster is healthy
        // For now, we just return true if the orchestrator exists
        Ok(true)
    }
}
