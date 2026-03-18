//! Session Management - Claude Agent SDK Compatible
//!
//! Provides session persistence, resumption, forking, and checkpoint/restore functionality.
//!
//! # Checkpoint System
//!
//! The checkpoint system enables saving and restoring session states, providing:
//! - Point-in-time snapshots for recovery
//! - Auto-checkpointing before dangerous operations
//! - Periodic checkpoints based on turn count
//! - Session restoration across restarts
//!
//! ```rust,ignore
//! use gateway_core::agent::session::{Checkpoint, CheckpointManager, FileCheckpointManager};
//!
//! // Create checkpoint manager
//! let manager = FileCheckpointManager::new("/path/to/checkpoints");
//!
//! // Save checkpoint
//! let checkpoint = Checkpoint::new("session-1", messages, context_state);
//! let id = manager.save(&checkpoint).await?;
//!
//! // Restore from checkpoint
//! let checkpoint = manager.load(&id).await?;
//! let restored = restore_from_checkpoint(checkpoint)?;
//! ```

pub mod checkpoint;
pub mod compact;
pub mod manager;
pub mod rollback;
pub mod storage;

pub use checkpoint::{
    restore_from_checkpoint,
    // Auto-checkpoint configuration
    AutoCheckpointConfig,
    AutoCheckpointTrigger,
    // Core types
    Checkpoint,
    // Errors
    CheckpointError,
    // Manager trait and implementations
    CheckpointManager,
    CheckpointMetadata,
    CheckpointTrigger,
    ContextState,
    DangerousOperation,
    FileCheckpointManager,
    MemoryCheckpointManager,
    // Session restore
    RestoreResult,
    CHECKPOINT_SCHEMA_VERSION,
};
pub use compact::{
    CompactConfig, CompactTrigger, CompactableSession, CompactionError, CompactionResult,
    ContextCompactor, ContextCompactorBuilder, ContextStats, LlmSummarizer, Summarizer,
    TokenEstimationStrategy,
};
pub use manager::{
    CompactingSessionManager, CompactionStats, DefaultSessionManager, Session, SessionError,
    SessionManager,
};
pub use rollback::{
    BashRollbackHandler,
    // Manager
    CheckpointRollbackManager,
    CheckpointRollbackResult,
    FileEditRollbackHandler,
    // Built-in handlers
    FileWriteRollbackHandler,
    PreOperationState,
    RollbackComplexity,
    // Errors
    RollbackError,
    // Handler trait
    RollbackHandler,
    RollbackResult,
    // Core types
    RollbackableOperation,
};
pub use storage::{FileSessionStorage, MemorySessionStorage, SessionStorage};

use crate::agent::types::{AgentMessage, Usage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Session metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Session ID
    pub id: String,
    /// Parent session ID (if forked)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// User ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// Working directory
    pub cwd: String,
    /// Creation time
    pub created_at: DateTime<Utc>,
    /// Last update time
    pub updated_at: DateTime<Utc>,
    /// Turn count
    pub turn_count: u32,
    /// Message count
    pub message_count: u32,
    /// Token usage
    pub usage: Usage,
    /// Total cost in USD
    pub total_cost_usd: f64,
    /// Custom metadata
    #[serde(default)]
    pub custom: serde_json::Map<String, serde_json::Value>,
}

impl SessionMetadata {
    /// Create new session metadata
    pub fn new(id: impl Into<String>, cwd: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            parent_id: None,
            user_id: None,
            cwd: cwd.into(),
            created_at: now,
            updated_at: now,
            turn_count: 0,
            message_count: 0,
            usage: Usage::default(),
            total_cost_usd: 0.0,
            custom: serde_json::Map::new(),
        }
    }

    /// Create forked metadata
    pub fn fork(&self, new_id: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: new_id.into(),
            parent_id: Some(self.id.clone()),
            user_id: self.user_id.clone(),
            cwd: self.cwd.clone(),
            created_at: now,
            updated_at: now,
            turn_count: self.turn_count,
            message_count: self.message_count,
            usage: self.usage.clone(),
            total_cost_usd: self.total_cost_usd,
            custom: self.custom.clone(),
        }
    }
}

/// Session snapshot for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Metadata
    pub metadata: SessionMetadata,
    /// Messages
    pub messages: Vec<AgentMessage>,
    /// Summary (if compacted)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl SessionSnapshot {
    /// Create a new snapshot
    pub fn new(metadata: SessionMetadata, messages: Vec<AgentMessage>) -> Self {
        Self {
            metadata,
            messages,
            summary: None,
        }
    }

    /// Create with summary
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_metadata_new() {
        let meta = SessionMetadata::new("session-1", "/tmp");
        assert_eq!(meta.id, "session-1");
        assert_eq!(meta.cwd, "/tmp");
        assert!(meta.parent_id.is_none());
    }

    #[test]
    fn test_session_metadata_fork() {
        let meta = SessionMetadata::new("session-1", "/tmp");
        let forked = meta.fork("session-2");

        assert_eq!(forked.id, "session-2");
        assert_eq!(forked.parent_id, Some("session-1".to_string()));
        assert_eq!(forked.cwd, "/tmp");
    }
}
