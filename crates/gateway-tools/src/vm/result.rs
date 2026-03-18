//! VM Execution Result Handling
//!
//! Provides comprehensive result handling for VM code execution,
//! including result aggregation, streaming support, and artifact collection.
//!
//! # Architecture
//!
//! ```text
//! +-------------------+     +-------------------+
//! |   VmExecutor      |     |  ResultCollector  |
//! |   (execution)     |---->|  (aggregation)    |
//! +-------------------+     +-------------------+
//!                                    |
//!                                    v
//!                           +-------------------+
//!                           | VmExecutionResult |
//!                           |   - stdout/stderr |
//!                           |   - return_value  |
//!                           |   - artifacts     |
//!                           |   - variables     |
//!                           +-------------------+
//! ```

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, instrument, warn};
use uuid::Uuid;

use crate::error::{ServiceError as Error, ServiceResult as Result};

// ============================================================================
// Core Result Types
// ============================================================================

/// Comprehensive execution result from VM code execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmExecutionResult {
    /// Unique execution identifier
    pub execution_id: String,
    /// Current execution status
    pub status: ExecutionStatus,
    /// Captured standard output
    pub stdout: String,
    /// Captured standard error
    pub stderr: String,
    /// Return value from execution (if any)
    pub return_value: Option<serde_json::Value>,
    /// Process exit code (0 = success)
    pub exit_code: i32,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
    /// Collected artifacts (screenshots, generated files, etc.)
    pub artifacts: Vec<Artifact>,
    /// Captured variables from execution context
    pub variables: HashMap<String, serde_json::Value>,
    /// Timestamp when execution started
    pub started_at: DateTime<Utc>,
    /// Timestamp when execution completed (if completed)
    pub completed_at: Option<DateTime<Utc>>,
    /// Error details (if failed)
    pub error_details: Option<ErrorDetails>,
    /// Execution metadata
    pub metadata: ExecutionMetadata,
}

impl VmExecutionResult {
    /// Create a new result for a starting execution
    pub fn new(execution_id: impl Into<String>) -> Self {
        Self {
            execution_id: execution_id.into(),
            status: ExecutionStatus::Running,
            stdout: String::new(),
            stderr: String::new(),
            return_value: None,
            exit_code: -1,
            duration_ms: 0,
            artifacts: Vec::new(),
            variables: HashMap::new(),
            started_at: Utc::now(),
            completed_at: None,
            error_details: None,
            metadata: ExecutionMetadata::default(),
        }
    }

    /// Create a completed result
    pub fn completed(
        execution_id: impl Into<String>,
        stdout: String,
        stderr: String,
        exit_code: i32,
        duration_ms: u64,
    ) -> Self {
        Self {
            execution_id: execution_id.into(),
            status: if exit_code == 0 {
                ExecutionStatus::Completed
            } else {
                ExecutionStatus::Failed
            },
            stdout,
            stderr,
            return_value: None,
            exit_code,
            duration_ms,
            artifacts: Vec::new(),
            variables: HashMap::new(),
            started_at: Utc::now() - chrono::Duration::milliseconds(duration_ms as i64),
            completed_at: Some(Utc::now()),
            error_details: None,
            metadata: ExecutionMetadata::default(),
        }
    }

    /// Create a failed result with error details
    pub fn failed(execution_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            execution_id: execution_id.into(),
            status: ExecutionStatus::Failed,
            stdout: String::new(),
            stderr: String::new(),
            return_value: None,
            exit_code: 1,
            duration_ms: 0,
            artifacts: Vec::new(),
            variables: HashMap::new(),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            error_details: Some(ErrorDetails {
                message: error.into(),
                error_type: ErrorType::ExecutionError,
                code: None,
                stack_trace: None,
            }),
            metadata: ExecutionMetadata::default(),
        }
    }

    /// Create a timeout result
    pub fn timeout(execution_id: impl Into<String>, timeout_ms: u64) -> Self {
        Self {
            execution_id: execution_id.into(),
            status: ExecutionStatus::Timeout,
            stdout: String::new(),
            stderr: String::new(),
            return_value: None,
            exit_code: 124, // Standard timeout exit code
            duration_ms: timeout_ms,
            artifacts: Vec::new(),
            variables: HashMap::new(),
            started_at: Utc::now() - chrono::Duration::milliseconds(timeout_ms as i64),
            completed_at: Some(Utc::now()),
            error_details: Some(ErrorDetails {
                message: format!("Execution timed out after {}ms", timeout_ms),
                error_type: ErrorType::Timeout,
                code: Some("TIMEOUT".to_string()),
                stack_trace: None,
            }),
            metadata: ExecutionMetadata::default(),
        }
    }

    /// Create a cancelled result
    pub fn cancelled(execution_id: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            execution_id: execution_id.into(),
            status: ExecutionStatus::Cancelled,
            stdout: String::new(),
            stderr: String::new(),
            return_value: None,
            exit_code: 130, // Standard SIGINT exit code
            duration_ms,
            artifacts: Vec::new(),
            variables: HashMap::new(),
            started_at: Utc::now() - chrono::Duration::milliseconds(duration_ms as i64),
            completed_at: Some(Utc::now()),
            error_details: Some(ErrorDetails {
                message: "Execution was cancelled".to_string(),
                error_type: ErrorType::Cancelled,
                code: Some("CANCELLED".to_string()),
                stack_trace: None,
            }),
            metadata: ExecutionMetadata::default(),
        }
    }

    /// Check if execution completed successfully
    pub fn is_success(&self) -> bool {
        matches!(self.status, ExecutionStatus::Completed) && self.exit_code == 0
    }

    /// Check if execution is still running
    pub fn is_running(&self) -> bool {
        matches!(self.status, ExecutionStatus::Running)
    }

    /// Check if execution is terminal (won't change)
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            ExecutionStatus::Completed
                | ExecutionStatus::Failed
                | ExecutionStatus::Timeout
                | ExecutionStatus::Cancelled
        )
    }

    /// Add an artifact to the result
    pub fn add_artifact(&mut self, artifact: Artifact) {
        self.artifacts.push(artifact);
    }

    /// Add a variable to the result
    pub fn add_variable(&mut self, name: impl Into<String>, value: serde_json::Value) {
        self.variables.insert(name.into(), value);
    }

    /// Set the return value
    pub fn set_return_value(&mut self, value: serde_json::Value) {
        self.return_value = Some(value);
    }

    /// Mark as completed
    pub fn mark_completed(&mut self, exit_code: i32, duration_ms: u64) {
        self.status = if exit_code == 0 {
            ExecutionStatus::Completed
        } else {
            ExecutionStatus::Failed
        };
        self.exit_code = exit_code;
        self.duration_ms = duration_ms;
        self.completed_at = Some(Utc::now());
    }

    /// Get total artifact size in bytes
    pub fn total_artifact_size(&self) -> u64 {
        self.artifacts.iter().map(|a| a.size).sum()
    }

    /// Convert to API response format
    pub fn to_api_response(&self) -> VmExecutionResponse {
        VmExecutionResponse {
            execution_id: self.execution_id.clone(),
            status: self.status.clone(),
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            return_value: self.return_value.clone(),
            exit_code: self.exit_code,
            duration_ms: self.duration_ms,
            artifacts: self.artifacts.iter().map(|a| a.to_api_artifact()).collect(),
            variable_names: self.variables.keys().cloned().collect(),
            error: self.error_details.as_ref().map(|e| e.message.clone()),
        }
    }
}

