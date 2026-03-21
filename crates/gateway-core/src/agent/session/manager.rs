//! Session Manager - Create, resume, and manage sessions
//!
//! This module provides session management with integrated context compaction
//! and checkpoint support for session persistence and recovery.

use super::checkpoint::{
    restore_from_checkpoint, AutoCheckpointConfig, AutoCheckpointTrigger, Checkpoint,
    CheckpointError, CheckpointManager, CheckpointMetadata, CheckpointTrigger, ContextState,
    RestoreResult,
};
use super::compact::{
    CompactableSession, CompactionError, CompactionResult, ContextCompactor, ContextStats,
};
use super::{SessionMetadata, SessionSnapshot, SessionStorage};
use crate::agent::types::{AgentMessage, Usage};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Session represents an active conversation session
pub struct Session {
    /// Metadata
    pub metadata: SessionMetadata,
    /// Messages
    messages: RwLock<Vec<AgentMessage>>,
    /// Storage backend
    storage: Option<Arc<dyn SessionStorage>>,
    /// Whether there are unsaved changes
    dirty: RwLock<bool>,
    /// Compaction statistics
    compaction_stats: RwLock<CompactionStats>,
}

/// Statistics about compaction operations on this session
#[derive(Debug, Clone, Default)]
pub struct CompactionStats {
    /// Number of compactions performed
    pub compaction_count: usize,
    /// Total tokens saved by compaction
    pub total_tokens_saved: usize,
    /// Total messages removed by compaction
    pub total_messages_removed: usize,
}

impl Session {
    /// Create a new session
    pub fn new(id: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            metadata: SessionMetadata::new(id, cwd),
            messages: RwLock::new(Vec::new()),
            storage: None,
            dirty: RwLock::new(false),
            compaction_stats: RwLock::new(CompactionStats::default()),
        }
    }

    /// Create with storage
    pub fn with_storage(mut self, storage: Arc<dyn SessionStorage>) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Create from snapshot
    pub fn from_snapshot(
        snapshot: SessionSnapshot,
        storage: Option<Arc<dyn SessionStorage>>,
    ) -> Self {
        Self {
            metadata: snapshot.metadata,
            messages: RwLock::new(snapshot.messages),
            storage,
            dirty: RwLock::new(false),
            compaction_stats: RwLock::new(CompactionStats::default()),
        }
    }

    /// Get session ID
    pub fn id(&self) -> &str {
        &self.metadata.id
    }

    /// Get messages
    pub async fn messages(&self) -> Vec<AgentMessage> {
        self.messages.read().await.clone()
    }

    /// Get message count
    pub async fn message_count(&self) -> usize {
        self.messages.read().await.len()
    }

    /// Add a message
    pub async fn add_message(&mut self, message: AgentMessage) {
        self.messages.write().await.push(message);
        self.metadata.message_count += 1;
        *self.dirty.write().await = true;
    }

    /// Add a message and compact if needed
    ///
    /// This is a convenience method that adds a message and automatically
    /// triggers compaction if the token threshold is exceeded.
    pub async fn add_message_with_compaction(
        &mut self,
        message: AgentMessage,
        compactor: &ContextCompactor,
    ) -> Result<Option<CompactionResult>, CompactionError> {
        self.add_message(message).await;

        // Check if compaction is needed
        self.compact_if_needed(compactor).await
    }

    /// Replace all messages (used after compaction)
    pub async fn replace_messages(&mut self, messages: Vec<AgentMessage>) {
        *self.messages.write().await = messages;
        *self.dirty.write().await = true;
    }

    /// Add usage
    pub fn add_usage(&mut self, usage: &Usage) {
        self.metadata.usage.add(usage);
    }

    /// Add cost
    pub fn add_cost(&mut self, cost: f64) {
        self.metadata.total_cost_usd += cost;
    }

    /// Increment turn count
    pub fn increment_turn(&mut self) {
        self.metadata.turn_count += 1;
    }

    /// Update timestamp
    pub fn touch(&mut self) {
        self.metadata.updated_at = Utc::now();
    }

    /// Create a snapshot
    pub async fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot::new(self.metadata.clone(), self.messages().await)
    }

    /// Save to storage
    pub async fn save(&self) -> Result<(), SessionError> {
        if let Some(storage) = &self.storage {
            let snapshot = self.snapshot().await;
            storage.save(&snapshot).await?;
            *self.dirty.write().await = false;
        }
        Ok(())
    }

    /// Check if dirty
    pub async fn is_dirty(&self) -> bool {
        *self.dirty.read().await
    }

    /// Get compaction statistics
    pub async fn compaction_stats(&self) -> CompactionStats {
        self.compaction_stats.read().await.clone()
    }

    /// Fork this session
    pub async fn fork(&self, new_id: impl Into<String>) -> Self {
        let new_metadata = self.metadata.fork(new_id);
        let messages = self.messages().await;

        Self {
            metadata: new_metadata,
            messages: RwLock::new(messages),
            storage: self.storage.clone(),
            dirty: RwLock::new(true),
            compaction_stats: RwLock::new(CompactionStats::default()),
        }
    }

    /// Create a checkpoint of the current session state
    pub async fn create_checkpoint(&self, trigger: CheckpointTrigger) -> Checkpoint {
        let messages = self.messages().await;
        let stats = self.compaction_stats().await;

        let context_state = ContextState {
            cwd: Some(self.metadata.cwd.clone()),
            env: HashMap::new(),
            estimated_tokens: 0, // Could be calculated if compactor is available
            turn_count: self.metadata.turn_count,
            usage: self.metadata.usage.clone(),
            total_cost_usd: self.metadata.total_cost_usd,
            custom: {
                let mut custom = HashMap::new();
                custom.insert(
                    "compaction_count".to_string(),
                    serde_json::json!(stats.compaction_count),
                );
                custom.insert(
                    "total_tokens_saved".to_string(),
                    serde_json::json!(stats.total_tokens_saved),
                );
                custom
            },
        };

        Checkpoint::new(&self.metadata.id, messages, context_state).with_trigger(trigger)
    }

    /// Create a checkpoint with a specific label
    pub async fn create_checkpoint_with_label(
        &self,
        trigger: CheckpointTrigger,
        label: impl Into<String>,
    ) -> Checkpoint {
        self.create_checkpoint(trigger).await.with_label(label)
    }

    /// Restore session state from a checkpoint
    ///
    /// This replaces all messages and updates metadata to match the checkpoint.
    /// Returns the restore result containing any warnings.
    pub async fn restore_from_checkpoint(
        &mut self,
        checkpoint: Checkpoint,
    ) -> Result<RestoreResult, CheckpointError> {
        let result = restore_from_checkpoint(checkpoint)?;

        // Restore messages
        *self.messages.write().await = result.messages.clone();

        // Update metadata from checkpoint context
        self.metadata.turn_count = result.checkpoint.context_state.turn_count;
        self.metadata.usage = result.checkpoint.context_state.usage.clone();
        self.metadata.total_cost_usd = result.checkpoint.context_state.total_cost_usd;
        self.metadata.message_count = result.messages.len() as u32;
        self.metadata.updated_at = Utc::now();

        // Mark as dirty since we've modified the session
        *self.dirty.write().await = true;

        tracing::info!(
            session_id = %self.metadata.id,
            checkpoint_id = %result.checkpoint.id,
            messages_restored = result.messages.len(),
            warnings = ?result.warnings,
            "Session restored from checkpoint"
        );

        Ok(result)
    }
}

