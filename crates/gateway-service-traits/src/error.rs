//! Unified service error type.
//!
//! All service traits return this error type, which can be converted to/from
//! crate-specific errors (gateway_llm::Error, gateway_core::Error, etc.).

use thiserror::Error;

/// Error type returned by all service boundary traits.
#[derive(Error, Debug)]
pub enum ServiceError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Rate limited")]
    RateLimited,

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Unavailable: {0}")]
    Unavailable(String),

    #[error("Internal: {0}")]
    Internal(String),
}

/// A specialized Result type for service operations.
pub type ServiceResult<T> = std::result::Result<T, ServiceError>;

impl From<gateway_llm::Error> for ServiceError {
    fn from(e: gateway_llm::Error) -> Self {
        match e {
            gateway_llm::Error::NotFound(msg) => ServiceError::NotFound(msg),
            gateway_llm::Error::ProfileNotFound(msg) => ServiceError::NotFound(msg),
            gateway_llm::Error::ProviderUnhealthy { provider, state } => {
                ServiceError::Unavailable(format!("{provider}: {state}"))
            }
            gateway_llm::Error::RoutingFailed(msg) => ServiceError::Unavailable(msg),
            gateway_llm::Error::StrategyConfig(msg) => ServiceError::InvalidInput(msg),
            gateway_llm::Error::Config(msg) => ServiceError::InvalidInput(msg),
            gateway_llm::Error::Http(e) => ServiceError::Internal(e.to_string()),
            gateway_llm::Error::Serialization(e) => ServiceError::Internal(e.to_string()),
            gateway_llm::Error::Io(e) => ServiceError::Internal(e.to_string()),
            gateway_llm::Error::Llm(msg) | gateway_llm::Error::Internal(msg) => {
                ServiceError::Internal(msg)
            }
        }
    }
}