/// Execution status enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionStatus {
    /// Execution is pending (queued)
    Pending,
    /// Execution is currently running
    Running,
    /// Execution completed successfully
    Completed,
    /// Execution failed with error
    Failed,
    /// Execution timed out
    Timeout,
    /// Execution was cancelled
    Cancelled,
}

impl ExecutionStatus {
    /// Check if this status indicates a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ExecutionStatus::Completed
                | ExecutionStatus::Failed
                | ExecutionStatus::Timeout
                | ExecutionStatus::Cancelled
        )
    }

    /// Convert to human-readable string
    pub fn as_str(&self) -> &'static str {
        match self {
            ExecutionStatus::Pending => "pending",
            ExecutionStatus::Running => "running",
            ExecutionStatus::Completed => "completed",
            ExecutionStatus::Failed => "failed",
            ExecutionStatus::Timeout => "timeout",
            ExecutionStatus::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for ExecutionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// Artifact Types
// ============================================================================

/// Artifact collected during execution (screenshots, files, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Unique artifact identifier
    pub id: String,
    /// Artifact name/filename
    pub name: String,
    /// MIME content type
    pub content_type: String,
    /// Raw artifact data
    #[serde(with = "base64_serde")]
    pub data: Vec<u8>,
    /// Size in bytes
    pub size: u64,
    /// Artifact type category
    pub artifact_type: ArtifactType,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Optional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Artifact {
    /// Create a new artifact
    pub fn new(
        name: impl Into<String>,
        content_type: impl Into<String>,
        data: Vec<u8>,
        artifact_type: ArtifactType,
    ) -> Self {
        let size = data.len() as u64;
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            content_type: content_type.into(),
            data,
            size,
            artifact_type,
            created_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Create a screenshot artifact
    pub fn screenshot(name: impl Into<String>, data: Vec<u8>, format: &str) -> Self {
        let content_type = match format.to_lowercase().as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            _ => "image/png",
        };
        Self::new(name, content_type, data, ArtifactType::Screenshot)
    }

    /// Create a file artifact
    pub fn file(name: impl Into<String>, content_type: impl Into<String>, data: Vec<u8>) -> Self {
        Self::new(name, content_type, data, ArtifactType::File)
    }

    /// Create a log artifact
    pub fn log(name: impl Into<String>, content: String) -> Self {
        Self::new(name, "text/plain", content.into_bytes(), ArtifactType::Log)
    }

    /// Create a JSON data artifact
    pub fn json_data(name: impl Into<String>, value: &serde_json::Value) -> Result<Self> {
        let data =
            serde_json::to_vec_pretty(value).map_err(|e| Error::Serialization(e.to_string()))?;
        Ok(Self::new(
            name,
            "application/json",
            data,
            ArtifactType::Data,
        ))
    }

    /// Get data as base64 string
    pub fn data_base64(&self) -> String {
        BASE64.encode(&self.data)
    }

    /// Get data as string (if valid UTF-8)
    pub fn data_as_string(&self) -> Option<String> {
        String::from_utf8(self.data.clone()).ok()
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Convert to API artifact (with base64 data)
    pub fn to_api_artifact(&self) -> ApiArtifact {
        ApiArtifact {
            id: self.id.clone(),
            name: self.name.clone(),
            content_type: self.content_type.clone(),
            data_base64: self.data_base64(),
            size: self.size,
            artifact_type: self.artifact_type.clone(),
            created_at: self.created_at,
        }
    }
}

/// Artifact type category
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    /// Browser screenshot
    Screenshot,
    /// Generated or modified file
    File,
    /// Log output
    Log,
    /// Structured data (JSON, etc.)
    Data,
    /// Video recording
    Video,
    /// Audio recording
    Audio,
    /// HTML content
    Html,
    /// PDF document
    Pdf,
    /// Other artifact type
    Other,
}

impl ArtifactType {
    /// Get default content type for artifact type
    pub fn default_content_type(&self) -> &'static str {
        match self {
            ArtifactType::Screenshot => "image/png",
            ArtifactType::File => "application/octet-stream",
            ArtifactType::Log => "text/plain",
            ArtifactType::Data => "application/json",
            ArtifactType::Video => "video/webm",
            ArtifactType::Audio => "audio/webm",
            ArtifactType::Html => "text/html",
            ArtifactType::Pdf => "application/pdf",
            ArtifactType::Other => "application/octet-stream",
        }
    }
}

