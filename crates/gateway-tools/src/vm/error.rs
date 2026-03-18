//! VM-Specific Error Types and Handling
//!
//! This module provides comprehensive error handling for VM (Virtual Machine)
//! operations in cloud execution environments. It extends the base error module
//! with VM-specific error types for lifecycle management, communication,
//! resource pooling, and execution.
//!
//! # Error Categories
//!
//! - **Lifecycle Errors**: VM start, stop, and state management issues
//! - **Communication Errors**: Network, timeout, and response handling
//! - **Resource Errors**: Pool exhaustion and VM health issues
//! - **Execution Errors**: Code execution failures within VMs
//!
//! # Integration
//!
//! This module integrates with:
//! - `executor::error::ExecutionError` for execution-related errors
//! - `gateway_core::Error` for top-level error handling

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

use crate::executor::error::ExecutionError;

/// VM-specific error types for cloud execution
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum VmError {
    // === Lifecycle Errors ===
    /// VM failed to start
    #[error("VM start failed: {reason}")]
    StartFailed { reason: String },

    /// VM failed to stop
    #[error("VM stop failed: {reason}")]
    StopFailed { reason: String },

    /// VM is not running when expected to be
    #[error("VM not running: {vm_id}")]
    NotRunning { vm_id: String },

    /// VM is already running when attempting to start
    #[error("VM already running: {vm_id}")]
    AlreadyRunning { vm_id: String },

    /// VM not found in the system
    #[error("VM not found: {vm_id}")]
    NotFound { vm_id: String },

    /// VM is in an invalid state for the requested operation
    #[error(
        "VM in invalid state: {vm_id}, current state: {current_state}, expected: {expected_state}"
    )]
    InvalidState {
        vm_id: String,
        current_state: String,
        expected_state: String,
    },

    /// VM initialization failed
    #[error("VM initialization failed: {vm_id}, reason: {reason}")]
    InitializationFailed { vm_id: String, reason: String },

    /// VM shutdown timed out
    #[error("VM shutdown timed out: {vm_id}, timeout: {timeout_ms}ms")]
    ShutdownTimeout { vm_id: String, timeout_ms: u64 },

    // === Communication Errors ===
    /// Connection to VM failed
    #[error("Connection to VM failed: {vm_id} at {url}")]
    ConnectionFailed { vm_id: String, url: String },

    /// Request to VM timed out
    #[error("Request to VM timed out: {vm_id}, timeout: {timeout_ms}ms")]
    RequestTimeout { vm_id: String, timeout_ms: u64 },

    /// Response from VM was invalid or malformed
    #[error("Invalid response from VM: {vm_id}, message: {message}")]
    ResponseInvalid { vm_id: String, message: String },

    /// Network error during VM communication
    #[error("Network error with VM: {vm_id}, error: {message}")]
    NetworkError { vm_id: String, message: String },

    /// Socket error during VM communication
    #[error("Socket error with VM: {vm_id}, path: {socket_path}, error: {message}")]
    SocketError {
        vm_id: String,
        socket_path: String,
        message: String,
    },

    /// API error from VM
    #[error("VM API error: {vm_id}, status: {status_code}, message: {message}")]
    ApiError {
        vm_id: String,
        status_code: u16,
        message: String,
    },

    // === Resource Errors ===
    /// VM pool is exhausted (no available VMs)
    #[error("VM pool exhausted: no available VMs")]
    PoolExhausted,

    /// No healthy VM available in the pool
    #[error("No healthy VM available")]
    NoHealthyVm,

    /// VM is unhealthy and cannot be used
    #[error("VM unhealthy: {vm_id}, reason: {reason}")]
    VmUnhealthy { vm_id: String, reason: String },

    /// VM resource limit exceeded
    #[error(
        "VM resource limit exceeded: {vm_id}, resource: {resource}, limit: {limit}, used: {used}"
    )]
    ResourceLimitExceeded {
        vm_id: String,
        resource: String,
        limit: String,
        used: String,
    },

    /// VM pool configuration error
    #[error("VM pool configuration error: {message}")]
    PoolConfigError { message: String },

    /// Failed to acquire VM from pool
    #[error("Failed to acquire VM: {reason}")]
    AcquisitionFailed { reason: String },

    /// Failed to release VM back to pool
    #[error("Failed to release VM: {vm_id}, reason: {reason}")]
    ReleaseFailed { vm_id: String, reason: String },

    // === Execution Errors ===
    /// Code execution failed within VM
    #[error("Execution failed in VM: {vm_id}, execution_id: {execution_id}, error: {error}")]
    ExecutionFailed {
        vm_id: String,
        execution_id: String,
        error: String,
    },

    /// Execution timed out within VM
    #[error(
        "Execution timed out in VM: {vm_id}, execution_id: {execution_id}, timeout: {timeout_ms}ms"
    )]
    ExecutionTimeout {
        vm_id: String,
        execution_id: String,
        timeout_ms: u64,
    },

    /// Execution was cancelled
    #[error("Execution cancelled: {vm_id}, execution_id: {execution_id}")]
    ExecutionCancelled { vm_id: String, execution_id: String },

    // === Wrapped Executor Error ===
    /// Error from the executor module
    #[error("Executor error: {0}")]
    Executor(#[from] ExecutionError),

    // === Other Errors ===
    /// Configuration error
    #[error("VM configuration error: {message}")]
    ConfigError { message: String },

    /// IO error during VM operations
    #[error("VM IO error: {message}")]
    IoError { message: String },

    /// Serialization/deserialization error
    #[error("VM serialization error: {message}")]
    SerializationError { message: String },

    /// Internal VM error
    #[error("Internal VM error: {message}")]
    Internal { message: String },
}

