//! Execution Error Types and Recovery Mechanisms
//!
//! This module provides comprehensive error handling for code execution,
//! including error categorization, recovery strategies, and user-friendly messages.
//!
//! # Error Categories
//!
//! - **Container Errors**: Issues with Docker container lifecycle
//! - **Code Execution Errors**: Python/Bash runtime errors
//! - **Resource Errors**: Memory, CPU, disk, and network limits
//! - **Security Errors**: Policy violations and blocked operations
//! - **Timeout Errors**: Execution and idle timeouts
//!
//! # Recovery Strategies
//!
//! Each error type has an associated recovery strategy that indicates
//! whether the error is recoverable and what action should be taken.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

/// Execution error types with detailed context
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionError {
    // === Container Errors ===
    /// Container failed to start
    #[error("Container failed to start: {reason}")]
    ContainerStart {
        reason: String,
        container_id: Option<String>,
    },

    /// Container execution timed out
    #[error("Container timed out after {timeout_ms}ms")]
    ContainerTimeout {
        timeout_ms: u64,
        container_id: String,
    },

    /// Container ran out of memory (OOM killed)
    #[error("Container killed due to memory limit ({memory_limit_mb}MB exceeded)")]
    ContainerOom {
        memory_limit_mb: u64,
        container_id: String,
    },

    /// Container was killed unexpectedly
    #[error("Container was killed: {reason}")]
    ContainerKilled {
        reason: String,
        container_id: String,
    },

    /// Container not found
    #[error("Container not found: {container_id}")]
    ContainerNotFound { container_id: String },

    /// Docker daemon connection failed
    #[error("Docker connection failed: {message}")]
    DockerConnection { message: String },

    /// Image pull failed
    #[error("Failed to pull image '{image}': {message}")]
    ImagePullFailed { image: String, message: String },

    // === Code Execution Errors ===
    /// Syntax error in the code
    #[error("Syntax error at line {line}, column {column}: {message}")]
    SyntaxError {
        line: usize,
        column: usize,
        message: String,
        code_snippet: Option<String>,
    },

    /// Runtime error during execution
    #[error("{error_type}: {message}")]
    RuntimeError {
        error_type: String,
        message: String,
        traceback: String,
    },

    /// Module import failed
    #[error("Import error: cannot import '{module}': {message}")]
    ImportError { module: String, message: String },

    /// Name/variable not defined
    #[error("Name error: '{name}' is not defined")]
    NameError { name: String, traceback: String },

    /// Type error in operation
    #[error("Type error: {message}")]
    TypeError { message: String, traceback: String },

    /// Value error in operation
    #[error("Value error: {message}")]
    ValueError { message: String, traceback: String },

    /// Index out of bounds
    #[error("Index error: {message}")]
    IndexError { message: String, traceback: String },

    /// Key not found in dictionary
    #[error("Key error: {key}")]
    KeyError { key: String, traceback: String },

    /// Attribute access failed
    #[error("Attribute error: '{object}' has no attribute '{attribute}'")]
    AttributeError {
        object: String,
        attribute: String,
        traceback: String,
    },

    /// Zero division error
    #[error("Division by zero")]
    ZeroDivisionError { traceback: String },

    /// Generic Python exception
    #[error("{exception_type}: {message}")]
    PythonException {
        exception_type: String,
        message: String,
        traceback: String,
    },

    // === Resource Errors ===
    /// Resource limit exceeded
    #[error("Resource limit exceeded: {resource} limit is {limit}, used {used}")]
    ResourceLimit {
        resource: String,
        limit: String,
        used: String,
    },

    /// CPU limit exceeded
    #[error("CPU limit exceeded: {limit_cores} cores")]
    CpuLimitExceeded { limit_cores: f64 },

    /// Memory limit exceeded (before OOM)
    #[error("Memory limit warning: {used_mb}MB of {limit_mb}MB used")]
    MemoryLimitWarning { used_mb: u64, limit_mb: u64 },

    /// Disk space exhausted
    #[error("Disk space exhausted in {path}")]
    DiskSpaceExhausted { path: String },

    /// Network error during execution
    #[error("Network error: {message}")]
    NetworkError { message: String },

    /// File system error
    #[error("File system error at '{path}': {message}")]
    FileSystemError { path: String, message: String },

    /// Too many open files
    #[error("Too many open files (limit: {limit})")]
    TooManyOpenFiles { limit: u64 },

    /// Process limit exceeded
    #[error("Process limit exceeded (limit: {limit})")]
    ProcessLimitExceeded { limit: u64 },

    // === Security Errors ===
    /// Security policy violation
    #[error("Security violation ({violation_type}): {message}")]
    SecurityViolation {
        violation_type: String,
        message: String,
    },

    /// Blocked import attempt
    #[error("Import blocked: '{module}' is not allowed for security reasons")]
    BlockedImport { module: String },

    /// Blocked command execution
    #[error("Command blocked: '{command}' is not allowed")]
    BlockedCommand { command: String },

    /// Blocked file access
    #[error("File access blocked: '{path}' is outside allowed directories")]
    BlockedFileAccess { path: String },

    /// Blocked network access
    #[error("Network access blocked: {reason}")]
    BlockedNetworkAccess { reason: String },

    /// Privilege escalation attempt
    #[error("Privilege escalation blocked: {attempt}")]
    PrivilegeEscalation { attempt: String },

    // === Timeout Errors ===
    /// Execution timeout
    #[error("Execution timed out after {timeout_ms}ms")]
    ExecutionTimeout { timeout_ms: u64 },

    /// Idle timeout (no activity)
    #[error("Idle timeout: no activity for {idle_duration_ms}ms")]
    IdleTimeout { idle_duration_ms: u64 },

    /// Startup timeout
    #[error("Container startup timed out after {timeout_ms}ms")]
    StartupTimeout { timeout_ms: u64 },

    // === Other Errors ===
    /// Language not supported
    #[error("Language not supported: {language}")]
    UnsupportedLanguage { language: String },

    /// Invalid code input
    #[error("Invalid code: {reason}")]
    InvalidCode { reason: String },

    /// Internal executor error
    #[error("Internal executor error: {message}")]
    Internal { message: String },

    /// Unknown error with raw output
    #[error("Execution failed: {message}")]
    Unknown {
        message: String,
        stdout: Option<String>,
        stderr: Option<String>,
        exit_code: Option<i32>,
    },
}