/// API-friendly artifact representation (with base64 data)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiArtifact {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub data_base64: String,
    pub size: u64,
    pub artifact_type: ArtifactType,
    pub created_at: DateTime<Utc>,
}

// ============================================================================
// Error Details
// ============================================================================

/// Detailed error information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetails {
    /// Error message
    pub message: String,
    /// Error type category
    pub error_type: ErrorType,
    /// Optional error code
    pub code: Option<String>,
    /// Optional stack trace
    pub stack_trace: Option<String>,
}

/// Error type enumeration
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    /// Syntax error in code
    SyntaxError,
    /// Runtime execution error
    ExecutionError,
    /// Import/module error
    ImportError,
    /// Timeout error
    Timeout,
    /// Cancelled by user
    Cancelled,
    /// Resource limit exceeded
    ResourceLimit,
    /// Permission denied
    PermissionDenied,
    /// Network error
    NetworkError,
    /// System error
    SystemError,
    /// Unknown error
    Unknown,
}

// ============================================================================
// Execution Metadata
// ============================================================================

/// Execution metadata
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionMetadata {
    /// Language/runtime used
    pub language: Option<String>,
    /// VM instance ID
    pub vm_id: Option<String>,
    /// Session ID (if part of a session)
    pub session_id: Option<String>,
    /// Memory usage in bytes (peak)
    pub memory_bytes: Option<u64>,
    /// CPU time in milliseconds
    pub cpu_time_ms: Option<u64>,
    /// Custom metadata
    #[serde(flatten)]
    pub custom: HashMap<String, serde_json::Value>,
}

// ============================================================================
// API Response Types
// ============================================================================

/// API response format for execution results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmExecutionResponse {
    pub execution_id: String,
    pub status: ExecutionStatus,
    pub stdout: String,
    pub stderr: String,
    pub return_value: Option<serde_json::Value>,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub artifacts: Vec<ApiArtifact>,
    pub variable_names: Vec<String>,
    pub error: Option<String>,
}

// ============================================================================
// Streaming Result Types
// ============================================================================

/// Streaming output chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamChunk {
    /// Standard output data
    Stdout { data: String, offset: u64 },
    /// Standard error data
    Stderr { data: String, offset: u64 },
    /// Return value available
    ReturnValue { value: serde_json::Value },
    /// Artifact collected
    Artifact { artifact: ApiArtifact },
    /// Variable captured
    Variable {
        name: String,
        value: serde_json::Value,
    },
    /// Status update
    Status {
        status: ExecutionStatus,
        message: Option<String>,
    },
    /// Progress update
    Progress {
        percent: f32,
        message: Option<String>,
    },
    /// Heartbeat (keep-alive)
    Heartbeat { elapsed_ms: u64 },
    /// Execution completed
    Done {
        exit_code: i32,
        duration_ms: u64,
        success: bool,
    },
    /// Error occurred
    Error { error: ErrorDetails },
}

/// Stream configuration
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Buffer size for stdout/stderr
    pub buffer_size: usize,
    /// Heartbeat interval in milliseconds
    pub heartbeat_interval_ms: u64,
    /// Whether to include artifacts in stream
    pub stream_artifacts: bool,
    /// Whether to include variables in stream
    pub stream_variables: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            buffer_size: 4096,
            heartbeat_interval_ms: 5000,
            stream_artifacts: true,
            stream_variables: true,
        }
    }
}

// ============================================================================
// Result Collector
// ============================================================================

/// Collects and aggregates partial results from VM execution
pub struct ResultCollector {
    /// The result being built
    result: Arc<RwLock<VmExecutionResult>>,
    /// Stream sender for real-time updates
    stream_tx: Option<mpsc::Sender<StreamChunk>>,
    /// Stream configuration
    config: StreamConfig,
    /// Current stdout offset
    stdout_offset: Arc<RwLock<u64>>,
    /// Current stderr offset
    stderr_offset: Arc<RwLock<u64>>,
}

impl ResultCollector {
    /// Create a new result collector
    pub fn new(execution_id: impl Into<String>) -> Self {
        Self {
            result: Arc::new(RwLock::new(VmExecutionResult::new(execution_id))),
            stream_tx: None,
            config: StreamConfig::default(),
            stdout_offset: Arc::new(RwLock::new(0)),
            stderr_offset: Arc::new(RwLock::new(0)),
        }
    }

    /// Create a collector with streaming support
    pub fn with_stream(
        execution_id: impl Into<String>,
        stream_tx: mpsc::Sender<StreamChunk>,
        config: StreamConfig,
    ) -> Self {
        Self {
            result: Arc::new(RwLock::new(VmExecutionResult::new(execution_id))),
            stream_tx: Some(stream_tx),
            config,
            stdout_offset: Arc::new(RwLock::new(0)),
            stderr_offset: Arc::new(RwLock::new(0)),
        }
    }

    /// Append stdout data
    #[instrument(skip(self, data), fields(data_len = data.len()))]
    pub async fn append_stdout(&self, data: &str) {
        let mut result = self.result.write().await;
        result.stdout.push_str(data);

        if let Some(tx) = &self.stream_tx {
            let mut offset = self.stdout_offset.write().await;
            let chunk = StreamChunk::Stdout {
                data: data.to_string(),
                offset: *offset,
            };
            *offset += data.len() as u64;
            if tx.send(chunk).await.is_err() {
                debug!("Stream receiver dropped");
            }
        }
    }

    /// Append stderr data
    #[instrument(skip(self, data), fields(data_len = data.len()))]
    pub async fn append_stderr(&self, data: &str) {
        let mut result = self.result.write().await;
        result.stderr.push_str(data);

        if let Some(tx) = &self.stream_tx {
            let mut offset = self.stderr_offset.write().await;
            let chunk = StreamChunk::Stderr {
                data: data.to_string(),
                offset: *offset,
            };
            *offset += data.len() as u64;
            if tx.send(chunk).await.is_err() {
                debug!("Stream receiver dropped");
            }
        }
    }

