//! Error types for the memory crate.

use thiserror::Error;

/// Memory crate error type.
///
/// Covers memory persistence, cache, and embedding errors.
#[derive(Error, Debug)]
pub enum Error {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// A specialized Result type for memory operations.
pub type Result<T> = std::result::Result<T, Error>;
