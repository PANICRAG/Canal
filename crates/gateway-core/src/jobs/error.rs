//! Error types for the async job system.

use uuid::Uuid;

/// Errors that can occur in the job system.
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("job not found: {0}")]
    NotFound(Uuid),

    #[error("invalid status transition from {from} to {to}")]
    InvalidTransition { from: String, to: String },

    #[error("job already cancelled: {0}")]
    AlreadyCancelled(Uuid),

    #[error("scheduler is full, max concurrent jobs reached")]
    SchedulerFull,

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("execution error: {0}")]
    Execution(String),

    #[error("job cancelled: {0}")]
    Cancelled(Uuid),

    #[error("job paused: {0}")]
    Paused(Uuid),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