impl VmError {
    /// Check if this error is recoverable
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            VmError::ConnectionFailed { .. }
                | VmError::RequestTimeout { .. }
                | VmError::NetworkError { .. }
                | VmError::SocketError { .. }
                | VmError::PoolExhausted
                | VmError::NoHealthyVm
                | VmError::VmUnhealthy { .. }
                | VmError::AcquisitionFailed { .. }
        )
    }

    /// Check if this error is a lifecycle error
    pub fn is_lifecycle_error(&self) -> bool {
        matches!(
            self,
            VmError::StartFailed { .. }
                | VmError::StopFailed { .. }
                | VmError::NotRunning { .. }
                | VmError::AlreadyRunning { .. }
                | VmError::NotFound { .. }
                | VmError::InvalidState { .. }
                | VmError::InitializationFailed { .. }
                | VmError::ShutdownTimeout { .. }
        )
    }

    /// Check if this error is a communication error
    pub fn is_communication_error(&self) -> bool {
        matches!(
            self,
            VmError::ConnectionFailed { .. }
                | VmError::RequestTimeout { .. }
                | VmError::ResponseInvalid { .. }
                | VmError::NetworkError { .. }
                | VmError::SocketError { .. }
                | VmError::ApiError { .. }
        )
    }

    /// Check if this error is a resource error
    pub fn is_resource_error(&self) -> bool {
        matches!(
            self,
            VmError::PoolExhausted
                | VmError::NoHealthyVm
                | VmError::VmUnhealthy { .. }
                | VmError::ResourceLimitExceeded { .. }
                | VmError::PoolConfigError { .. }
                | VmError::AcquisitionFailed { .. }
                | VmError::ReleaseFailed { .. }
        )
    }

    /// Check if this error is an execution error
    pub fn is_execution_error(&self) -> bool {
        matches!(
            self,
            VmError::ExecutionFailed { .. }
                | VmError::ExecutionTimeout { .. }
                | VmError::ExecutionCancelled { .. }
                | VmError::Executor(_)
        )
    }

    /// Check if this error is retriable
    pub fn is_retriable(&self) -> bool {
        match self {
            VmError::ConnectionFailed { .. }
            | VmError::RequestTimeout { .. }
            | VmError::NetworkError { .. }
            | VmError::SocketError { .. }
            | VmError::PoolExhausted
            | VmError::NoHealthyVm
            | VmError::AcquisitionFailed { .. } => true,
            VmError::Executor(exec_err) => exec_err.is_recoverable(),
            _ => false,
        }
    }

    /// Get the VM ID if available
    pub fn vm_id(&self) -> Option<&str> {
        match self {
            VmError::NotRunning { vm_id }
            | VmError::AlreadyRunning { vm_id }
            | VmError::NotFound { vm_id }
            | VmError::InvalidState { vm_id, .. }
            | VmError::InitializationFailed { vm_id, .. }
            | VmError::ShutdownTimeout { vm_id, .. }
            | VmError::ConnectionFailed { vm_id, .. }
            | VmError::RequestTimeout { vm_id, .. }
            | VmError::ResponseInvalid { vm_id, .. }
            | VmError::NetworkError { vm_id, .. }
            | VmError::SocketError { vm_id, .. }
            | VmError::ApiError { vm_id, .. }
            | VmError::VmUnhealthy { vm_id, .. }
            | VmError::ResourceLimitExceeded { vm_id, .. }
            | VmError::ReleaseFailed { vm_id, .. }
            | VmError::ExecutionFailed { vm_id, .. }
            | VmError::ExecutionTimeout { vm_id, .. }
            | VmError::ExecutionCancelled { vm_id, .. } => Some(vm_id),
            _ => None,
        }
    }

    /// Get the execution ID if available
    pub fn execution_id(&self) -> Option<&str> {
        match self {
            VmError::ExecutionFailed { execution_id, .. }
            | VmError::ExecutionTimeout { execution_id, .. }
            | VmError::ExecutionCancelled { execution_id, .. } => Some(execution_id),
            _ => None,
        }
    }

    /// Get an error code for categorization and logging
    pub fn error_code(&self) -> &'static str {
        match self {
            VmError::StartFailed { .. } => "E_VM_START_FAILED",
            VmError::StopFailed { .. } => "E_VM_STOP_FAILED",
            VmError::NotRunning { .. } => "E_VM_NOT_RUNNING",
            VmError::AlreadyRunning { .. } => "E_VM_ALREADY_RUNNING",
            VmError::NotFound { .. } => "E_VM_NOT_FOUND",
            VmError::InvalidState { .. } => "E_VM_INVALID_STATE",
            VmError::InitializationFailed { .. } => "E_VM_INIT_FAILED",
            VmError::ShutdownTimeout { .. } => "E_VM_SHUTDOWN_TIMEOUT",
            VmError::ConnectionFailed { .. } => "E_VM_CONNECTION_FAILED",
            VmError::RequestTimeout { .. } => "E_VM_REQUEST_TIMEOUT",
            VmError::ResponseInvalid { .. } => "E_VM_RESPONSE_INVALID",
            VmError::NetworkError { .. } => "E_VM_NETWORK_ERROR",
            VmError::SocketError { .. } => "E_VM_SOCKET_ERROR",
            VmError::ApiError { .. } => "E_VM_API_ERROR",
            VmError::PoolExhausted => "E_VM_POOL_EXHAUSTED",
            VmError::NoHealthyVm => "E_VM_NO_HEALTHY",
            VmError::VmUnhealthy { .. } => "E_VM_UNHEALTHY",
            VmError::ResourceLimitExceeded { .. } => "E_VM_RESOURCE_LIMIT",
            VmError::PoolConfigError { .. } => "E_VM_POOL_CONFIG",
            VmError::AcquisitionFailed { .. } => "E_VM_ACQUISITION_FAILED",
            VmError::ReleaseFailed { .. } => "E_VM_RELEASE_FAILED",
            VmError::ExecutionFailed { .. } => "E_VM_EXECUTION_FAILED",
            VmError::ExecutionTimeout { .. } => "E_VM_EXECUTION_TIMEOUT",
            VmError::ExecutionCancelled { .. } => "E_VM_EXECUTION_CANCELLED",
            VmError::Executor(_) => "E_VM_EXECUTOR",
            VmError::ConfigError { .. } => "E_VM_CONFIG",
            VmError::IoError { .. } => "E_VM_IO",
            VmError::SerializationError { .. } => "E_VM_SERIALIZATION",
            VmError::Internal { .. } => "E_VM_INTERNAL",
        }
    }

    /// Get a user-friendly error message
    pub fn user_message(&self) -> String {
        match self {
            VmError::StartFailed { reason } => {
                format!(
                    "Failed to start the execution environment. Please try again. Details: {}",
                    reason
                )
            }
            VmError::StopFailed { reason } => {
                format!(
                    "Failed to stop the execution environment. Details: {}",
                    reason
                )
            }
            VmError::NotRunning { vm_id } => {
                format!(
                    "The execution environment ({}) is not running. Please start a new session.",
                    truncate_id(vm_id)
                )
            }
            VmError::AlreadyRunning { vm_id } => {
                format!(
                    "The execution environment ({}) is already running.",
                    truncate_id(vm_id)
                )
            }
            VmError::PoolExhausted => {
                "All execution environments are currently in use. Please wait and try again."
                    .to_string()
            }
            VmError::NoHealthyVm => {
                "No healthy execution environments are available. Please try again later."
                    .to_string()
            }
            VmError::ConnectionFailed { .. } => {
                "Failed to connect to the execution environment. Please try again.".to_string()
            }
            VmError::RequestTimeout { timeout_ms, .. } => {
                format!(
                    "Request timed out after {}ms. The operation took too long.",
                    timeout_ms
                )
            }
            VmError::ExecutionFailed { error, .. } => {
                format!("Execution failed: {}", error)
            }
            VmError::ExecutionTimeout { timeout_ms, .. } => {
                format!(
                    "Execution timed out after {}ms. Consider breaking your code into smaller chunks.",
                    timeout_ms
                )
            }
            VmError::Executor(exec_err) => exec_err.user_message(),
            _ => self.to_string(),
        }
    }

    /// Get suggested retry delay in milliseconds
    pub fn suggested_retry_delay_ms(&self) -> Option<u64> {
        match self {
            VmError::ConnectionFailed { .. } => Some(1000),
            VmError::RequestTimeout { .. } => Some(2000),
            VmError::NetworkError { .. } => Some(1000),
            VmError::SocketError { .. } => Some(1000),
            VmError::PoolExhausted => Some(5000),
            VmError::NoHealthyVm => Some(3000),
            VmError::AcquisitionFailed { .. } => Some(2000),
            VmError::Executor(exec_err) => {
                if exec_err.is_recoverable() {
                    Some(1000)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Get maximum retry attempts for this error
    pub fn max_retry_attempts(&self) -> u32 {
        match self {
            VmError::ConnectionFailed { .. } => 3,
            VmError::RequestTimeout { .. } => 2,
            VmError::NetworkError { .. } => 3,
            VmError::SocketError { .. } => 3,
            VmError::PoolExhausted => 5,
            VmError::NoHealthyVm => 3,
            VmError::AcquisitionFailed { .. } => 3,
            _ => 0,
        }
    }
}

// Note: From<ExecutionError> for VmError is automatically implemented via #[from] attribute

/// Convert VmError to ServiceError for cross-crate interop.
impl From<VmError> for crate::ServiceError {
    fn from(err: VmError) -> Self {
        match &err {
            VmError::NotFound { vm_id } => {
                crate::ServiceError::NotFound(format!("VM not found: {}", vm_id))
            }
            VmError::PoolExhausted | VmError::NoHealthyVm => {
                crate::ServiceError::RateLimited("VM pool exhausted".to_string())
            }
            VmError::RequestTimeout { timeout_ms, .. }
            | VmError::ExecutionTimeout { timeout_ms, .. } => {
                crate::ServiceError::Timeout(format!("Operation timed out after {}ms", timeout_ms))
            }
            VmError::ShutdownTimeout { timeout_ms, .. } => crate::ServiceError::Timeout(format!(
                "VM shutdown timed out after {}ms",
                timeout_ms
            )),
            VmError::ExecutionFailed { error, .. } => {
                crate::ServiceError::ExecutionFailed(error.clone())
            }
            VmError::Executor(exec_err) => {
                crate::ServiceError::ExecutionFailed(exec_err.user_message())
            }
            VmError::ConfigError { message } => crate::ServiceError::Config(message.clone()),
            VmError::IoError { message } => {
                crate::ServiceError::Internal(format!("IO error: {}", message))
            }
            VmError::SerializationError { message } => {
                crate::ServiceError::Internal(format!("Serialization error: {}", message))
            }
            _ => crate::ServiceError::Internal(err.to_string()),
        }
    }
}

/// Convert std::io::Error to VmError
impl From<std::io::Error> for VmError {
    fn from(err: std::io::Error) -> Self {
        VmError::IoError {
            message: err.to_string(),
        }
    }
}

/// Convert serde_json::Error to VmError
impl From<serde_json::Error> for VmError {
    fn from(err: serde_json::Error) -> Self {
        VmError::SerializationError {
            message: err.to_string(),
        }
    }
}

/// Result type for VM operations
pub type VmResult<T> = std::result::Result<T, VmError>;

/// Helper to truncate VM IDs for user-friendly messages
fn truncate_id(id: &str) -> String {
    if id.len() > 8 {
        format!("{}...", &id[..8])
    } else {
        id.to_string()
    }
}

/// VM error context for enhanced error reporting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmErrorContext {
    /// Unique error ID for tracking
    pub error_id: String,
    /// Timestamp when error occurred
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// VM ID involved
    pub vm_id: Option<String>,
    /// Execution ID if applicable
    pub execution_id: Option<String>,
    /// Operation that caused the error
    pub operation: Option<String>,
    /// Additional metadata
    pub metadata: std::collections::HashMap<String, String>,
}

impl VmErrorContext {
    /// Create a new error context
    pub fn new() -> Self {
        Self {
            error_id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now(),
            vm_id: None,
            execution_id: None,
            operation: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Set VM ID
    pub fn with_vm_id(mut self, id: impl Into<String>) -> Self {
        self.vm_id = Some(id.into());
        self
    }

    /// Set execution ID
    pub fn with_execution_id(mut self, id: impl Into<String>) -> Self {
        self.execution_id = Some(id.into());
        self
    }

    /// Set operation name
    pub fn with_operation(mut self, op: impl Into<String>) -> Self {
        self.operation = Some(op.into());
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Create context from VmError
    pub fn from_error(error: &VmError) -> Self {
        let mut ctx = Self::new();
        if let Some(vm_id) = error.vm_id() {
            ctx.vm_id = Some(vm_id.to_string());
        }
        if let Some(exec_id) = error.execution_id() {
            ctx.execution_id = Some(exec_id.to_string());
        }
        ctx.metadata
            .insert("error_code".to_string(), error.error_code().to_string());
        ctx
    }
}

impl Default for VmErrorContext {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for VmErrorContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VmErrorContext {{ error_id: {}", self.error_id)?;
        if let Some(ref vm_id) = self.vm_id {
            write!(f, ", vm_id: {}", vm_id)?;
        }
        if let Some(ref exec_id) = self.execution_id {
            write!(f, ", execution_id: {}", exec_id)?;
        }
        if let Some(ref op) = self.operation {
            write!(f, ", operation: {}", op)?;
        }
        write!(f, " }}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vm_error_lifecycle_categorization() {
        let start_err = VmError::StartFailed {
            reason: "image not found".into(),
        };
        assert!(start_err.is_lifecycle_error());
        assert!(!start_err.is_communication_error());
        assert!(!start_err.is_resource_error());
        assert!(!start_err.is_execution_error());

        let not_running = VmError::NotRunning {
            vm_id: "vm-123".into(),
        };
        assert!(not_running.is_lifecycle_error());
        assert_eq!(not_running.vm_id(), Some("vm-123"));
    }

    #[test]
    fn test_vm_error_communication_categorization() {
        let conn_err = VmError::ConnectionFailed {
            vm_id: "vm-456".into(),
            url: "http://localhost:8080".into(),
        };
        assert!(conn_err.is_communication_error());
        assert!(!conn_err.is_lifecycle_error());
        assert!(conn_err.is_recoverable());
        assert!(conn_err.is_retriable());

        let timeout_err = VmError::RequestTimeout {
            vm_id: "vm-789".into(),
            timeout_ms: 30000,
        };
        assert!(timeout_err.is_communication_error());
        assert!(timeout_err.is_retriable());
    }

    #[test]
    fn test_vm_error_resource_categorization() {
        let pool_err = VmError::PoolExhausted;
        assert!(pool_err.is_resource_error());
        assert!(pool_err.is_recoverable());
        assert!(pool_err.is_retriable());
        assert!(pool_err.vm_id().is_none());

        let unhealthy = VmError::VmUnhealthy {
            vm_id: "vm-unhealthy".into(),
            reason: "health check failed".into(),
        };
        assert!(unhealthy.is_resource_error());
        assert_eq!(unhealthy.vm_id(), Some("vm-unhealthy"));
    }

    #[test]
    fn test_vm_error_execution_categorization() {
        let exec_err = VmError::ExecutionFailed {
            vm_id: "vm-exec".into(),
            execution_id: "exec-123".into(),
            error: "syntax error".into(),
        };
        assert!(exec_err.is_execution_error());
        assert!(!exec_err.is_recoverable());
        assert_eq!(exec_err.vm_id(), Some("vm-exec"));
        assert_eq!(exec_err.execution_id(), Some("exec-123"));

        let timeout = VmError::ExecutionTimeout {
            vm_id: "vm-timeout".into(),
            execution_id: "exec-456".into(),
            timeout_ms: 60000,
        };
        assert!(timeout.is_execution_error());
        assert_eq!(timeout.execution_id(), Some("exec-456"));
    }

    #[test]
    fn test_vm_error_codes() {
        assert_eq!(
            VmError::StartFailed { reason: "".into() }.error_code(),
            "E_VM_START_FAILED"
        );
        assert_eq!(VmError::PoolExhausted.error_code(), "E_VM_POOL_EXHAUSTED");
        assert_eq!(
            VmError::ExecutionFailed {
                vm_id: "".into(),
                execution_id: "".into(),
                error: "".into()
            }
            .error_code(),
            "E_VM_EXECUTION_FAILED"
        );
    }

    #[test]
    fn test_vm_error_user_messages() {
        let pool_err = VmError::PoolExhausted;
        let msg = pool_err.user_message();
        assert!(msg.contains("All execution environments"));
        assert!(msg.contains("try again"));

        let timeout_err = VmError::ExecutionTimeout {
            vm_id: "vm-123".into(),
            execution_id: "exec-456".into(),
            timeout_ms: 30000,
        };
        let msg = timeout_err.user_message();
        assert!(msg.contains("30000ms"));
        assert!(msg.contains("timed out"));
    }

    #[test]
    fn test_vm_error_retry_config() {
        let conn_err = VmError::ConnectionFailed {
            vm_id: "vm-123".into(),
            url: "http://localhost".into(),
        };
        assert_eq!(conn_err.suggested_retry_delay_ms(), Some(1000));
        assert_eq!(conn_err.max_retry_attempts(), 3);

        let pool_err = VmError::PoolExhausted;
        assert_eq!(pool_err.suggested_retry_delay_ms(), Some(5000));
        assert_eq!(pool_err.max_retry_attempts(), 5);

        let start_err = VmError::StartFailed { reason: "".into() };
        assert_eq!(start_err.suggested_retry_delay_ms(), None);
        assert_eq!(start_err.max_retry_attempts(), 0);
    }

    #[test]
    fn test_from_execution_error() {
        let exec_err = ExecutionError::ContainerStart {
            reason: "docker not running".into(),
            container_id: Some("container-123".into()),
        };
        let vm_err: VmError = exec_err.into();

        assert!(matches!(vm_err, VmError::Executor(_)));
        assert!(vm_err.is_execution_error());
    }

    #[test]
    fn test_into_service_error() {
        let not_found = VmError::NotFound {
            vm_id: "vm-missing".into(),
        };
        let svc_err: crate::ServiceError = not_found.into();
        assert!(matches!(svc_err, crate::ServiceError::NotFound(_)));

        let pool_err = VmError::PoolExhausted;
        let svc_err: crate::ServiceError = pool_err.into();
        assert!(matches!(svc_err, crate::ServiceError::RateLimited(_)));

        let timeout_err = VmError::RequestTimeout {
            vm_id: "vm-123".into(),
            timeout_ms: 30000,
        };
        let svc_err: crate::ServiceError = timeout_err.into();
        assert!(matches!(svc_err, crate::ServiceError::Timeout(_)));
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let vm_err: VmError = io_err.into();
        assert!(matches!(vm_err, VmError::IoError { .. }));
    }

    #[test]
    fn test_from_serde_error() {
        let json_str = "{ invalid json }";
        let serde_err = serde_json::from_str::<serde_json::Value>(json_str).unwrap_err();
        let vm_err: VmError = serde_err.into();
        assert!(matches!(vm_err, VmError::SerializationError { .. }));
    }

    #[test]
    fn test_vm_error_context() {
        let ctx = VmErrorContext::new()
            .with_vm_id("vm-123")
            .with_execution_id("exec-456")
            .with_operation("start")
            .with_metadata("region", "us-west-2");

        assert!(ctx.vm_id.is_some());
        assert!(ctx.execution_id.is_some());
        assert_eq!(ctx.operation, Some("start".to_string()));
        assert!(ctx.metadata.contains_key("region"));
    }

    #[test]
    fn test_vm_error_context_from_error() {
        let err = VmError::ExecutionFailed {
            vm_id: "vm-ctx".into(),
            execution_id: "exec-ctx".into(),
            error: "test error".into(),
        };
        let ctx = VmErrorContext::from_error(&err);

        assert_eq!(ctx.vm_id, Some("vm-ctx".to_string()));
        assert_eq!(ctx.execution_id, Some("exec-ctx".to_string()));
        assert!(ctx.metadata.contains_key("error_code"));
    }

    #[test]
    fn test_vm_error_serialization() {
        let err = VmError::ExecutionFailed {
            vm_id: "vm-serialize".into(),
            execution_id: "exec-serialize".into(),
            error: "test error".into(),
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("ExecutionFailed"));
        assert!(json.contains("vm-serialize"));

        let parsed: VmError = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, VmError::ExecutionFailed { .. }));
    }

    #[test]
    fn test_truncate_id() {
        assert_eq!(truncate_id("short"), "short");
        assert_eq!(truncate_id("12345678"), "12345678");
        assert_eq!(truncate_id("123456789"), "12345678...");
        assert_eq!(truncate_id("very-long-vm-identifier-string"), "very-lon...");
    }

    #[test]
    fn test_vm_error_display() {
        let err = VmError::ConnectionFailed {
            vm_id: "vm-display".into(),
            url: "http://localhost:8080".into(),
        };
        let display = err.to_string();
        assert!(display.contains("Connection to VM failed"));
        assert!(display.contains("vm-display"));
        assert!(display.contains("localhost:8080"));
    }

    #[test]
    fn test_vm_error_context_display() {
        let ctx = VmErrorContext::new()
            .with_vm_id("vm-display")
            .with_operation("test_op");

        let display = ctx.to_string();
        assert!(display.contains("error_id"));
        assert!(display.contains("vm_id: vm-display"));
        assert!(display.contains("operation: test_op"));
    }
}