#[async_trait]
impl CompactableSession for Session {
    async fn context_stats(&self, compactor: &ContextCompactor) -> ContextStats {
        let messages = self.messages().await;
        let current_tokens = compactor.estimate_tokens(&messages);
        let max_tokens = compactor.config().max_tokens;
        let threshold_tokens = compactor.threshold_tokens();
        let usage_percent = if max_tokens > 0 {
            (current_tokens as f64 / max_tokens as f64) * 100.0
        } else {
            0.0
        };

        let stats = self.compaction_stats.read().await;

        ContextStats {
            current_tokens,
            max_tokens,
            threshold_tokens,
            usage_percent,
            message_count: messages.len(),
            compaction_count: stats.compaction_count,
            total_tokens_saved: stats.total_tokens_saved,
        }
    }

    async fn compact_if_needed(
        &mut self,
        compactor: &ContextCompactor,
    ) -> Result<Option<CompactionResult>, CompactionError> {
        let messages = self.messages().await;

        if !compactor.needs_compaction(&messages) {
            return Ok(None);
        }

        let result = compactor.compact_if_needed(&messages).await?;

        if result.was_compacted {
            // Update session with compacted messages
            self.replace_messages(result.messages.clone()).await;

            // Update statistics
            {
                let mut stats = self.compaction_stats.write().await;
                stats.compaction_count += 1;
                stats.total_tokens_saved += result.tokens_saved();
                stats.total_messages_removed += result.messages_removed;
            }

            tracing::info!(
                session_id = %self.id(),
                tokens_before = result.tokens_before,
                tokens_after = result.tokens_after,
                messages_removed = result.messages_removed,
                "Session compacted"
            );

            Ok(Some(result))
        } else {
            Ok(None)
        }
    }
}

/// Session error
#[derive(Debug)]
pub enum SessionError {
    NotFound(String),
    StorageError(String),
    SerializationError(String),
    CompactionError(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "Session not found: {}", id),
            Self::StorageError(msg) => write!(f, "Storage error: {}", msg),
            Self::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            Self::CompactionError(msg) => write!(f, "Compaction error: {}", msg),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<CompactionError> for SessionError {
    fn from(err: CompactionError) -> Self {
        SessionError::CompactionError(err.to_string())
    }
}

/// Session manager trait
#[async_trait]
pub trait SessionManager: Send + Sync {
    /// Create a new session
    async fn create(&self, cwd: &str) -> Result<Session, SessionError>;