/// Recovery strategy for execution errors
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RecoveryStrategy {
    /// Retry the operation with exponential backoff
    Retry {
        max_attempts: u32,
        base_backoff_ms: u64,
    },

    /// Restart the container and retry
    RestartContainer,

    /// Use an alternate resource (e.g., different container pool)
    UseAlternateResource { resource_hint: String },

    /// Reduce resource requirements and retry
    ReduceResources { memory_factor: f64, cpu_factor: f64 },

    /// Split the work into smaller chunks
    SplitWork { max_chunk_size: Option<usize> },

    /// Wait and retry (for rate limiting or temporary issues)
    WaitAndRetry { wait_ms: u64 },

    /// Abort with a user-friendly message
    AbortWithMessage(String),

    /// Require user intervention
    RequireUserIntervention(String),

    /// Fix code automatically (for simple syntax errors)
    AutoFix { suggestion: String },

    /// No recovery possible
    NoRecovery,
}

impl ExecutionError {
    /// Get the recommended recovery strategy for this error
    pub fn recovery_strategy(&self) -> RecoveryStrategy {
        match self {
            // Container errors - often recoverable with restart
            ExecutionError::ContainerStart { .. } => RecoveryStrategy::Retry {
                max_attempts: 3,
                base_backoff_ms: 1000,
            },
            ExecutionError::ContainerTimeout { .. } => RecoveryStrategy::RestartContainer,
            ExecutionError::ContainerOom { .. } => RecoveryStrategy::ReduceResources {
                memory_factor: 0.8,
                cpu_factor: 1.0,
            },
            ExecutionError::ContainerKilled { .. } => RecoveryStrategy::RestartContainer,
            ExecutionError::ContainerNotFound { .. } => RecoveryStrategy::RestartContainer,
            ExecutionError::DockerConnection { .. } => RecoveryStrategy::Retry {
                max_attempts: 5,
                base_backoff_ms: 2000,
            },
            ExecutionError::ImagePullFailed { .. } => RecoveryStrategy::Retry {
                max_attempts: 3,
                base_backoff_ms: 5000,
            },

            // Code errors - usually require user intervention
            ExecutionError::SyntaxError { message, .. } => {
                RecoveryStrategy::RequireUserIntervention(format!(
                    "Please fix the syntax error: {}",
                    message
                ))
            }
            ExecutionError::RuntimeError { error_type, .. } => {
                RecoveryStrategy::RequireUserIntervention(format!(
                    "Runtime error occurred: {}. Please check your code logic.",
                    error_type
                ))
            }
            ExecutionError::ImportError { module, .. } => {
                RecoveryStrategy::RequireUserIntervention(format!(
                    "Module '{}' could not be imported. You may need to install it.",
                    module
                ))
            }
            ExecutionError::NameError { name, .. } => RecoveryStrategy::RequireUserIntervention(
                format!("Variable '{}' is not defined. Check for typos.", name),
            ),
            ExecutionError::TypeError { .. }
            | ExecutionError::ValueError { .. }
            | ExecutionError::IndexError { .. }
            | ExecutionError::KeyError { .. }
            | ExecutionError::AttributeError { .. }
            | ExecutionError::ZeroDivisionError { .. }
            | ExecutionError::PythonException { .. } => RecoveryStrategy::RequireUserIntervention(
                "Please review and fix the error in your code.".into(),
            ),

            // Resource errors - may be recoverable
            ExecutionError::ResourceLimit { resource, .. } => {
                RecoveryStrategy::AbortWithMessage(format!(
                    "Resource limit exceeded for {}. Try reducing your workload.",
                    resource
                ))
            }
            ExecutionError::CpuLimitExceeded { .. } => RecoveryStrategy::SplitWork {
                max_chunk_size: None,
            },
            ExecutionError::MemoryLimitWarning { .. } => RecoveryStrategy::ReduceResources {
                memory_factor: 0.7,
                cpu_factor: 1.0,
            },
            ExecutionError::DiskSpaceExhausted { .. } => RecoveryStrategy::AbortWithMessage(
                "Disk space exhausted. Clean up temporary files and try again.".into(),
            ),
            ExecutionError::NetworkError { .. } => RecoveryStrategy::Retry {
                max_attempts: 3,
                base_backoff_ms: 1000,
            },
            ExecutionError::FileSystemError { .. } => RecoveryStrategy::AbortWithMessage(
                "File system error occurred. Check file permissions and paths.".into(),
            ),
            ExecutionError::TooManyOpenFiles { .. } => RecoveryStrategy::RestartContainer,
            ExecutionError::ProcessLimitExceeded { .. } => RecoveryStrategy::RestartContainer,

            // Security errors - never recoverable automatically
            ExecutionError::SecurityViolation { .. }
            | ExecutionError::BlockedImport { .. }
            | ExecutionError::BlockedCommand { .. }
            | ExecutionError::BlockedFileAccess { .. }
            | ExecutionError::BlockedNetworkAccess { .. }
            | ExecutionError::PrivilegeEscalation { .. } => {
                RecoveryStrategy::AbortWithMessage("Operation blocked for security reasons.".into())
            }

            // Timeout errors
            ExecutionError::ExecutionTimeout { timeout_ms } => {
                if *timeout_ms < 300_000 {
                    // Less than 5 minutes
                    RecoveryStrategy::SplitWork {
                        max_chunk_size: Some(1000),
                    }
                } else {
                    RecoveryStrategy::AbortWithMessage(
                        "Execution took too long. Try breaking it into smaller tasks.".into(),
                    )
                }
            }
            ExecutionError::IdleTimeout { .. } => RecoveryStrategy::RestartContainer,
            ExecutionError::StartupTimeout { .. } => RecoveryStrategy::Retry {
                max_attempts: 2,
                base_backoff_ms: 3000,
            },

            // Other errors
            ExecutionError::UnsupportedLanguage { language } => {
                RecoveryStrategy::AbortWithMessage(format!(
                    "Language '{}' is not supported. Supported languages: Python, Bash, JavaScript, TypeScript, Go, Rust",
                    language
                ))
            }
            ExecutionError::InvalidCode { .. } => RecoveryStrategy::RequireUserIntervention(
                "The provided code is invalid. Please check the format.".into(),
            ),
            ExecutionError::Internal { .. } => RecoveryStrategy::Retry {
                max_attempts: 2,
                base_backoff_ms: 1000,
            },
            ExecutionError::Unknown { .. } => RecoveryStrategy::NoRecovery,
        }
    }