    /// Set the return value
    #[instrument(skip(self, value))]
    pub async fn set_return_value(&self, value: serde_json::Value) {
        let mut result = self.result.write().await;
        result.return_value = Some(value.clone());

        if let Some(tx) = &self.stream_tx {
            let chunk = StreamChunk::ReturnValue { value };
            if tx.send(chunk).await.is_err() {
                debug!("Stream receiver dropped");
            }
        }
    }

    /// Add an artifact
    #[instrument(skip(self, artifact), fields(artifact_name = %artifact.name))]
    pub async fn add_artifact(&self, artifact: Artifact) {
        let api_artifact = artifact.to_api_artifact();

        let mut result = self.result.write().await;
        result.artifacts.push(artifact);

        if self.config.stream_artifacts {
            if let Some(tx) = &self.stream_tx {
                let chunk = StreamChunk::Artifact {
                    artifact: api_artifact,
                };
                if tx.send(chunk).await.is_err() {
                    debug!("Stream receiver dropped");
                }
            }
        }
    }

    /// Add a variable
    #[instrument(skip(self, name, value))]
    pub async fn add_variable(&self, name: impl Into<String>, value: serde_json::Value) {
        let name = name.into();

        let mut result = self.result.write().await;
        result.variables.insert(name.clone(), value.clone());

        if self.config.stream_variables {
            if let Some(tx) = &self.stream_tx {
                let chunk = StreamChunk::Variable { name, value };
                if tx.send(chunk).await.is_err() {
                    debug!("Stream receiver dropped");
                }
            }
        }
    }

    /// Update execution status
    #[instrument(skip(self))]
    pub async fn update_status(&self, status: ExecutionStatus, message: Option<String>) {
        let mut result = self.result.write().await;
        result.status = status;

        if let Some(tx) = &self.stream_tx {
            let chunk = StreamChunk::Status { status, message };
            if tx.send(chunk).await.is_err() {
                debug!("Stream receiver dropped");
            }
        }
    }

    /// Send progress update
    #[instrument(skip(self))]
    pub async fn send_progress(&self, percent: f32, message: Option<String>) {
        if let Some(tx) = &self.stream_tx {
            let chunk = StreamChunk::Progress { percent, message };
            if tx.send(chunk).await.is_err() {
                debug!("Stream receiver dropped");
            }
        }
    }

    /// Send heartbeat
    pub async fn send_heartbeat(&self, elapsed_ms: u64) {
        if let Some(tx) = &self.stream_tx {
            let chunk = StreamChunk::Heartbeat { elapsed_ms };
            if tx.send(chunk).await.is_err() {
                debug!("Stream receiver dropped");
            }
        }
    }

    /// Mark execution as completed
    #[instrument(skip(self))]
    pub async fn complete(&self, exit_code: i32, duration_ms: u64) {
        let success = exit_code == 0;

        {
            let mut result = self.result.write().await;
            result.mark_completed(exit_code, duration_ms);
        }

        if let Some(tx) = &self.stream_tx {
            let chunk = StreamChunk::Done {
                exit_code,
                duration_ms,
                success,
            };
            if tx.send(chunk).await.is_err() {
                debug!("Stream receiver dropped");
            }
        }
    }

    /// Mark execution as failed with error
    #[instrument(skip(self))]
    pub async fn fail(&self, error: ErrorDetails, duration_ms: u64) {
        {
            let mut result = self.result.write().await;
            result.status = ExecutionStatus::Failed;
            result.error_details = Some(error.clone());
            result.duration_ms = duration_ms;
            result.completed_at = Some(Utc::now());
        }

        if let Some(tx) = &self.stream_tx {
            let chunk = StreamChunk::Error { error };
            if tx.send(chunk).await.is_err() {
                debug!("Stream receiver dropped");
            }
        }
    }

    /// Mark execution as timed out
    #[instrument(skip(self))]
    pub async fn timeout(&self, timeout_ms: u64) {
        let error = ErrorDetails {
            message: format!("Execution timed out after {}ms", timeout_ms),
            error_type: ErrorType::Timeout,
            code: Some("TIMEOUT".to_string()),
            stack_trace: None,
        };

        {
            let mut result = self.result.write().await;
            result.status = ExecutionStatus::Timeout;
            result.error_details = Some(error.clone());
            result.exit_code = 124;
            result.duration_ms = timeout_ms;
            result.completed_at = Some(Utc::now());
        }

        if let Some(tx) = &self.stream_tx {
            let chunk = StreamChunk::Error { error };
            if tx.send(chunk).await.is_err() {
                debug!("Stream receiver dropped");
            }
        }
    }

    /// Mark execution as cancelled
    #[instrument(skip(self))]
    pub async fn cancel(&self, duration_ms: u64) {
        let error = ErrorDetails {
            message: "Execution was cancelled".to_string(),
            error_type: ErrorType::Cancelled,
            code: Some("CANCELLED".to_string()),
            stack_trace: None,
        };

        {
            let mut result = self.result.write().await;
            result.status = ExecutionStatus::Cancelled;
            result.error_details = Some(error.clone());
            result.exit_code = 130;
            result.duration_ms = duration_ms;
            result.completed_at = Some(Utc::now());
        }

        if let Some(tx) = &self.stream_tx {
            let chunk = StreamChunk::Error { error };
            if tx.send(chunk).await.is_err() {
                debug!("Stream receiver dropped");
            }
        }
    }

    /// Get current result snapshot
    pub async fn get_result(&self) -> VmExecutionResult {
        self.result.read().await.clone()
    }

    /// Take final result (consumes collector)
    pub async fn take_result(self) -> VmExecutionResult {
        let result = self.result.read().await;
        result.clone()
    }

    /// Set metadata
    pub async fn set_metadata(&self, metadata: ExecutionMetadata) {
        let mut result = self.result.write().await;
        result.metadata = metadata;
    }

