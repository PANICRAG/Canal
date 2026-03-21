//! RTE (Remote Tool Execution) error types.

use std::time::Duration;
use uuid::Uuid;

/// Errors that can occur during RTE protocol operations.
#[derive(Debug, thiserror::Error)]
pub enum RteError {
    /// Tool execution request not found in pending store
    #[error("pending request not found: {0}")]
    RequestNotFound(Uuid),

    /// HMAC signature verification failed
    #[error("HMAC signature verification failed for request {0}")]
    InvalidSignature(Uuid),

    /// Tool execution timed out
    #[error("tool execution timed out after {0:?} for request {1}")]
    Timeout(Duration, Uuid),

    /// Client does not support the requested tool
    #[error("client does not support tool: {0}")]
    UnsupportedTool(String),

    /// Session not found or expired
    #[error("RTE session not found: {0}")]
    SessionNotFound(Uuid),

    /// Client disconnected during tool execution
    #[error("client disconnected during tool execution: {0}")]
    ClientDisconnected(Uuid),

    /// Fallback execution failed
    #[error("fallback execution failed for tool {tool}: {reason}")]
    FallbackFailed { tool: String, reason: String },

    /// Maximum concurrent tool executions reached
    #[error("max concurrent tool executions reached ({0})")]
    ConcurrencyLimit(u32),

    /// Serialization error
    #[error("serialization error: {0}")]
    Serialization(String),
}