    /// Check if this error is recoverable
    pub fn is_recoverable(&self) -> bool {
        !matches!(
            self.recovery_strategy(),
            RecoveryStrategy::NoRecovery
                | RecoveryStrategy::AbortWithMessage(_)
                | RecoveryStrategy::RequireUserIntervention(_)
        )
    }

    /// Check if this error is a user error (vs system error)
    pub fn is_user_error(&self) -> bool {
        matches!(
            self,
            ExecutionError::SyntaxError { .. }
                | ExecutionError::RuntimeError { .. }
                | ExecutionError::ImportError { .. }
                | ExecutionError::NameError { .. }
                | ExecutionError::TypeError { .. }
                | ExecutionError::ValueError { .. }
                | ExecutionError::IndexError { .. }
                | ExecutionError::KeyError { .. }
                | ExecutionError::AttributeError { .. }
                | ExecutionError::ZeroDivisionError { .. }
                | ExecutionError::PythonException { .. }
                | ExecutionError::InvalidCode { .. }
        )
    }

    /// Check if this error is a security error
    pub fn is_security_error(&self) -> bool {
        matches!(
            self,
            ExecutionError::SecurityViolation { .. }
                | ExecutionError::BlockedImport { .. }
                | ExecutionError::BlockedCommand { .. }
                | ExecutionError::BlockedFileAccess { .. }
                | ExecutionError::BlockedNetworkAccess { .. }
                | ExecutionError::PrivilegeEscalation { .. }
        )
    }

    /// Check if this error is a resource error
    pub fn is_resource_error(&self) -> bool {
        matches!(
            self,
            ExecutionError::ResourceLimit { .. }
                | ExecutionError::CpuLimitExceeded { .. }
                | ExecutionError::MemoryLimitWarning { .. }
                | ExecutionError::ContainerOom { .. }
                | ExecutionError::DiskSpaceExhausted { .. }
                | ExecutionError::TooManyOpenFiles { .. }
                | ExecutionError::ProcessLimitExceeded { .. }
        )
    }

    /// Check if this error is a timeout error
    pub fn is_timeout_error(&self) -> bool {
        matches!(
            self,
            ExecutionError::ContainerTimeout { .. }
                | ExecutionError::ExecutionTimeout { .. }
                | ExecutionError::IdleTimeout { .. }
                | ExecutionError::StartupTimeout { .. }
        )
    }

    /// Get a user-friendly error message
    pub fn user_message(&self) -> String {
        match self {
            ExecutionError::SyntaxError {
                line,
                column,
                message,
                ..
            } => {
                format!(
                    "There's a syntax error in your code at line {}, column {}:\n{}",
                    line, column, message
                )
            }
            ExecutionError::RuntimeError {
                error_type,
                message,
                ..
            } => {
                format!("Your code encountered a {} error:\n{}", error_type, message)
            }
            ExecutionError::ImportError { module, message } => {
                format!(
                    "Could not import module '{}'. {}",
                    module,
                    if message.contains("No module named") {
                        "Make sure the module is installed."
                    } else {
                        message.as_str()
                    }
                )
            }
            ExecutionError::NameError { name, .. } => {
                format!(
                    "Variable '{}' is not defined. Did you forget to define it or made a typo?",
                    name
                )
            }
            ExecutionError::ContainerOom {
                memory_limit_mb, ..
            } => {
                format!(
                    "Your code ran out of memory (limit: {}MB). Try processing data in smaller chunks or using more memory-efficient algorithms.",
                    memory_limit_mb
                )
            }
            ExecutionError::ExecutionTimeout { timeout_ms } => {
                format!(
                    "Your code took too long to run (timeout: {}s). Consider optimizing your algorithm or processing less data at once.",
                    timeout_ms / 1000
                )
            }
            ExecutionError::BlockedImport { module } => {
                format!(
                    "The module '{}' is not allowed for security reasons. Please use an alternative approach.",
                    module
                )
            }
            ExecutionError::BlockedCommand { command } => {
                format!("The command '{}' is blocked for security reasons.", command)
            }
            ExecutionError::SecurityViolation { message, .. } => {
                format!("Security policy violation: {}", message)
            }
            ExecutionError::NetworkError { message } => {
                format!(
                    "Network error: {}. Please check your network configuration.",
                    message
                )
            }
            ExecutionError::FileSystemError { path, message } => {
                format!("File system error at '{}': {}", path, message)
            }
            // Default to the standard error message
            _ => self.to_string(),
        }
    }

