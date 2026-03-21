//! Session management module
//!
//! Provides session persistence, checkpointing, and state management
//! for user sessions with isolated container environments.

pub mod checkpoint;
pub mod repository;

pub use checkpoint::{Checkpoint, CheckpointManager, CheckpointType};
pub use repository::{SessionRepository, SessionState, SessionStatus};