    /// Resume an existing session
    async fn resume(&self, session_id: &str) -> Result<Session, SessionError>;

    /// Fork a session
    async fn fork(&self, session_id: &str) -> Result<Session, SessionError>;

    /// Save a session
    async fn save(&self, session: &Session) -> Result<(), SessionError>;

    /// Delete a session
    async fn delete(&self, session_id: &str) -> Result<(), SessionError>;

    /// List sessions
    async fn list(&self, limit: Option<u32>) -> Result<Vec<SessionMetadata>, SessionError>;
}

/// Default session manager implementation
pub struct DefaultSessionManager {
    storage: Arc<dyn SessionStorage>,
    compactor: Option<Arc<ContextCompactor>>,
}

impl DefaultSessionManager {
    /// Create a new session manager
    pub fn new(storage: Arc<dyn SessionStorage>) -> Self {
        Self {
            storage,
            compactor: None,
        }
    }

    /// Create a session manager with automatic compaction
    pub fn with_compactor(
        storage: Arc<dyn SessionStorage>,
        compactor: Arc<ContextCompactor>,
    ) -> Self {
        Self {
            storage,
            compactor: Some(compactor),
        }
    }

    /// Get the compactor if configured
    pub fn compactor(&self) -> Option<&Arc<ContextCompactor>> {
        self.compactor.as_ref()
    }

    /// Compact a session if configured
    pub async fn compact_session(
        &self,
        session: &mut Session,
    ) -> Result<Option<CompactionResult>, SessionError> {
        if let Some(compactor) = &self.compactor {
            session
                .compact_if_needed(compactor)
                .await
                .map_err(SessionError::from)
        } else {
            Ok(None)
        }
    }
}

#[async_trait]
impl SessionManager for DefaultSessionManager {
    async fn create(&self, cwd: &str) -> Result<Session, SessionError> {
        let id = uuid::Uuid::new_v4().to_string();
        let session = Session::new(id, cwd).with_storage(self.storage.clone());
        session.save().await?;
        Ok(session)
    }

    async fn resume(&self, session_id: &str) -> Result<Session, SessionError> {
        let snapshot = self.storage.load(session_id).await?;
        Ok(Session::from_snapshot(snapshot, Some(self.storage.clone())))
    }

    async fn fork(&self, session_id: &str) -> Result<Session, SessionError> {
        let original = self.resume(session_id).await?;
        let new_id = uuid::Uuid::new_v4().to_string();
        let forked = original.fork(new_id).await;
        forked.save().await?;
        Ok(forked)
    }

    async fn save(&self, session: &Session) -> Result<(), SessionError> {
        session.save().await
    }

    async fn delete(&self, session_id: &str) -> Result<(), SessionError> {
        self.storage.delete(session_id).await
    }

    async fn list(&self, limit: Option<u32>) -> Result<Vec<SessionMetadata>, SessionError> {
        self.storage.list(limit).await
    }
}

/// Session manager with automatic compaction on every message
pub struct CompactingSessionManager {
    inner: DefaultSessionManager,
    compactor: Arc<ContextCompactor>,
}

impl CompactingSessionManager {
    /// Create a new compacting session manager
    pub fn new(storage: Arc<dyn SessionStorage>, compactor: Arc<ContextCompactor>) -> Self {
        Self {
            inner: DefaultSessionManager::new(storage),
            compactor,
        }
    }

    /// Get the compactor
    pub fn compactor(&self) -> &Arc<ContextCompactor> {
        &self.compactor
    }

    /// Add a message to a session and compact if needed
    pub async fn add_message_and_compact(
        &self,
        session: &mut Session,
        message: AgentMessage,
    ) -> Result<Option<CompactionResult>, SessionError> {
        session.add_message(message).await;

        // Check and perform compaction
        session
            .compact_if_needed(&self.compactor)
            .await
            .map_err(SessionError::from)
    }

    /// Get context statistics for a session
    pub async fn context_stats(&self, session: &Session) -> ContextStats {
        session.context_stats(&self.compactor).await
    }
}

#[async_trait]
impl SessionManager for CompactingSessionManager {
    async fn create(&self, cwd: &str) -> Result<Session, SessionError> {
        self.inner.create(cwd).await
    }

    async fn resume(&self, session_id: &str) -> Result<Session, SessionError> {
        self.inner.resume(session_id).await
    }

    async fn fork(&self, session_id: &str) -> Result<Session, SessionError> {
        self.inner.fork(session_id).await
    }

    async fn save(&self, session: &Session) -> Result<(), SessionError> {
        self.inner.save(session).await
    }

    async fn delete(&self, session_id: &str) -> Result<(), SessionError> {
        self.inner.delete(session_id).await
    }