    /// Get the traceback if available
    pub fn traceback(&self) -> Option<&str> {
        match self {
            ExecutionError::RuntimeError { traceback, .. }
            | ExecutionError::NameError { traceback, .. }
            | ExecutionError::TypeError { traceback, .. }
            | ExecutionError::ValueError { traceback, .. }
            | ExecutionError::IndexError { traceback, .. }
            | ExecutionError::KeyError { traceback, .. }
            | ExecutionError::AttributeError { traceback, .. }
            | ExecutionError::ZeroDivisionError { traceback }
            | ExecutionError::PythonException { traceback, .. } => Some(traceback.as_str()),
            _ => None,
        }
    }

    /// Get error code for categorization
    pub fn error_code(&self) -> &'static str {
        match self {
            ExecutionError::ContainerStart { .. } => "E_CONTAINER_START",
            ExecutionError::ContainerTimeout { .. } => "E_CONTAINER_TIMEOUT",
            ExecutionError::ContainerOom { .. } => "E_CONTAINER_OOM",
            ExecutionError::ContainerKilled { .. } => "E_CONTAINER_KILLED",
            ExecutionError::ContainerNotFound { .. } => "E_CONTAINER_NOT_FOUND",
            ExecutionError::DockerConnection { .. } => "E_DOCKER_CONNECTION",
            ExecutionError::ImagePullFailed { .. } => "E_IMAGE_PULL_FAILED",
            ExecutionError::SyntaxError { .. } => "E_SYNTAX_ERROR",
            ExecutionError::RuntimeError { .. } => "E_RUNTIME_ERROR",
            ExecutionError::ImportError { .. } => "E_IMPORT_ERROR",
            ExecutionError::NameError { .. } => "E_NAME_ERROR",
            ExecutionError::TypeError { .. } => "E_TYPE_ERROR",
            ExecutionError::ValueError { .. } => "E_VALUE_ERROR",
            ExecutionError::IndexError { .. } => "E_INDEX_ERROR",
            ExecutionError::KeyError { .. } => "E_KEY_ERROR",
            ExecutionError::AttributeError { .. } => "E_ATTRIBUTE_ERROR",
            ExecutionError::ZeroDivisionError { .. } => "E_ZERO_DIVISION",
            ExecutionError::PythonException { .. } => "E_PYTHON_EXCEPTION",
            ExecutionError::ResourceLimit { .. } => "E_RESOURCE_LIMIT",
            ExecutionError::CpuLimitExceeded { .. } => "E_CPU_LIMIT",
            ExecutionError::MemoryLimitWarning { .. } => "E_MEMORY_WARNING",
            ExecutionError::DiskSpaceExhausted { .. } => "E_DISK_SPACE",
            ExecutionError::NetworkError { .. } => "E_NETWORK_ERROR",
            ExecutionError::FileSystemError { .. } => "E_FILESYSTEM_ERROR",
            ExecutionError::TooManyOpenFiles { .. } => "E_TOO_MANY_FILES",
            ExecutionError::ProcessLimitExceeded { .. } => "E_PROCESS_LIMIT",
            ExecutionError::SecurityViolation { .. } => "E_SECURITY_VIOLATION",
            ExecutionError::BlockedImport { .. } => "E_BLOCKED_IMPORT",
            ExecutionError::BlockedCommand { .. } => "E_BLOCKED_COMMAND",
            ExecutionError::BlockedFileAccess { .. } => "E_BLOCKED_FILE_ACCESS",
            ExecutionError::BlockedNetworkAccess { .. } => "E_BLOCKED_NETWORK",
            ExecutionError::PrivilegeEscalation { .. } => "E_PRIVILEGE_ESCALATION",
            ExecutionError::ExecutionTimeout { .. } => "E_EXECUTION_TIMEOUT",
            ExecutionError::IdleTimeout { .. } => "E_IDLE_TIMEOUT",
            ExecutionError::StartupTimeout { .. } => "E_STARTUP_TIMEOUT",
            ExecutionError::UnsupportedLanguage { .. } => "E_UNSUPPORTED_LANGUAGE",
            ExecutionError::InvalidCode { .. } => "E_INVALID_CODE",
            ExecutionError::Internal { .. } => "E_INTERNAL",
            ExecutionError::Unknown { .. } => "E_UNKNOWN",
        }
    }
}

/// Action to take after handling an error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryAction {
    /// Retry the execution
    Retry {
        attempt: u32,
        max_attempts: u32,
        backoff_ms: u64,
    },
    /// Restart container and retry
    RestartAndRetry { container_id: String },
    /// Abort execution
    Abort { reason: String },
    /// Continue with modified parameters
    ContinueWithModification {
        modification: String,
        new_params: serde_json::Value,
    },
    /// Ask user for input
    AskUser { prompt: String },
    /// Complete (no action needed)
    Complete,
}