    /// Update metadata field
    pub async fn update_metadata(&self, key: impl Into<String>, value: serde_json::Value) {
        let mut result = self.result.write().await;
        result.metadata.custom.insert(key.into(), value);
    }
}

// ============================================================================
// Artifact Collector
// ============================================================================

/// Collects artifacts from VM execution
pub struct ArtifactCollector {
    /// Collected artifacts
    artifacts: Arc<RwLock<Vec<Artifact>>>,
    /// Maximum artifact size
    max_artifact_size: u64,
    /// Maximum total size
    max_total_size: u64,
    /// Current total size
    current_total_size: Arc<RwLock<u64>>,
}

impl ArtifactCollector {
    /// Create a new artifact collector
    pub fn new(max_artifact_size: u64, max_total_size: u64) -> Self {
        Self {
            artifacts: Arc::new(RwLock::new(Vec::new())),
            max_artifact_size,
            max_total_size,
            current_total_size: Arc::new(RwLock::new(0)),
        }
    }

    /// Create with default limits (10MB per artifact, 100MB total)
    pub fn with_defaults() -> Self {
        Self::new(10 * 1024 * 1024, 100 * 1024 * 1024)
    }

    /// Add an artifact
    #[instrument(skip(self, name, content_type, data))]
    pub async fn add(
        &self,
        name: impl Into<String>,
        content_type: impl Into<String>,
        data: Vec<u8>,
        artifact_type: ArtifactType,
    ) -> Result<Artifact> {
        let size = data.len() as u64;
        let name = name.into();
        let content_type = content_type.into();
        debug!(artifact_name = %name, artifact_size = size, "Adding artifact");

        // Check artifact size
        if size > self.max_artifact_size {
            warn!(
                size = size,
                max = self.max_artifact_size,
                "Artifact exceeds size limit"
            );
            return Err(Error::FileTooLarge {
                size,
                limit: self.max_artifact_size,
            });
        }

        // Check total size
        {
            let current = *self.current_total_size.read().await;
            if current + size > self.max_total_size {
                warn!(
                    current = current,
                    new_size = size,
                    max = self.max_total_size,
                    "Total artifact size limit exceeded"
                );
                return Err(Error::FileTooLarge {
                    size: current + size,
                    limit: self.max_total_size,
                });
            }
        }

        let artifact = Artifact::new(name, content_type.clone(), data, artifact_type);

        // Update total size and add artifact
        {
            let mut total = self.current_total_size.write().await;
            *total += size;
        }

        let mut artifacts = self.artifacts.write().await;
        artifacts.push(artifact.clone());

        Ok(artifact)
    }

    /// Add a screenshot
    pub async fn add_screenshot(
        &self,
        name: impl Into<String>,
        data: Vec<u8>,
        format: &str,
    ) -> Result<Artifact> {
        let content_type = match format.to_lowercase().as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            _ => "image/png",
        };
        self.add(name, content_type, data, ArtifactType::Screenshot)
            .await
    }

    /// Add a file artifact
    pub async fn add_file(
        &self,
        name: impl Into<String>,
        content_type: impl Into<String>,
        data: Vec<u8>,
    ) -> Result<Artifact> {
        self.add(name, content_type, data, ArtifactType::File).await
    }

    /// Add a log artifact
    pub async fn add_log(&self, name: impl Into<String>, content: String) -> Result<Artifact> {
        self.add(name, "text/plain", content.into_bytes(), ArtifactType::Log)
            .await
    }

    /// Get all collected artifacts
    pub async fn get_artifacts(&self) -> Vec<Artifact> {
        self.artifacts.read().await.clone()
    }

    /// Get current total size
    pub async fn total_size(&self) -> u64 {
        *self.current_total_size.read().await
    }

    /// Get artifact count
    pub async fn count(&self) -> usize {
        self.artifacts.read().await.len()
    }

    /// Clear all artifacts
    pub async fn clear(&self) {
        let mut artifacts = self.artifacts.write().await;
        artifacts.clear();
        let mut total = self.current_total_size.write().await;
        *total = 0;
    }
}

// ============================================================================
// Helper Modules
// ============================================================================

