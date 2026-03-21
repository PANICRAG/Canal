//! Error types for the gateway core.

use thiserror::Error;

/// The main error type for the gateway core.
#[derive(Error, Debug)]
pub enum Error {
    // === LLM and MCP Errors ===
    #[error("LLM error: {0}")]
    Llm(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("Workflow error: {0}")]
    Workflow(String),

    // === Model Router Errors ===
    #[error("Profile not found: {0}")]
    ProfileNotFound(String),

    #[error("Provider unhealthy: {provider} - circuit is {state}")]
    ProviderUnhealthy { provider: String, state: String },

    #[error("Routing failed: {0}")]
    RoutingFailed(String),

    #[error("Strategy configuration error: {0}")]
    StrategyConfig(String),

    // === Infrastructure Errors ===
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    // === Access Control Errors ===
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Path blocked: {0}")]
    PathBlocked(String),

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Rate limited")]
    RateLimited,

    // === Execution Errors ===
    #[error("Execution timeout: {0}")]
    Timeout(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Command blocked: {0}")]
    CommandBlocked(String),

    #[error("Docker error: {0}")]
    Docker(String),

    #[error("Language not supported: {0}")]
    UnsupportedLanguage(String),

    // === Worker Orchestration Errors ===
    #[error("Worker error: {0}")]
    Worker(String),

    #[error("Worker timeout: worker '{name}' exceeded {timeout_secs}s")]
    WorkerTimeout { name: String, timeout_secs: u64 },

    #[error("Worker dependency cycle detected: {0}")]
    WorkerDependencyCycle(String),

    #[error("Worker budget exceeded: spent ${spent:.2}, limit ${limit:.2}")]
    WorkerBudgetExceeded { spent: f64, limit: f64 },

    // === Code Orchestration Errors ===
    #[error("Code orchestration error: {0}")]
    CodeOrchestration(String),

    #[error("Tool proxy error: {0}")]
    ToolProxy(String),

    #[error("Code orchestration timeout after {0}ms")]
    CodeOrchestrationTimeout(u64),

    #[error("Too many tool calls: {count} exceeds limit of {limit}")]
    TooManyToolCalls { count: usize, limit: usize },

    // === Validation Errors ===
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("File too large: {size} bytes exceeds limit of {limit} bytes")]
    FileTooLarge { size: u64, limit: u64 },

    // === Generic Errors ===
    #[error("Internal error: {0}")]
    Internal(String),
}

impl Error {
    /// Check if this error indicates a client error (4xx)
    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            Error::NotFound(_)
                | Error::PermissionDenied(_)
                | Error::PathBlocked(_)
                | Error::Unauthorized
                | Error::RateLimited
                | Error::CommandBlocked(_)
                | Error::UnsupportedLanguage(_)
                | Error::InvalidInput(_)
                | Error::FileTooLarge { .. }
                | Error::ProfileNotFound(_)
                | Error::StrategyConfig(_)
        )
    }

    /// Check if this error indicates a server error (5xx)
    pub fn is_server_error(&self) -> bool {
        matches!(
            self,
            Error::Database(_) | Error::Http(_) | Error::Docker(_) | Error::Internal(_)
        )
    }

    /// Check if this error is retriable
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            Error::RateLimited
                | Error::Timeout(_)
                | Error::Http(_)
                | Error::Database(_)
                | Error::WorkerTimeout { .. }
                | Error::ProviderUnhealthy { .. }
                | Error::RoutingFailed(_)
        )
    }

    /// Check if this error is a worker/orchestration error
    pub fn is_worker_error(&self) -> bool {
        matches!(
            self,
            Error::Worker(_)
                | Error::WorkerTimeout { .. }
                | Error::WorkerDependencyCycle(_)
                | Error::WorkerBudgetExceeded { .. }
        )
    }

    /// Check if this error is a code orchestration error
    pub fn is_code_orchestration_error(&self) -> bool {
        matches!(
            self,
            Error::CodeOrchestration(_)
                | Error::ToolProxy(_)
                | Error::CodeOrchestrationTimeout(_)
                | Error::TooManyToolCalls { .. }
        )
    }

    /// Check if this error is a model routing error
    pub fn is_routing_error(&self) -> bool {
        matches!(
            self,
            Error::ProfileNotFound(_)
                | Error::ProviderUnhealthy { .. }
                | Error::RoutingFailed(_)
                | Error::StrategyConfig(_)
        )
    }
}