/// Error handler for managing execution errors
pub struct ErrorHandler {
    /// Maximum retry attempts
    max_retries: u32,
    /// Base backoff duration for retries
    retry_backoff: Duration,
    /// Current retry count per error type
    retry_counts: std::sync::Mutex<std::collections::HashMap<String, u32>>,
}

impl Default for ErrorHandler {
    fn default() -> Self {
        Self::new(3, Duration::from_secs(1))
    }
}

impl ErrorHandler {
    /// Create a new error handler
    pub fn new(max_retries: u32, retry_backoff: Duration) -> Self {
        Self {
            max_retries,
            retry_backoff,
            retry_counts: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Get the base retry backoff duration
    pub fn retry_backoff(&self) -> Duration {
        self.retry_backoff
    }

    /// Get the maximum retry count
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// Handle an execution error and determine the recovery action
    pub async fn handle(&self, error: &ExecutionError) -> RecoveryAction {
        let error_code = error.error_code();
        let attempt = self.increment_retry_count(error_code);

        match error.recovery_strategy() {
            RecoveryStrategy::Retry {
                max_attempts,
                base_backoff_ms,
            } => {
                let effective_max = max_attempts.min(self.max_retries);
                if attempt <= effective_max {
                    let backoff = self.calculate_backoff(attempt, base_backoff_ms);
                    RecoveryAction::Retry {
                        attempt,
                        max_attempts: effective_max,
                        backoff_ms: backoff,
                    }
                } else {
                    self.reset_retry_count(error_code);
                    RecoveryAction::Abort {
                        reason: format!(
                            "Maximum retry attempts ({}) exceeded for {}",
                            effective_max,
                            error.user_message()
                        ),
                    }
                }
            }
            RecoveryStrategy::RestartContainer => {
                if attempt <= 2 {
                    if let Some(container_id) = self.extract_container_id(error) {
                        RecoveryAction::RestartAndRetry { container_id }
                    } else {
                        RecoveryAction::Abort {
                            reason: "Container restart required but container ID unknown".into(),
                        }
                    }
                } else {
                    self.reset_retry_count(error_code);
                    RecoveryAction::Abort {
                        reason: "Container restart attempts exhausted".into(),
                    }
                }
            }
            RecoveryStrategy::WaitAndRetry { wait_ms } => {
                if attempt <= self.max_retries {
                    RecoveryAction::Retry {
                        attempt,
                        max_attempts: self.max_retries,
                        backoff_ms: wait_ms,
                    }
                } else {
                    self.reset_retry_count(error_code);
                    RecoveryAction::Abort {
                        reason: "Wait and retry attempts exhausted".into(),
                    }
                }
            }
            RecoveryStrategy::ReduceResources {
                memory_factor,
                cpu_factor,
            } => {
                if attempt <= 2 {
                    RecoveryAction::ContinueWithModification {
                        modification: "Reduced resource limits".into(),
                        new_params: serde_json::json!({
                            "memory_factor": memory_factor.powi(attempt as i32),
                            "cpu_factor": cpu_factor.powi(attempt as i32),
                        }),
                    }
                } else {
                    self.reset_retry_count(error_code);
                    RecoveryAction::Abort {
                        reason: "Cannot reduce resources further".into(),
                    }
                }
            }
            RecoveryStrategy::SplitWork { max_chunk_size } => RecoveryAction::AskUser {
                prompt: format!(
                    "The operation is too large. Would you like to split it into smaller chunks{}?",
                    max_chunk_size
                        .map(|s| format!(" (max {} items)", s))
                        .unwrap_or_default()
                ),
            },
            RecoveryStrategy::UseAlternateResource { resource_hint } => {
                RecoveryAction::ContinueWithModification {
                    modification: format!("Switching to alternate resource: {}", resource_hint),
                    new_params: serde_json::json!({
                        "alternate_resource": resource_hint,
                    }),
                }
            }
            RecoveryStrategy::AutoFix { suggestion } => RecoveryAction::AskUser {
                prompt: format!("Suggested fix: {}. Apply this fix?", suggestion),
            },
            RecoveryStrategy::AbortWithMessage(msg) => {
                self.reset_retry_count(error_code);
                RecoveryAction::Abort { reason: msg }
            }
            RecoveryStrategy::RequireUserIntervention(msg) => RecoveryAction::AskUser {
                prompt: format!("{}\n\n{}", error.user_message(), msg),
            },
            RecoveryStrategy::NoRecovery => {
                self.reset_retry_count(error_code);
                RecoveryAction::Abort {
                    reason: error.user_message(),
                }
            }
        }
    }

    /// Check if we should retry for the given error and attempt number
    pub fn should_retry(&self, error: &ExecutionError, attempt: u32) -> bool {
        if !error.is_recoverable() {
            return false;
        }

        match error.recovery_strategy() {
            RecoveryStrategy::Retry { max_attempts, .. } => {
                attempt < max_attempts && attempt < self.max_retries
            }
            RecoveryStrategy::RestartContainer => attempt < 3,
            RecoveryStrategy::WaitAndRetry { .. } => attempt < self.max_retries,
            _ => false,
        }
    }

    /// Calculate backoff duration with exponential increase
    fn calculate_backoff(&self, attempt: u32, base_ms: u64) -> u64 {
        // Use handler's default backoff if base_ms is 0
        let effective_base = if base_ms == 0 {
            self.retry_backoff.as_millis() as u64
        } else {
            base_ms
        };
        // Exponential backoff with jitter
        let exponential = effective_base * 2_u64.pow(attempt.saturating_sub(1));
        let jitter = (exponential as f64 * 0.1 * rand_factor()) as u64;
        exponential.saturating_add(jitter).min(60_000) // Cap at 60 seconds
    }

    /// Increment retry count for an error type
    fn increment_retry_count(&self, error_code: &str) -> u32 {
        let mut counts = self.retry_counts.lock().unwrap();
        let count = counts.entry(error_code.to_string()).or_insert(0);
        *count += 1;
        *count
    }

    /// Reset retry count for an error type
    fn reset_retry_count(&self, error_code: &str) {
        let mut counts = self.retry_counts.lock().unwrap();
        counts.remove(error_code);
    }

    /// Extract container ID from error if present
    fn extract_container_id(&self, error: &ExecutionError) -> Option<String> {
        match error {
            ExecutionError::ContainerStart { container_id, .. } => container_id.clone(),
            ExecutionError::ContainerTimeout { container_id, .. }
            | ExecutionError::ContainerOom { container_id, .. }
            | ExecutionError::ContainerKilled { container_id, .. }
            | ExecutionError::ContainerNotFound { container_id } => Some(container_id.clone()),
            _ => None,
        }
    }
}

/// Generate a pseudo-random factor between 0 and 1 for jitter
fn rand_factor() -> f64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1000) as f64 / 1000.0
}