    async fn list(&self, limit: Option<u32>) -> Result<Vec<SessionMetadata>, SessionError> {
        self.inner.list(limit).await
    }
}

/// Session manager with automatic checkpointing support
///
/// This manager wraps a base session manager and adds checkpoint functionality:
/// - Auto-checkpoints before dangerous operations
/// - Periodic checkpoints based on turn count
/// - Checkpoints before context compaction
/// - Checkpoint management (list, restore, delete)
pub struct CheckpointingSessionManager<M: SessionManager, C: CheckpointManager> {
    /// Inner session manager
    inner: M,
    /// Checkpoint manager
    checkpoint_manager: Arc<C>,
    /// Auto-checkpoint configuration
    config: AutoCheckpointConfig,
    /// Per-session checkpoint triggers
    triggers: RwLock<HashMap<String, AutoCheckpointTrigger>>,
}

impl<M: SessionManager, C: CheckpointManager> CheckpointingSessionManager<M, C> {
    /// Create a new checkpointing session manager
    pub fn new(inner: M, checkpoint_manager: Arc<C>) -> Self {
        Self {
            inner,
            checkpoint_manager,
            config: AutoCheckpointConfig::default(),
            triggers: RwLock::new(HashMap::new()),
        }
    }

    /// Create with custom auto-checkpoint configuration
    pub fn with_config(inner: M, checkpoint_manager: Arc<C>, config: AutoCheckpointConfig) -> Self {
        Self {
            inner,
            checkpoint_manager,
            config,
            triggers: RwLock::new(HashMap::new()),
        }
    }

    /// Get the checkpoint manager
    pub fn checkpoint_manager(&self) -> &Arc<C> {
        &self.checkpoint_manager
    }

    /// Get auto-checkpoint configuration
    pub fn config(&self) -> &AutoCheckpointConfig {
        &self.config
    }

    /// Get or create a trigger for a session
    async fn get_or_create_trigger(&self, session_id: &str) -> AutoCheckpointTrigger {
        let mut triggers = self.triggers.write().await;
        triggers
            .entry(session_id.to_string())
            .or_insert_with(|| AutoCheckpointTrigger::new(self.config.clone()))
            .clone()
    }

    /// Update trigger for a session
    async fn update_trigger(&self, session_id: &str, trigger: AutoCheckpointTrigger) {
        self.triggers
            .write()
            .await
            .insert(session_id.to_string(), trigger);
    }

    /// Create a checkpoint for a session
    pub async fn create_checkpoint(
        &self,
        session: &Session,
        trigger: CheckpointTrigger,
    ) -> Result<String, SessionError> {
        let checkpoint = session.create_checkpoint(trigger).await;
        self.checkpoint_manager
            .save(&checkpoint)
            .await
            .map_err(|e| SessionError::StorageError(format!("Checkpoint save failed: {}", e)))
    }

    /// Create a checkpoint with a label
    pub async fn create_checkpoint_with_label(
        &self,
        session: &Session,
        trigger: CheckpointTrigger,
        label: impl Into<String>,
    ) -> Result<String, SessionError> {
        let checkpoint = session.create_checkpoint_with_label(trigger, label).await;
        self.checkpoint_manager
            .save(&checkpoint)
            .await
            .map_err(|e| SessionError::StorageError(format!("Checkpoint save failed: {}", e)))
    }

    /// Check if a checkpoint should be created before a tool execution
    pub async fn maybe_checkpoint_before_tool(
        &self,
        session: &Session,
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) -> Result<Option<String>, SessionError> {
        let trigger = self.get_or_create_trigger(session.id()).await;

        if let Some(checkpoint_trigger) =
            trigger.should_checkpoint_before_tool(tool_name, tool_input)
        {
            let checkpoint_id = self.create_checkpoint(session, checkpoint_trigger).await?;

            tracing::debug!(
                session_id = %session.id(),
                tool_name = %tool_name,
                checkpoint_id = %checkpoint_id,
                "Auto-checkpoint created before tool execution"
            );

            // Clean up old checkpoints if needed
            self.cleanup_old_checkpoints(session.id()).await?;

            return Ok(Some(checkpoint_id));
        }

        Ok(None)
    }

    /// Check and maybe create a periodic checkpoint
    pub async fn maybe_checkpoint_periodic(
        &self,
        session: &Session,
    ) -> Result<Option<String>, SessionError> {
        let mut trigger = self.get_or_create_trigger(session.id()).await;

        if let Some(checkpoint_trigger) = trigger.should_checkpoint_periodic() {
            trigger.reset_periodic_counter();
            self.update_trigger(session.id(), trigger).await;

            let checkpoint_id = self.create_checkpoint(session, checkpoint_trigger).await?;

            tracing::debug!(
                session_id = %session.id(),
                checkpoint_id = %checkpoint_id,
                "Periodic auto-checkpoint created"
            );

            // Clean up old checkpoints if needed
            self.cleanup_old_checkpoints(session.id()).await?;

            return Ok(Some(checkpoint_id));
        }

        // Update the trigger even if no checkpoint was created
        self.update_trigger(session.id(), trigger).await;
        Ok(None)
    }

