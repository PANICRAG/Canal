//! Error types for gateway-tools modules.
//!
//! Mirrors the subset of `gateway-core::error::Error` used by filesystem
//! and executor modules, enabling them to compile independently.

/// Error type for tool service operations
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// Resource not found
    #[error("Not found: {0}")]
    NotFound(String),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),

    /// Unsupported operation or language
    #[error("Unsupported: {0}")]
    Unsupported(String),

    /// Unsupported language (executor-specific)
    #[error("Unsupported language: {0}")]
    UnsupportedLanguage(String),

    /// Invalid input
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Timeout
    #[error("Timeout: {0}")]
    Timeout(String),

    /// Docker error
    #[error("Docker error: {0}")]
    Docker(String),

    /// Configuration error
    #[error("Config error: {0}")]
    Config(String),

    /// Execution failed
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    /// Command blocked (security)
    #[error("Command blocked: {0}")]
    CommandBlocked(String),

    /// Rate limited
    #[error("Rate limited: {0}")]
    RateLimited(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// HTTP error
    #[error("HTTP error: {0}")]
    Http(String),

    /// Permission denied
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    /// File too large
    #[error("File too large: {size} bytes exceeds limit of {limit} bytes")]
    FileTooLarge { size: u64, limit: u64 },

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type alias using ServiceError
pub type ServiceResult<T> = Result<T, ServiceError>;