/// Parse Python stderr to extract structured error information
pub fn parse_python_error(stderr: &str, exit_code: i32) -> ExecutionError {
    let stderr = stderr.trim();

    // Try to extract traceback
    let traceback = if stderr.contains("Traceback (most recent call last):") {
        stderr.to_string()
    } else {
        String::new()
    };

    // Parse different error types
    if let Some(err) = parse_syntax_error(stderr) {
        return err;
    }

    if let Some(err) = parse_name_error(stderr, &traceback) {
        return err;
    }

    if let Some(err) = parse_import_error(stderr, &traceback) {
        return err;
    }

    if let Some(err) = parse_type_error(stderr, &traceback) {
        return err;
    }

    if let Some(err) = parse_value_error(stderr, &traceback) {
        return err;
    }

    if let Some(err) = parse_index_error(stderr, &traceback) {
        return err;
    }

    if let Some(err) = parse_key_error(stderr, &traceback) {
        return err;
    }

    if let Some(err) = parse_attribute_error(stderr, &traceback) {
        return err;
    }

    if stderr.contains("ZeroDivisionError") {
        return ExecutionError::ZeroDivisionError { traceback };
    }

    // Check for OOM
    if stderr.contains("MemoryError") || stderr.contains("Cannot allocate memory") {
        return ExecutionError::ContainerOom {
            memory_limit_mb: 0, // Unknown
            container_id: String::new(),
        };
    }

    // Generic Python exception
    if let Some(exception_match) = extract_exception(stderr) {
        return ExecutionError::PythonException {
            exception_type: exception_match.0,
            message: exception_match.1,
            traceback,
        };
    }

    // Unknown error
    ExecutionError::Unknown {
        message: if stderr.is_empty() {
            format!("Execution failed with exit code {}", exit_code)
        } else {
            stderr.lines().last().unwrap_or(stderr).to_string()
        },
        stdout: None,
        stderr: Some(stderr.to_string()),
        exit_code: Some(exit_code),
    }
}

fn parse_syntax_error(stderr: &str) -> Option<ExecutionError> {
    // Look for SyntaxError pattern
    if !stderr.contains("SyntaxError") {
        return None;
    }

    let mut line = 1;
    let column = 1;
    let mut message = String::new();

    for text_line in stderr.lines() {
        if text_line.contains("line ") {
            if let Some(num) = text_line
                .split("line ")
                .nth(1)
                .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|s| s.parse().ok())
            {
                line = num;
            }
        }
        if text_line.starts_with("SyntaxError:") {
            message = text_line
                .strip_prefix("SyntaxError:")
                .unwrap_or("")
                .trim()
                .to_string();
        }
    }

    Some(ExecutionError::SyntaxError {
        line,
        column,
        message: if message.is_empty() {
            "Invalid syntax".to_string()
        } else {
            message
        },
        code_snippet: None,
    })
}

fn parse_name_error(stderr: &str, traceback: &str) -> Option<ExecutionError> {
    if !stderr.contains("NameError") {
        return None;
    }

    let name = stderr
        .lines()
        .find(|l| l.contains("NameError"))
        .and_then(|l| l.split('\'').nth(1).map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string());

    Some(ExecutionError::NameError {
        name,
        traceback: traceback.to_string(),
    })
}

fn parse_import_error(stderr: &str, _traceback: &str) -> Option<ExecutionError> {
    if !stderr.contains("ImportError") && !stderr.contains("ModuleNotFoundError") {
        return None;
    }

    let module = stderr
        .lines()
        .find(|l| l.contains("ImportError") || l.contains("ModuleNotFoundError"))
        .and_then(|l| l.split('\'').nth(1).map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string());

    let message = stderr
        .lines()
        .find(|l| l.contains("ImportError") || l.contains("ModuleNotFoundError"))
        .map(|l| l.to_string())
        .unwrap_or_default();

    Some(ExecutionError::ImportError { module, message })
}