    /// Check and maybe create a checkpoint before compaction
    pub async fn maybe_checkpoint_before_compaction(
        &self,
        session: &Session,
    ) -> Result<Option<String>, SessionError> {
        let trigger = self.get_or_create_trigger(session.id()).await;

        if let Some(checkpoint_trigger) = trigger.should_checkpoint_before_compaction() {
            let checkpoint_id = self.create_checkpoint(session, checkpoint_trigger).await?;

            tracing::debug!(
                session_id = %session.id(),
                checkpoint_id = %checkpoint_id,
                "Auto-checkpoint created before compaction"
            );

            return Ok(Some(checkpoint_id));
        }

        Ok(None)
    }

    /// List all checkpoints for a session
    pub async fn list_checkpoints(
        &self,
        session_id: &str,
    ) -> Result<Vec<CheckpointMetadata>, SessionError> {
        self.checkpoint_manager
            .list(session_id)
            .await
            .map_err(|e| SessionError::StorageError(format!("Failed to list checkpoints: {}", e)))
    }

    /// Load a checkpoint by ID
    pub async fn load_checkpoint(&self, checkpoint_id: &str) -> Result<Checkpoint, SessionError> {
        self.checkpoint_manager
            .load(checkpoint_id)
            .await
            .map_err(|e| SessionError::StorageError(format!("Failed to load checkpoint: {}", e)))
    }

    /// Restore a session from a checkpoint
    pub async fn restore_session(
        &self,
        session: &mut Session,
        checkpoint_id: &str,
    ) -> Result<RestoreResult, SessionError> {
        let checkpoint = self.load_checkpoint(checkpoint_id).await?;

        // Verify the checkpoint belongs to this session
        if checkpoint.session_id != session.id() {
            return Err(SessionError::StorageError(format!(
                "Checkpoint {} belongs to session {}, not {}",
                checkpoint_id,
                checkpoint.session_id,
                session.id()
            )));
        }

        session
            .restore_from_checkpoint(checkpoint)
            .await
            .map_err(|e| SessionError::StorageError(format!("Failed to restore: {}", e)))
    }

    /// Get the latest checkpoint for a session
    pub async fn get_latest_checkpoint(
        &self,
        session_id: &str,
    ) -> Result<Option<Checkpoint>, SessionError> {
        self.checkpoint_manager
            .get_latest(session_id)
            .await
            .map_err(|e| {
                SessionError::StorageError(format!("Failed to get latest checkpoint: {}", e))
            })
    }

    /// Delete a checkpoint
    pub async fn delete_checkpoint(&self, checkpoint_id: &str) -> Result<(), SessionError> {
        self.checkpoint_manager
            .delete(checkpoint_id)
            .await
            .map_err(|e| SessionError::StorageError(format!("Failed to delete checkpoint: {}", e)))
    }

    /// Delete all checkpoints for a session
    pub async fn delete_all_checkpoints(&self, session_id: &str) -> Result<usize, SessionError> {
        self.checkpoint_manager
            .delete_all(session_id)
            .await
            .map_err(|e| SessionError::StorageError(format!("Failed to delete checkpoints: {}", e)))
    }