/// Convert gateway-llm Error into gateway-core Error.
///
/// Maps LLM-specific error variants to their gateway-core equivalents.
impl From<gateway_llm::Error> for Error {
    fn from(e: gateway_llm::Error) -> Self {
        match e {
            gateway_llm::Error::Llm(msg) => Error::Llm(msg),
            gateway_llm::Error::ProfileNotFound(msg) => Error::ProfileNotFound(msg),
            gateway_llm::Error::ProviderUnhealthy { provider, state } => {
                Error::ProviderUnhealthy { provider, state }
            }
            gateway_llm::Error::RoutingFailed(msg) => Error::RoutingFailed(msg),
            gateway_llm::Error::StrategyConfig(msg) => Error::StrategyConfig(msg),
            gateway_llm::Error::Config(msg) => Error::Config(msg),
            gateway_llm::Error::NotFound(msg) => Error::NotFound(msg),
            gateway_llm::Error::Http(e) => Error::Http(e),
            gateway_llm::Error::Serialization(e) => Error::Serialization(e),
            gateway_llm::Error::Io(e) => Error::Io(e),
            gateway_llm::Error::Internal(msg) => Error::Internal(msg),
        }
    }
}

/// Convert gateway-memory Error into gateway-core Error.
impl From<gateway_memory::Error> for Error {
    fn from(e: gateway_memory::Error) -> Self {
        match e {
            gateway_memory::Error::NotFound(msg) => Error::NotFound(msg),
            gateway_memory::Error::Config(msg) => Error::Config(msg),
            gateway_memory::Error::Io(e) => Error::Io(e),
            gateway_memory::Error::Serialization(e) => Error::Serialization(e),
            gateway_memory::Error::Http(e) => Error::Http(e),
            gateway_memory::Error::Internal(msg) => Error::Internal(msg),
        }
    }
}

/// Convert VmError to gateway-core Error.
#[cfg(unix)]
impl From<gateway_tools::vm::VmError> for Error {
    fn from(err: gateway_tools::vm::VmError) -> Self {
        let svc_err: gateway_tools::ServiceError = err.into();
        svc_err.into()
    }
}

/// Convert gateway-tools ServiceError into gateway-core Error.
///
/// This enables seamless interop between gateway-tools modules (filesystem, executor)
/// and gateway-core code that uses `crate::error::Error`.
impl From<gateway_tools::ServiceError> for Error {
    fn from(e: gateway_tools::ServiceError) -> Self {
        match e {
            gateway_tools::ServiceError::NotFound(msg) => Error::NotFound(msg),
            gateway_tools::ServiceError::Internal(msg) => Error::Internal(msg),
            gateway_tools::ServiceError::Unsupported(msg) => Error::UnsupportedLanguage(msg),
            gateway_tools::ServiceError::UnsupportedLanguage(msg) => {
                Error::UnsupportedLanguage(msg)
            }
            gateway_tools::ServiceError::InvalidInput(msg) => Error::InvalidInput(msg),
            gateway_tools::ServiceError::Timeout(msg) => Error::Timeout(msg),
            gateway_tools::ServiceError::Docker(msg) => Error::Docker(msg),
            gateway_tools::ServiceError::Config(msg) => Error::Config(msg),
            gateway_tools::ServiceError::ExecutionFailed(msg) => Error::ExecutionFailed(msg),
            gateway_tools::ServiceError::CommandBlocked(msg) => Error::CommandBlocked(msg),
            gateway_tools::ServiceError::RateLimited(_) => Error::RateLimited,
            gateway_tools::ServiceError::Serialization(msg) => Error::Internal(msg),
            gateway_tools::ServiceError::Http(msg) => Error::Internal(msg),
            gateway_tools::ServiceError::PermissionDenied(msg) => Error::PermissionDenied(msg),
            gateway_tools::ServiceError::FileTooLarge { size, limit } => {
                Error::FileTooLarge { size, limit }
            }
            gateway_tools::ServiceError::Io(e) => Error::Io(e),
        }
    }
}

/// A specialized Result type for gateway operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_classification() {
        let not_found = Error::NotFound("test".into());
        assert!(not_found.is_client_error());
        assert!(!not_found.is_server_error());

        let internal = Error::Internal("test".into());
        assert!(!internal.is_client_error());
        assert!(internal.is_server_error());

        let rate_limited = Error::RateLimited;
        assert!(rate_limited.is_retriable());

        let permission = Error::PermissionDenied("test".into());
        assert!(!permission.is_retriable());
    }

    #[test]
    fn test_file_too_large_message() {
        let err = Error::FileTooLarge {
            size: 1000,
            limit: 500,
        };
        assert_eq!(
            err.to_string(),
            "File too large: 1000 bytes exceeds limit of 500 bytes"
        );
    }
}