fn parse_type_error(stderr: &str, traceback: &str) -> Option<ExecutionError> {
    if !stderr.contains("TypeError") {
        return None;
    }

    let message = stderr
        .lines()
        .find(|l| l.contains("TypeError"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "Type error".to_string());

    Some(ExecutionError::TypeError {
        message,
        traceback: traceback.to_string(),
    })
}

fn parse_value_error(stderr: &str, traceback: &str) -> Option<ExecutionError> {
    if !stderr.contains("ValueError") {
        return None;
    }

    let message = stderr
        .lines()
        .find(|l| l.contains("ValueError"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "Value error".to_string());

    Some(ExecutionError::ValueError {
        message,
        traceback: traceback.to_string(),
    })
}

fn parse_index_error(stderr: &str, traceback: &str) -> Option<ExecutionError> {
    if !stderr.contains("IndexError") {
        return None;
    }

    let message = stderr
        .lines()
        .find(|l| l.contains("IndexError"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "Index out of range".to_string());

    Some(ExecutionError::IndexError {
        message,
        traceback: traceback.to_string(),
    })
}

fn parse_key_error(stderr: &str, traceback: &str) -> Option<ExecutionError> {
    if !stderr.contains("KeyError") {
        return None;
    }

    let key = stderr
        .lines()
        .find(|l| l.contains("KeyError"))
        .and_then(|l| l.split('\'').nth(1).map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string());

    Some(ExecutionError::KeyError {
        key,
        traceback: traceback.to_string(),
    })
}

fn parse_attribute_error(stderr: &str, traceback: &str) -> Option<ExecutionError> {
    if !stderr.contains("AttributeError") {
        return None;
    }

    // Try to parse: "'type' object has no attribute 'attr'"
    let parts: Vec<&str> = stderr
        .lines()
        .find(|l| l.contains("AttributeError"))
        .map(|l| l.split('\'').collect())
        .unwrap_or_default();

    let (object, attribute) = if parts.len() >= 4 {
        (parts[1].to_string(), parts[3].to_string())
    } else {
        ("unknown".to_string(), "unknown".to_string())
    };

    Some(ExecutionError::AttributeError {
        object,
        attribute,
        traceback: traceback.to_string(),
    })
}

fn extract_exception(stderr: &str) -> Option<(String, String)> {
    // Look for pattern: ExceptionType: message
    for line in stderr.lines().rev() {
        let line = line.trim();
        if let Some(colon_pos) = line.find(':') {
            let exception_type = &line[..colon_pos];
            // Check if it looks like an exception type (starts with capital, ends with Error/Exception)
            if exception_type
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
                && (exception_type.ends_with("Error") || exception_type.ends_with("Exception"))
            {
                let message = line[colon_pos + 1..].trim().to_string();
                return Some((exception_type.to_string(), message));
            }
        }
    }
    None
}

/// Context for error reporting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorContext {
    /// Unique error ID for tracking
    pub error_id: String,
    /// Timestamp when error occurred
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Execution ID that caused the error
    pub execution_id: Option<String>,
    /// Container ID involved
    pub container_id: Option<String>,
    /// Language being executed
    pub language: Option<String>,
    /// Code that caused the error (first 500 chars)
    pub code_preview: Option<String>,
    /// Additional metadata
    pub metadata: std::collections::HashMap<String, String>,
}

impl ErrorContext {
    /// Create a new error context
    pub fn new() -> Self {
        Self {
            error_id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now(),
            execution_id: None,
            container_id: None,
            language: None,
            code_preview: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Set execution ID
    pub fn with_execution_id(mut self, id: impl Into<String>) -> Self {
        self.execution_id = Some(id.into());
        self
    }

    /// Set container ID
    pub fn with_container_id(mut self, id: impl Into<String>) -> Self {
        self.container_id = Some(id.into());
        self
    }

    /// Set language
    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        self.language = Some(lang.into());
        self
    }

    /// Set code preview (truncated to 500 chars, safe at char boundary)
    pub fn with_code(mut self, code: &str) -> Self {
        self.code_preview = Some(if code.len() > 500 {
            let safe_end = code
                .char_indices()
                .take_while(|(i, _)| *i < 500)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(code.len().min(500));
            format!("{}...", &code[..safe_end])
        } else {
            code.to_string()
        });
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

impl Default for ErrorContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_categorization() {
        let syntax_error = ExecutionError::SyntaxError {
            line: 1,
            column: 5,
            message: "invalid syntax".into(),
            code_snippet: None,
        };
        assert!(syntax_error.is_user_error());
        assert!(!syntax_error.is_security_error());
        assert!(!syntax_error.is_resource_error());
        assert!(!syntax_error.is_recoverable());

        let oom_error = ExecutionError::ContainerOom {
            memory_limit_mb: 512,
            container_id: "abc123".into(),
        };
        assert!(!oom_error.is_user_error());
        assert!(oom_error.is_resource_error());
        assert!(oom_error.is_recoverable());

        let security_error = ExecutionError::BlockedImport {
            module: "subprocess".into(),
        };
        assert!(security_error.is_security_error());
        assert!(!security_error.is_recoverable());

        let timeout_error = ExecutionError::ExecutionTimeout { timeout_ms: 30000 };
        assert!(timeout_error.is_timeout_error());
    }

    #[test]
    fn test_recovery_strategy_selection() {
        let container_start = ExecutionError::ContainerStart {
            reason: "image not found".into(),
            container_id: None,
        };
        assert!(matches!(
            container_start.recovery_strategy(),
            RecoveryStrategy::Retry { .. }
        ));

        let container_oom = ExecutionError::ContainerOom {
            memory_limit_mb: 512,
            container_id: "abc123".into(),
        };
        assert!(matches!(
            container_oom.recovery_strategy(),
            RecoveryStrategy::ReduceResources { .. }
        ));

        let blocked_import = ExecutionError::BlockedImport {
            module: "os".into(),
        };
        assert!(matches!(
            blocked_import.recovery_strategy(),
            RecoveryStrategy::AbortWithMessage(_)
        ));
    }

    #[test]
    fn test_user_friendly_messages() {
        let name_error = ExecutionError::NameError {
            name: "undefined_var".into(),
            traceback: "...".into(),
        };
        let msg = name_error.user_message();
        assert!(msg.contains("undefined_var"));
        assert!(msg.contains("not defined"));

        let oom = ExecutionError::ContainerOom {
            memory_limit_mb: 256,
            container_id: "test".into(),
        };
        let msg = oom.user_message();
        assert!(msg.contains("256MB"));
        assert!(msg.contains("memory"));
    }

    #[test]
    fn test_error_codes() {
        assert_eq!(
            ExecutionError::SyntaxError {
                line: 1,
                column: 1,
                message: "".into(),
                code_snippet: None
            }
            .error_code(),
            "E_SYNTAX_ERROR"
        );
        assert_eq!(
            ExecutionError::ContainerOom {
                memory_limit_mb: 0,
                container_id: "".into()
            }
            .error_code(),
            "E_CONTAINER_OOM"
        );
    }

    #[test]
    fn test_error_handler_should_retry() {
        let handler = ErrorHandler::new(3, Duration::from_secs(1));

        let container_error = ExecutionError::ContainerStart {
            reason: "test".into(),
            container_id: None,
        };
        assert!(handler.should_retry(&container_error, 1));
        assert!(handler.should_retry(&container_error, 2));
        assert!(!handler.should_retry(&container_error, 3));

        let security_error = ExecutionError::BlockedImport {
            module: "os".into(),
        };
        assert!(!handler.should_retry(&security_error, 1));
    }

    #[tokio::test]
    async fn test_error_handler_handle() {
        let handler = ErrorHandler::default();

        // Test retry action
        let error = ExecutionError::DockerConnection {
            message: "connection refused".into(),
        };
        let action = handler.handle(&error).await;
        assert!(matches!(action, RecoveryAction::Retry { attempt: 1, .. }));

        // Second attempt
        let action = handler.handle(&error).await;
        assert!(matches!(action, RecoveryAction::Retry { attempt: 2, .. }));
    }

    #[test]
    fn test_parse_python_syntax_error() {
        let stderr = r#"
  File "test.py", line 5
    print("hello"
                 ^
SyntaxError: unexpected EOF while parsing
"#;

        let error = parse_python_error(stderr, 1);
        assert!(matches!(error, ExecutionError::SyntaxError { line: 5, .. }));
    }

    #[test]
    fn test_parse_python_name_error() {
        let stderr = r#"
Traceback (most recent call last):
  File "test.py", line 1, in <module>
    print(undefined_var)
NameError: name 'undefined_var' is not defined
"#;

        let error = parse_python_error(stderr, 1);
        assert!(matches!(
            error,
            ExecutionError::NameError { name, .. } if name == "undefined_var"
        ));
    }

    #[test]
    fn test_parse_python_import_error() {
        let stderr = r#"
Traceback (most recent call last):
  File "test.py", line 1, in <module>
    import nonexistent_module
ModuleNotFoundError: No module named 'nonexistent_module'
"#;

        let error = parse_python_error(stderr, 1);
        assert!(matches!(
            error,
            ExecutionError::ImportError { module, .. } if module == "nonexistent_module"
        ));
    }

    #[test]
    fn test_parse_python_key_error() {
        let stderr = r#"
Traceback (most recent call last):
  File "test.py", line 1, in <module>
    d['missing']
KeyError: 'missing'
"#;

        let error = parse_python_error(stderr, 1);
        assert!(matches!(
            error,
            ExecutionError::KeyError { key, .. } if key == "missing"
        ));
    }

    #[test]
    fn test_parse_python_zero_division() {
        let stderr = r#"
Traceback (most recent call last):
  File "test.py", line 1, in <module>
    1/0
ZeroDivisionError: division by zero
"#;

        let error = parse_python_error(stderr, 1);
        assert!(matches!(error, ExecutionError::ZeroDivisionError { .. }));
    }

    #[test]
    fn test_error_context_builder() {
        let ctx = ErrorContext::new()
            .with_execution_id("exec-123")
            .with_container_id("container-456")
            .with_language("python")
            .with_code("print('hello')")
            .with_metadata("attempt", "1");

        assert!(ctx.execution_id.is_some());
        assert!(ctx.container_id.is_some());
        assert_eq!(ctx.language.as_deref(), Some("python"));
        assert!(ctx.metadata.contains_key("attempt"));
    }

    #[test]
    fn test_recovery_strategy_serialization() {
        let strategy = RecoveryStrategy::Retry {
            max_attempts: 3,
            base_backoff_ms: 1000,
        };
        let json = serde_json::to_string(&strategy).unwrap();
        let parsed: RecoveryStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(strategy, parsed);
    }

    #[test]
    fn test_execution_error_serialization() {
        let error = ExecutionError::SyntaxError {
            line: 10,
            column: 5,
            message: "unexpected indent".into(),
            code_snippet: Some("    print('test')".into()),
        };
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("SyntaxError"));
        assert!(json.contains("unexpected indent"));
    }

    #[test]
    fn test_traceback_extraction() {
        let error = ExecutionError::RuntimeError {
            error_type: "ValueError".into(),
            message: "invalid value".into(),
            traceback: "Traceback (most recent call last):\n  File...".into(),
        };
        assert!(error.traceback().is_some());

        let container_error = ExecutionError::ContainerStart {
            reason: "test".into(),
            container_id: None,
        };
        assert!(container_error.traceback().is_none());
    }
}