/// Serde helper for base64 encoding of `Vec<u8>`
mod base64_serde {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&BASE64.encode(data))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        BASE64.decode(&s).map_err(serde::de::Error::custom)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========== VmExecutionResult Tests ==========

    #[test]
    fn test_vm_execution_result_new() {
        let result = VmExecutionResult::new("exec-123");

        assert_eq!(result.execution_id, "exec-123");
        assert_eq!(result.status, ExecutionStatus::Running);
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
        assert!(result.return_value.is_none());
        assert_eq!(result.exit_code, -1);
        assert!(result.artifacts.is_empty());
        assert!(result.is_running());
        assert!(!result.is_terminal());
    }

    #[test]
    fn test_vm_execution_result_completed() {
        let result = VmExecutionResult::completed(
            "exec-456",
            "Hello, World!".to_string(),
            "".to_string(),
            0,
            100,
        );

        assert_eq!(result.execution_id, "exec-456");
        assert_eq!(result.status, ExecutionStatus::Completed);
        assert_eq!(result.stdout, "Hello, World!");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.duration_ms, 100);
        assert!(result.is_success());
        assert!(result.is_terminal());
    }

    #[test]
    fn test_vm_execution_result_failed() {
        let result = VmExecutionResult::failed("exec-789", "Division by zero");

        assert_eq!(result.status, ExecutionStatus::Failed);
        assert!(!result.is_success());
        assert!(result.error_details.is_some());
        assert_eq!(
            result.error_details.as_ref().unwrap().message,
            "Division by zero"
        );
    }

    #[test]
    fn test_vm_execution_result_timeout() {
        let result = VmExecutionResult::timeout("exec-timeout", 30000);

        assert_eq!(result.status, ExecutionStatus::Timeout);
        assert_eq!(result.exit_code, 124);
        assert_eq!(result.duration_ms, 30000);
        assert!(result.error_details.is_some());
        assert_eq!(
            result.error_details.as_ref().unwrap().error_type,
            ErrorType::Timeout
        );
    }

    #[test]
    fn test_vm_execution_result_cancelled() {
        let result = VmExecutionResult::cancelled("exec-cancelled", 5000);

        assert_eq!(result.status, ExecutionStatus::Cancelled);
        assert_eq!(result.exit_code, 130);
        assert!(result.error_details.is_some());
        assert_eq!(
            result.error_details.as_ref().unwrap().error_type,
            ErrorType::Cancelled
        );
    }

    #[test]
    fn test_vm_execution_result_add_artifact() {
        let mut result = VmExecutionResult::new("exec-art");
        let artifact = Artifact::screenshot("test.png", vec![1, 2, 3], "png");

        result.add_artifact(artifact);

        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(result.artifacts[0].name, "test.png");
        assert_eq!(result.total_artifact_size(), 3);
    }

    #[test]
    fn test_vm_execution_result_add_variable() {
        let mut result = VmExecutionResult::new("exec-var");

        result.add_variable("x", serde_json::json!(42));
        result.add_variable("y", serde_json::json!("hello"));

        assert_eq!(result.variables.len(), 2);
        assert_eq!(result.variables.get("x"), Some(&serde_json::json!(42)));
    }

    #[test]
    fn test_vm_execution_result_serialization() {
        let mut result = VmExecutionResult::completed(
            "exec-serial",
            "output".to_string(),
            "".to_string(),
            0,
            50,
        );
        result.add_variable("test", serde_json::json!(123));

        let json = serde_json::to_string(&result).unwrap();
        let parsed: VmExecutionResult = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.execution_id, "exec-serial");
        assert_eq!(parsed.status, ExecutionStatus::Completed);
        assert_eq!(parsed.stdout, "output");
    }

    #[test]
    fn test_vm_execution_result_to_api_response() {
        let mut result = VmExecutionResult::completed(
            "exec-api",
            "stdout".to_string(),
            "stderr".to_string(),
            0,
            100,
        );
        result.add_variable("var1", serde_json::json!(1));
        result.add_artifact(Artifact::log("test.log", "log content".to_string()));

        let response = result.to_api_response();

        assert_eq!(response.execution_id, "exec-api");
        assert_eq!(response.stdout, "stdout");
        assert_eq!(response.stderr, "stderr");
        assert_eq!(response.exit_code, 0);
        assert_eq!(response.artifacts.len(), 1);
        assert!(response.variable_names.contains(&"var1".to_string()));
    }

    // ========== ExecutionStatus Tests ==========

    #[test]
    fn test_execution_status_is_terminal() {
        assert!(!ExecutionStatus::Pending.is_terminal());
        assert!(!ExecutionStatus::Running.is_terminal());
        assert!(ExecutionStatus::Completed.is_terminal());
        assert!(ExecutionStatus::Failed.is_terminal());
        assert!(ExecutionStatus::Timeout.is_terminal());
        assert!(ExecutionStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_execution_status_display() {
        assert_eq!(ExecutionStatus::Running.to_string(), "running");
        assert_eq!(ExecutionStatus::Completed.to_string(), "completed");
        assert_eq!(ExecutionStatus::Timeout.to_string(), "timeout");
    }

    #[test]
    fn test_execution_status_serialization() {
        let json = serde_json::to_string(&ExecutionStatus::Running).unwrap();
        assert_eq!(json, "\"running\"");

        let parsed: ExecutionStatus = serde_json::from_str("\"completed\"").unwrap();
        assert_eq!(parsed, ExecutionStatus::Completed);
    }

    // ========== Artifact Tests ==========

    #[test]
    fn test_artifact_new() {
        let artifact = Artifact::new(
            "test.txt",
            "text/plain",
            vec![1, 2, 3, 4],
            ArtifactType::File,
        );

        assert!(!artifact.id.is_empty());
        assert_eq!(artifact.name, "test.txt");
        assert_eq!(artifact.content_type, "text/plain");
        assert_eq!(artifact.size, 4);
        assert_eq!(artifact.artifact_type, ArtifactType::File);
    }

    #[test]
    fn test_artifact_screenshot() {
        let artifact = Artifact::screenshot("screen.png", vec![0x89, 0x50, 0x4E, 0x47], "png");

        assert_eq!(artifact.content_type, "image/png");
        assert_eq!(artifact.artifact_type, ArtifactType::Screenshot);
    }

    #[test]
    fn test_artifact_file() {
        let artifact = Artifact::file("data.bin", "application/octet-stream", vec![0, 1, 2]);

        assert_eq!(artifact.artifact_type, ArtifactType::File);
    }

    #[test]
    fn test_artifact_log() {
        let artifact = Artifact::log("output.log", "Log line 1\nLog line 2".to_string());

        assert_eq!(artifact.content_type, "text/plain");
        assert_eq!(artifact.artifact_type, ArtifactType::Log);
        assert_eq!(
            artifact.data_as_string(),
            Some("Log line 1\nLog line 2".to_string())
        );
    }

    #[test]
    fn test_artifact_json_data() {
        let value = serde_json::json!({"key": "value", "num": 42});
        let artifact = Artifact::json_data("data.json", &value).unwrap();

        assert_eq!(artifact.content_type, "application/json");
        assert_eq!(artifact.artifact_type, ArtifactType::Data);
    }

    #[test]
    fn test_artifact_data_base64() {
        let artifact = Artifact::new("test", "text/plain", b"hello".to_vec(), ArtifactType::File);

        let base64 = artifact.data_base64();
        assert_eq!(base64, "aGVsbG8=");
    }

    #[test]
    fn test_artifact_with_metadata() {
        let artifact = Artifact::new("test", "text/plain", vec![], ArtifactType::File)
            .with_metadata("key1", serde_json::json!("value1"))
            .with_metadata("key2", serde_json::json!(42));

        assert_eq!(artifact.metadata.len(), 2);
        assert_eq!(
            artifact.metadata.get("key1"),
            Some(&serde_json::json!("value1"))
        );
    }

    #[test]
    fn test_artifact_to_api_artifact() {
        let artifact = Artifact::new(
            "test.txt",
            "text/plain",
            b"data".to_vec(),
            ArtifactType::File,
        );

        let api = artifact.to_api_artifact();

        assert_eq!(api.id, artifact.id);
        assert_eq!(api.name, "test.txt");
        assert_eq!(api.data_base64, "ZGF0YQ==");
    }

    #[test]
    fn test_artifact_serialization() {
        let artifact = Artifact::new("test", "text/plain", b"hello".to_vec(), ArtifactType::File);

        let json = serde_json::to_string(&artifact).unwrap();
        let parsed: Artifact = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.name, artifact.name);
        assert_eq!(parsed.data, artifact.data);
    }

    // ========== ArtifactType Tests ==========

    #[test]
    fn test_artifact_type_default_content_type() {
        assert_eq!(ArtifactType::Screenshot.default_content_type(), "image/png");
        assert_eq!(ArtifactType::Log.default_content_type(), "text/plain");
        assert_eq!(
            ArtifactType::Data.default_content_type(),
            "application/json"
        );
        assert_eq!(ArtifactType::Pdf.default_content_type(), "application/pdf");
    }

    #[test]
    fn test_artifact_type_serialization() {
        let json = serde_json::to_string(&ArtifactType::Screenshot).unwrap();
        assert_eq!(json, "\"screenshot\"");

        let parsed: ArtifactType = serde_json::from_str("\"log\"").unwrap();
        assert_eq!(parsed, ArtifactType::Log);
    }

    // ========== ErrorDetails Tests ==========

    #[test]
    fn test_error_details_serialization() {
        let error = ErrorDetails {
            message: "Test error".to_string(),
            error_type: ErrorType::ExecutionError,
            code: Some("ERR001".to_string()),
            stack_trace: Some("at line 1\nat line 2".to_string()),
        };

        let json = serde_json::to_string(&error).unwrap();
        let parsed: ErrorDetails = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.message, "Test error");
        assert_eq!(parsed.error_type, ErrorType::ExecutionError);
        assert_eq!(parsed.code, Some("ERR001".to_string()));
    }

    // ========== StreamChunk Tests ==========

    #[test]
    fn test_stream_chunk_stdout() {
        let chunk = StreamChunk::Stdout {
            data: "hello".to_string(),
            offset: 0,
        };

        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("\"type\":\"stdout\""));
        assert!(json.contains("\"data\":\"hello\""));
    }

    #[test]
    fn test_stream_chunk_done() {
        let chunk = StreamChunk::Done {
            exit_code: 0,
            duration_ms: 1000,
            success: true,
        };

        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("\"type\":\"done\""));
        assert!(json.contains("\"success\":true"));
    }

    #[test]
    fn test_stream_chunk_error() {
        let chunk = StreamChunk::Error {
            error: ErrorDetails {
                message: "Failed".to_string(),
                error_type: ErrorType::ExecutionError,
                code: None,
                stack_trace: None,
            },
        };

        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("\"type\":\"error\""));
        assert!(json.contains("\"message\":\"Failed\""));
    }

    // ========== ResultCollector Tests ==========

    #[tokio::test]
    async fn test_result_collector_basic() {
        let collector = ResultCollector::new("exec-collect");

        collector.append_stdout("Hello").await;
        collector.append_stderr("Warning").await;

        let result = collector.get_result().await;
        assert_eq!(result.stdout, "Hello");
        assert_eq!(result.stderr, "Warning");
    }

    #[tokio::test]
    async fn test_result_collector_set_return_value() {
        let collector = ResultCollector::new("exec-rv");

        collector.set_return_value(serde_json::json!(42)).await;

        let result = collector.get_result().await;
        assert_eq!(result.return_value, Some(serde_json::json!(42)));
    }

    #[tokio::test]
    async fn test_result_collector_add_artifact() {
        let collector = ResultCollector::new("exec-art");
        let artifact = Artifact::log("test.log", "content".to_string());

        collector.add_artifact(artifact).await;

        let result = collector.get_result().await;
        assert_eq!(result.artifacts.len(), 1);
    }

    #[tokio::test]
    async fn test_result_collector_add_variable() {
        let collector = ResultCollector::new("exec-var");

        collector.add_variable("x", serde_json::json!(100)).await;

        let result = collector.get_result().await;
        assert_eq!(result.variables.get("x"), Some(&serde_json::json!(100)));
    }

    #[tokio::test]
    async fn test_result_collector_complete() {
        let collector = ResultCollector::new("exec-complete");

        collector.complete(0, 500).await;

        let result = collector.get_result().await;
        assert_eq!(result.status, ExecutionStatus::Completed);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.duration_ms, 500);
    }

    #[tokio::test]
    async fn test_result_collector_fail() {
        let collector = ResultCollector::new("exec-fail");
        let error = ErrorDetails {
            message: "Test failure".to_string(),
            error_type: ErrorType::ExecutionError,
            code: None,
            stack_trace: None,
        };

        collector.fail(error, 100).await;

        let result = collector.get_result().await;
        assert_eq!(result.status, ExecutionStatus::Failed);
        assert!(result.error_details.is_some());
    }

    #[tokio::test]
    async fn test_result_collector_timeout() {
        let collector = ResultCollector::new("exec-timeout");

        collector.timeout(30000).await;

        let result = collector.get_result().await;
        assert_eq!(result.status, ExecutionStatus::Timeout);
        assert_eq!(result.exit_code, 124);
    }

    #[tokio::test]
    async fn test_result_collector_cancel() {
        let collector = ResultCollector::new("exec-cancel");

        collector.cancel(5000).await;

        let result = collector.get_result().await;
        assert_eq!(result.status, ExecutionStatus::Cancelled);
        assert_eq!(result.exit_code, 130);
    }

    #[tokio::test]
    async fn test_result_collector_with_stream() {
        let (tx, mut rx) = mpsc::channel(100);
        let collector = ResultCollector::with_stream("exec-stream", tx, StreamConfig::default());

        collector.append_stdout("Hello").await;
        collector.complete(0, 100).await;

        // Check received chunks
        let chunk1 = rx.recv().await.unwrap();
        assert!(matches!(chunk1, StreamChunk::Stdout { .. }));

        let chunk2 = rx.recv().await.unwrap();
        assert!(matches!(chunk2, StreamChunk::Done { .. }));
    }

    #[tokio::test]
    async fn test_result_collector_take_result() {
        let collector = ResultCollector::new("exec-take");
        collector.append_stdout("data").await;
        collector.complete(0, 50).await;

        let result = collector.take_result().await;
        assert_eq!(result.stdout, "data");
        assert_eq!(result.status, ExecutionStatus::Completed);
    }

    // ========== ArtifactCollector Tests ==========

    #[tokio::test]
    async fn test_artifact_collector_basic() {
        let collector = ArtifactCollector::with_defaults();

        let artifact = collector
            .add(
                "test.txt",
                "text/plain",
                b"content".to_vec(),
                ArtifactType::File,
            )
            .await
            .unwrap();

        assert_eq!(artifact.name, "test.txt");
        assert_eq!(collector.count().await, 1);
        assert_eq!(collector.total_size().await, 7);
    }

    #[tokio::test]
    async fn test_artifact_collector_screenshot() {
        let collector = ArtifactCollector::with_defaults();

        let artifact = collector
            .add_screenshot("screen.png", vec![1, 2, 3, 4], "png")
            .await
            .unwrap();

        assert_eq!(artifact.content_type, "image/png");
        assert_eq!(artifact.artifact_type, ArtifactType::Screenshot);
    }

    #[tokio::test]
    async fn test_artifact_collector_log() {
        let collector = ArtifactCollector::with_defaults();

        let artifact = collector
            .add_log("output.log", "Log content".to_string())
            .await
            .unwrap();

        assert_eq!(artifact.artifact_type, ArtifactType::Log);
    }

    #[tokio::test]
    async fn test_artifact_collector_size_limit() {
        let collector = ArtifactCollector::new(10, 100); // 10 byte limit per artifact

        let result = collector
            .add(
                "large.bin",
                "application/octet-stream",
                vec![0; 20], // 20 bytes, exceeds limit
                ArtifactType::File,
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_artifact_collector_total_size_limit() {
        let collector = ArtifactCollector::new(100, 50); // 50 byte total limit

        // First artifact succeeds
        collector
            .add("a.txt", "text/plain", vec![0; 30], ArtifactType::File)
            .await
            .unwrap();

        // Second artifact exceeds total
        let result = collector
            .add("b.txt", "text/plain", vec![0; 30], ArtifactType::File)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_artifact_collector_clear() {
        let collector = ArtifactCollector::with_defaults();

        collector
            .add("test", "text/plain", vec![1, 2, 3], ArtifactType::File)
            .await
            .unwrap();

        assert_eq!(collector.count().await, 1);

        collector.clear().await;

        assert_eq!(collector.count().await, 0);
        assert_eq!(collector.total_size().await, 0);
    }

    #[tokio::test]
    async fn test_artifact_collector_get_artifacts() {
        let collector = ArtifactCollector::with_defaults();

        collector
            .add("a.txt", "text/plain", vec![1], ArtifactType::File)
            .await
            .unwrap();
        collector
            .add("b.txt", "text/plain", vec![2], ArtifactType::File)
            .await
            .unwrap();

        let artifacts = collector.get_artifacts().await;
        assert_eq!(artifacts.len(), 2);
    }

    // ========== Integration Tests ==========

    #[tokio::test]
    async fn test_full_execution_flow() {
        let (tx, mut rx) = mpsc::channel(100);
        let collector = ResultCollector::with_stream("exec-full", tx, StreamConfig::default());

        // Simulate execution
        collector
            .update_status(ExecutionStatus::Running, None)
            .await;
        collector.append_stdout("Processing...\n").await;
        collector
            .send_progress(50.0, Some("Halfway done".to_string()))
            .await;
        collector.append_stdout("Done!\n").await;
        collector
            .set_return_value(serde_json::json!({"result": "success"}))
            .await;

        let artifact = Artifact::log("execution.log", "Full log content".to_string());
        collector.add_artifact(artifact).await;

        collector
            .add_variable("output", serde_json::json!("processed"))
            .await;
        collector.complete(0, 1500).await;

        // Verify final result
        let result = collector.get_result().await;
        assert_eq!(result.status, ExecutionStatus::Completed);
        assert_eq!(result.stdout, "Processing...\nDone!\n");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.duration_ms, 1500);
        assert_eq!(result.artifacts.len(), 1);
        assert!(result.variables.contains_key("output"));
        assert!(result.return_value.is_some());

        // Verify stream received all chunks
        let mut chunks = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            chunks.push(chunk);
        }
        assert!(chunks.len() >= 6); // status, stdout x2, progress, return_value, artifact, variable, done
    }

    #[test]
    fn test_execution_metadata_default() {
        let metadata = ExecutionMetadata::default();

        assert!(metadata.language.is_none());
        assert!(metadata.vm_id.is_none());
        assert!(metadata.session_id.is_none());
        assert!(metadata.memory_bytes.is_none());
        assert!(metadata.custom.is_empty());
    }

    #[test]
    fn test_stream_config_default() {
        let config = StreamConfig::default();

        assert_eq!(config.buffer_size, 4096);
        assert_eq!(config.heartbeat_interval_ms, 5000);
        assert!(config.stream_artifacts);
        assert!(config.stream_variables);
    }
}