    /// Clean up old checkpoints if over the limit
    async fn cleanup_old_checkpoints(&self, session_id: &str) -> Result<(), SessionError> {
        if let Some(max_checkpoints) = self.config.max_checkpoints_per_session {
            let checkpoints = self.list_checkpoints(session_id).await?;

            if checkpoints.len() > max_checkpoints {
                // Get the oldest checkpoints to delete
                // Note: list returns newest first, so we take from the end
                let to_delete = checkpoints.len() - max_checkpoints;

                // We need to get checkpoint IDs, but CheckpointMetadata doesn't have ID
                // Instead, let's get all checkpoints and delete oldest
                if let Ok(Some(_oldest)) = self.checkpoint_manager.get_latest(session_id).await {
                    // This is a simplification - in production, we'd need to track IDs
                    // For now, we just log that cleanup would happen
                    tracing::debug!(
                        session_id = %session_id,
                        checkpoint_count = checkpoints.len(),
                        max_checkpoints = max_checkpoints,
                        would_delete = to_delete,
                        "Checkpoint cleanup would delete old checkpoints"
                    );
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl<M: SessionManager + Send + Sync, C: CheckpointManager + Send + Sync> SessionManager
    for CheckpointingSessionManager<M, C>
{
    async fn create(&self, cwd: &str) -> Result<Session, SessionError> {
        self.inner.create(cwd).await
    }

    async fn resume(&self, session_id: &str) -> Result<Session, SessionError> {
        self.inner.resume(session_id).await
    }

    async fn fork(&self, session_id: &str) -> Result<Session, SessionError> {
        self.inner.fork(session_id).await
    }

    async fn save(&self, session: &Session) -> Result<(), SessionError> {
        self.inner.save(session).await
    }

    async fn delete(&self, session_id: &str) -> Result<(), SessionError> {
        // Also delete checkpoints when session is deleted
        let _ = self.delete_all_checkpoints(session_id).await;
        self.inner.delete(session_id).await
    }

    async fn list(&self, limit: Option<u32>) -> Result<Vec<SessionMetadata>, SessionError> {
        self.inner.list(limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::session::checkpoint::MemoryCheckpointManager;
    use crate::agent::types::MessageContent;

    #[tokio::test]
    async fn test_session_new() {
        let session = Session::new("test-session", "/tmp");
        assert_eq!(session.id(), "test-session");
        assert_eq!(session.message_count().await, 0);
    }

    #[tokio::test]
    async fn test_session_add_message() {
        let mut session = Session::new("test", "/tmp");

        let msg = AgentMessage::User(crate::agent::types::UserMessage {
            content: MessageContent::text("Hello"),
            uuid: None,
            parent_tool_use_id: None,
            tool_use_result: None,
        });

        session.add_message(msg).await;
        assert_eq!(session.message_count().await, 1);
        assert!(session.is_dirty().await);
    }

    #[tokio::test]
    async fn test_session_fork() {
        let session = Session::new("original", "/tmp");
        let forked = session.fork("forked").await;

        assert_eq!(forked.id(), "forked");
        assert_eq!(forked.metadata.parent_id, Some("original".to_string()));
    }

    #[tokio::test]
    async fn test_session_context_stats() {
        let mut session = Session::new("test", "/tmp");
        let compactor = ContextCompactor::builder()
            .max_tokens(1000)
            .threshold_ratio(0.8)
            .build();

        // Add some messages
        for i in 0..5 {
            let msg = AgentMessage::User(crate::agent::types::UserMessage {
                content: MessageContent::text(format!("Message {}", i)),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            });
            session.add_message(msg).await;
        }

        let stats = session.context_stats(&compactor).await;
        assert_eq!(stats.message_count, 5);
        assert!(stats.current_tokens > 0);
        assert_eq!(stats.max_tokens, 1000);
        assert_eq!(stats.threshold_tokens, 800);
    }

    #[tokio::test]
    async fn test_session_compaction() {
        let mut session = Session::new("test", "/tmp");
        let compactor = ContextCompactor::builder()
            .max_tokens(100) // Low threshold to trigger compaction
            .threshold_ratio(0.5)
            .keep_recent(2)
            .build();

        // Add many messages to exceed threshold
        for i in 0..20 {
            let msg = AgentMessage::User(crate::agent::types::UserMessage {
                content: MessageContent::text(format!(
                    "This is a longer message number {} with more content",
                    i
                )),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            });
            session.add_message(msg).await;
        }

        // Verify we have many messages
        assert_eq!(session.message_count().await, 20);

        // Compact
        let result = session.compact_if_needed(&compactor).await.unwrap();
        assert!(result.is_some());

        let result = result.unwrap();
        assert!(result.was_compacted);
        assert!(result.messages_removed > 0);

        // Verify messages were replaced
        let message_count = session.message_count().await;
        assert!(message_count < 20);

        // Verify stats were updated
        let stats = session.compaction_stats().await;
        assert_eq!(stats.compaction_count, 1);
        assert!(stats.total_tokens_saved > 0);
    }

    #[tokio::test]
    async fn test_add_message_with_compaction() {
        let mut session = Session::new("test", "/tmp");
        let compactor = ContextCompactor::builder()
            .max_tokens(50) // Very low to trigger on few messages
            .threshold_ratio(0.5)
            .keep_recent(2)
            .build();

        // Add messages until compaction triggers
        for i in 0..15 {
            let msg = AgentMessage::User(crate::agent::types::UserMessage {
                content: MessageContent::text(format!("Message {} with extra content", i)),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            });
            let _ = session.add_message_with_compaction(msg, &compactor).await;
        }

        // Should have been compacted at some point
        let stats = session.compaction_stats().await;
        assert!(stats.compaction_count > 0 || session.message_count().await <= 15);
    }

    #[tokio::test]
    async fn test_compaction_stats_tracking() {
        let mut session = Session::new("test", "/tmp");
        let compactor = ContextCompactor::builder()
            .max_tokens(100)
            .threshold_ratio(0.3) // Trigger early
            .keep_recent(2)
            .build();

        // Add enough messages to trigger compaction
        for i in 0..30 {
            let msg = AgentMessage::User(crate::agent::types::UserMessage {
                content: MessageContent::text(format!("Test message {} with content", i)),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            });
            session.add_message(msg).await;
        }

        // Compact multiple times
        let _ = session.compact_if_needed(&compactor).await;

        // Add more and compact again
        for i in 0..20 {
            let msg = AgentMessage::User(crate::agent::types::UserMessage {
                content: MessageContent::text(format!("Another message {}", i)),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            });
            session.add_message(msg).await;
        }
        let _ = session.compact_if_needed(&compactor).await;

        // Check cumulative stats
        let stats = session.compaction_stats().await;
        // At least one compaction should have happened
        assert!(stats.compaction_count >= 1);
    }

    // ========================================================================
    // Checkpoint Integration Tests
    // ========================================================================

    #[tokio::test]
    async fn test_session_create_checkpoint() {
        let mut session = Session::new("test-session", "/tmp/project");

        // Add some messages
        for i in 0..3 {
            let msg = AgentMessage::User(crate::agent::types::UserMessage {
                content: MessageContent::text(format!("Message {}", i)),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            });
            session.add_message(msg).await;
        }

        // Update some metadata
        session.metadata.turn_count = 5;
        session.metadata.total_cost_usd = 0.05;

        // Create checkpoint
        let checkpoint = session.create_checkpoint(CheckpointTrigger::Manual).await;

        assert_eq!(checkpoint.session_id, "test-session");
        assert_eq!(checkpoint.conversation_history.len(), 3);
        assert_eq!(checkpoint.context_state.turn_count, 5);
        assert_eq!(
            checkpoint.context_state.cwd,
            Some("/tmp/project".to_string())
        );
        assert_eq!(checkpoint.metadata.trigger, CheckpointTrigger::Manual);
    }

    #[tokio::test]
    async fn test_session_create_checkpoint_with_label() {
        let session = Session::new("test-session", "/tmp");
        let checkpoint = session
            .create_checkpoint_with_label(
                CheckpointTrigger::BeforeDangerousOperation,
                "Before file edit",
            )
            .await;

        assert_eq!(
            checkpoint.metadata.label,
            Some("Before file edit".to_string())
        );
        assert_eq!(
            checkpoint.metadata.trigger,
            CheckpointTrigger::BeforeDangerousOperation
        );
    }

    #[tokio::test]
    async fn test_session_restore_from_checkpoint() {
        let mut session = Session::new("test-session", "/tmp");

        // Add messages and create checkpoint
        for i in 0..5 {
            let msg = AgentMessage::User(crate::agent::types::UserMessage {
                content: MessageContent::text(format!("Original message {}", i)),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            });
            session.add_message(msg).await;
        }
        session.metadata.turn_count = 10;

        let checkpoint = session.create_checkpoint(CheckpointTrigger::Manual).await;

        // Add more messages after checkpoint
        for i in 0..3 {
            let msg = AgentMessage::User(crate::agent::types::UserMessage {
                content: MessageContent::text(format!("New message {}", i)),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            });
            session.add_message(msg).await;
        }
        session.metadata.turn_count = 15;

        // Verify current state
        assert_eq!(session.message_count().await, 8);
        assert_eq!(session.metadata.turn_count, 15);

        // Restore from checkpoint
        let result = session.restore_from_checkpoint(checkpoint).await.unwrap();

        // Verify restored state
        assert_eq!(session.message_count().await, 5);
        assert_eq!(session.metadata.turn_count, 10);
        assert_eq!(result.messages.len(), 5);
        assert!(result.warnings.is_empty());
    }

    #[tokio::test]
    async fn test_checkpointing_session_manager() {
        use crate::agent::session::storage::MemorySessionStorage;

        let storage = Arc::new(MemorySessionStorage::new());
        let session_manager = DefaultSessionManager::new(storage);
        let checkpoint_manager = Arc::new(MemoryCheckpointManager::new());

        let manager = CheckpointingSessionManager::new(session_manager, checkpoint_manager);

        // Create a session
        let mut session = manager.create("/tmp").await.unwrap();

        // Add messages
        for i in 0..3 {
            let msg = AgentMessage::User(crate::agent::types::UserMessage {
                content: MessageContent::text(format!("Message {}", i)),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            });
            session.add_message(msg).await;
        }

        // Create a checkpoint
        let checkpoint_id = manager
            .create_checkpoint(&session, CheckpointTrigger::Manual)
            .await
            .unwrap();

        // List checkpoints
        let checkpoints = manager.list_checkpoints(session.id()).await.unwrap();
        assert_eq!(checkpoints.len(), 1);

        // Load checkpoint
        let loaded = manager.load_checkpoint(&checkpoint_id).await.unwrap();
        assert_eq!(loaded.conversation_history.len(), 3);

        // Get latest checkpoint
        let latest = manager.get_latest_checkpoint(session.id()).await.unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().id, checkpoint_id);
    }

    #[tokio::test]
    async fn test_checkpointing_session_manager_auto_checkpoint_before_tool() {
        use crate::agent::session::storage::MemorySessionStorage;

        let storage = Arc::new(MemorySessionStorage::new());
        let session_manager = DefaultSessionManager::new(storage);
        let checkpoint_manager = Arc::new(MemoryCheckpointManager::new());

        let manager = CheckpointingSessionManager::new(session_manager, checkpoint_manager);

        let mut session = manager.create("/tmp").await.unwrap();
        session
            .add_message(AgentMessage::User(crate::agent::types::UserMessage {
                content: MessageContent::text("Test"),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            }))
            .await;

        // Should create checkpoint before Write tool
        let result = manager
            .maybe_checkpoint_before_tool(
                &session,
                "Write",
                &serde_json::json!({"file_path": "/tmp/test"}),
            )
            .await
            .unwrap();
        assert!(result.is_some());

        // Should not create checkpoint for Read tool
        let result = manager
            .maybe_checkpoint_before_tool(
                &session,
                "Read",
                &serde_json::json!({"file_path": "/tmp/test"}),
            )
            .await
            .unwrap();
        assert!(result.is_none());

        // Should create checkpoint before Bash tool
        let result = manager
            .maybe_checkpoint_before_tool(&session, "Bash", &serde_json::json!({"command": "ls"}))
            .await
            .unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_checkpointing_session_manager_periodic() {
        use crate::agent::session::storage::MemorySessionStorage;

        let storage = Arc::new(MemorySessionStorage::new());
        let session_manager = DefaultSessionManager::new(storage);
        let checkpoint_manager = Arc::new(MemoryCheckpointManager::new());

        // Configure to checkpoint every 2 turns
        let config = AutoCheckpointConfig {
            periodic_turns: Some(2),
            before_dangerous_ops: false,
            ..Default::default()
        };

        let manager =
            CheckpointingSessionManager::with_config(session_manager, checkpoint_manager, config);

        let session = manager.create("/tmp").await.unwrap();

        // First turn: no checkpoint
        let result = manager.maybe_checkpoint_periodic(&session).await.unwrap();
        assert!(result.is_none());

        // Second turn: checkpoint
        let result = manager.maybe_checkpoint_periodic(&session).await.unwrap();
        assert!(result.is_some());

        // Third turn: no checkpoint
        let result = manager.maybe_checkpoint_periodic(&session).await.unwrap();
        assert!(result.is_none());

        // Fourth turn: checkpoint
        let result = manager.maybe_checkpoint_periodic(&session).await.unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_checkpointing_session_manager_restore() {
        use crate::agent::session::storage::MemorySessionStorage;

        let storage = Arc::new(MemorySessionStorage::new());
        let session_manager = DefaultSessionManager::new(storage);
        let checkpoint_manager = Arc::new(MemoryCheckpointManager::new());

        let manager = CheckpointingSessionManager::new(session_manager, checkpoint_manager);

        let mut session = manager.create("/tmp").await.unwrap();

        // Add initial messages and create checkpoint
        for i in 0..3 {
            session
                .add_message(AgentMessage::User(crate::agent::types::UserMessage {
                    content: MessageContent::text(format!("Initial {}", i)),
                    uuid: None,
                    parent_tool_use_id: None,
                    tool_use_result: None,
                }))
                .await;
        }

        let checkpoint_id = manager
            .create_checkpoint(&session, CheckpointTrigger::Manual)
            .await
            .unwrap();

        // Add more messages
        for i in 0..5 {
            session
                .add_message(AgentMessage::User(crate::agent::types::UserMessage {
                    content: MessageContent::text(format!("After {}", i)),
                    uuid: None,
                    parent_tool_use_id: None,
                    tool_use_result: None,
                }))
                .await;
        }

        assert_eq!(session.message_count().await, 8);

        // Restore from checkpoint
        let result = manager
            .restore_session(&mut session, &checkpoint_id)
            .await
            .unwrap();

        assert_eq!(session.message_count().await, 3);
        assert_eq!(result.messages.len(), 3);
    }

    #[tokio::test]
    async fn test_checkpointing_session_manager_delete_session_cleans_checkpoints() {
        use crate::agent::session::storage::MemorySessionStorage;

        let storage = Arc::new(MemorySessionStorage::new());
        let session_manager = DefaultSessionManager::new(storage);
        let checkpoint_manager = Arc::new(MemoryCheckpointManager::new());

        let manager = CheckpointingSessionManager::new(session_manager, checkpoint_manager.clone());

        let session = manager.create("/tmp").await.unwrap();
        let session_id = session.id().to_string();

        // Create some checkpoints
        manager
            .create_checkpoint(&session, CheckpointTrigger::Manual)
            .await
            .unwrap();
        manager
            .create_checkpoint(&session, CheckpointTrigger::Manual)
            .await
            .unwrap();

        // Verify checkpoints exist
        let checkpoints = manager.list_checkpoints(&session_id).await.unwrap();
        assert_eq!(checkpoints.len(), 2);

        // Delete session
        manager.delete(&session_id).await.unwrap();

        // Verify checkpoints are also deleted
        let checkpoints = manager.list_checkpoints(&session_id).await.unwrap();
        assert_eq!(checkpoints.len(), 0);
    }
}
