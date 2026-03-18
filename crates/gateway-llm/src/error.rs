//! Error types for the LLM routing crate.

use thiserror::Error;

/// LLM routing error type.
///
/// Mirrors the LLM-relevant subset of `gateway_core::Error` so that
/// moved source files compile with `use crate::error::{Error, Result}` unchanged.
#[derive(Error, Debug)]
pub enum Error {
    #[error("LLM error: {0}")]
    Llm(String),

    #[error("Profile not found: {0}")]
    ProfileNotFound(String),

    #[error("Provider unhealthy: {provider} - circuit is {state}")]
    ProviderUnhealthy { provider: String, state: String },

    #[error("Routing failed: {0}")]
    RoutingFailed(String),

    #[error("Strategy configuration error: {0}")]
    StrategyConfig(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// A specialized Result type for LLM operations.
pub type Result<T> = std::result::Result<T, Error>;
