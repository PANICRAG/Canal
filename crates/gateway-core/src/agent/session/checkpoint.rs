//! Checkpoint System - Session state persistence for Claude Agent SDK compatibility
//!
//! This module provides checkpoint functionality for saving and restoring session states,
//! enabling features like:
//! - Point-in-time session snapshots
//! - Recovery from dangerous operations
//! - Session restoration across restarts
//! - Auto-checkpointing based on configurable triggers
//!
//! # Usage
//!
//! ```rust,ignore
//! use gateway_core::agent::session::{Checkpoint, CheckpointManager, FileCheckpointManager};
//!
//! // Create a file-based checkpoint manager
//! let manager = FileCheckpointManager::new("/path/to/checkpoints");
//!
//! // Save a checkpoint
//! let checkpoint = Checkpoint::new("session-1", messages, context_state);
//! let id = manager.save(&checkpoint).await?;
//!
//! // Load a checkpoint
//! let restored = manager.load(&id).await?;
//! ```

use crate::agent::types::{AgentMessage, Usage};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use tokio::fs;

// ============================================================================
// Core Types
// ============================================================================

/// Checkpoint represents a complete snapshot of session state at a point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Unique identifier for this checkpoint
    pub id: String,
    /// Session ID this checkpoint belongs to
    pub session_id: String,
    /// Timestamp when checkpoint was created
    pub created_at: DateTime<Utc>,
    /// Conversation history at checkpoint time
    pub conversation_history: Vec<AgentMessage>,
    /// Context state (working directory, environment, etc.)
    pub context_state: ContextState,
    /// Tool states (serialized state for each active tool)
    pub tool_states: HashMap<String, serde_json::Value>,
    /// Checkpoint metadata
    pub metadata: CheckpointMetadata,
}

/// Context state captured in checkpoint
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextState {
    /// Current working directory
    pub cwd: Option<String>,
    /// Environment variables (selective)
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Estimated token count at checkpoint time
    pub estimated_tokens: usize,
    /// Number of turns at checkpoint time
    pub turn_count: u32,
    /// Token usage statistics
    #[serde(default)]
    pub usage: Usage,
    /// Total cost in USD
    pub total_cost_usd: f64,
    /// Custom context data
    #[serde(default)]
    pub custom: HashMap<String, serde_json::Value>,
}

/// Metadata about a checkpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMetadata {
    /// Human-readable label
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Trigger that caused this checkpoint
    pub trigger: CheckpointTrigger,
    /// Number of messages in checkpoint
    pub message_count: usize,
    /// Whether this checkpoint is compressed
    pub is_compressed: bool,
    /// Size in bytes (after optional compression)
    pub size_bytes: Option<u64>,
    /// Schema version for compatibility
    pub schema_version: u32,
    /// Custom metadata tags
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Default for CheckpointMetadata {
    fn default() -> Self {
        Self {
            label: None,
            trigger: CheckpointTrigger::Manual,
            message_count: 0,
            is_compressed: false,
            size_bytes: None,
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            tags: Vec::new(),
        }
    }
}

/// Trigger that caused a checkpoint to be created
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointTrigger {
    /// Checkpoint created manually by user request
    Manual,
    /// Checkpoint created before a dangerous operation
    BeforeDangerousOperation,
    /// Checkpoint created periodically based on turn count
    Periodic,
    /// Checkpoint created before context compaction
    BeforeCompaction,
    /// Checkpoint created on session end
    SessionEnd,
    /// Checkpoint created at tool milestone
    ToolMilestone,
}

/// Current schema version for checkpoint compatibility
pub const CHECKPOINT_SCHEMA_VERSION: u32 = 1;

// ============================================================================
// Checkpoint Builder
// ============================================================================

impl Checkpoint {
    /// Create a new checkpoint
    pub fn new(
        session_id: impl Into<String>,
        conversation_history: Vec<AgentMessage>,
        context_state: ContextState,
    ) -> Self {
        let session_id = session_id.into();
        let id = format!(
            "chk_{}_{:.8}",
            Utc::now().format("%Y%m%d%H%M%S"),
            uuid::Uuid::new_v4().to_string().replace("-", "")
        );

        let message_count = conversation_history.len();

        Self {
            id,
            session_id,
            created_at: Utc::now(),
            conversation_history,
            context_state,
            tool_states: HashMap::new(),
            metadata: CheckpointMetadata {
                message_count,
                ..Default::default()
            },
        }
    }

    /// Create checkpoint with a specific ID
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Set the checkpoint trigger
    pub fn with_trigger(mut self, trigger: CheckpointTrigger) -> Self {
        self.metadata.trigger = trigger;
        self
    }

    /// Set a human-readable label
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.metadata.label = Some(label.into());
        self
    }

    /// Add tool states
    pub fn with_tool_states(mut self, states: HashMap<String, serde_json::Value>) -> Self {
        self.tool_states = states;
        self
    }

    /// Add a single tool state
    pub fn with_tool_state(
        mut self,
        tool_name: impl Into<String>,
        state: serde_json::Value,
    ) -> Self {
        self.tool_states.insert(tool_name.into(), state);
        self
    }

    /// Add tags to metadata
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.metadata.tags = tags;
        self
    }

    /// Validate checkpoint compatibility
    pub fn validate(&self) -> Result<(), CheckpointError> {
        // Check schema version
        if self.metadata.schema_version > CHECKPOINT_SCHEMA_VERSION {
            return Err(CheckpointError::IncompatibleVersion {
                checkpoint_version: self.metadata.schema_version,
                current_version: CHECKPOINT_SCHEMA_VERSION,
            });
        }

        // Check session ID
        if self.session_id.is_empty() {
            return Err(CheckpointError::InvalidCheckpoint(
                "Session ID cannot be empty".to_string(),
            ));
        }

        // Check ID
        if self.id.is_empty() {
            return Err(CheckpointError::InvalidCheckpoint(
                "Checkpoint ID cannot be empty".to_string(),
            ));
        }

        Ok(())
    }
}

// ============================================================================
// Checkpoint Manager Trait
// ============================================================================

/// Trait for checkpoint storage backends
#[async_trait]
pub trait CheckpointManager: Send + Sync {
    /// Save a checkpoint and return its ID
    async fn save(&self, checkpoint: &Checkpoint) -> Result<String, CheckpointError>;

    /// Load a checkpoint by ID
    async fn load(&self, checkpoint_id: &str) -> Result<Checkpoint, CheckpointError>;

    /// List all checkpoints for a session
    async fn list(&self, session_id: &str) -> Result<Vec<CheckpointMetadata>, CheckpointError>;

    /// Delete a checkpoint by ID
    async fn delete(&self, checkpoint_id: &str) -> Result<(), CheckpointError>;

    /// Delete all checkpoints for a session
    async fn delete_all(&self, session_id: &str) -> Result<usize, CheckpointError>;

    /// Get the latest checkpoint for a session
    async fn get_latest(&self, session_id: &str) -> Result<Option<Checkpoint>, CheckpointError>;
}

// ============================================================================
// File-Based Checkpoint Manager
// ============================================================================

/// File-based checkpoint manager supporting JSON with optional compression
pub struct FileCheckpointManager {
    /// Base directory for checkpoint files
    base_path: PathBuf,
    /// Whether to compress checkpoints
    compress: bool,
}

impl FileCheckpointManager {
    /// Create a new file checkpoint manager
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
            compress: false,
        }
    }

    /// Enable compression for checkpoints
    pub fn with_compression(mut self, compress: bool) -> Self {
        self.compress = compress;
        self
    }

    /// Get the directory path for a session's checkpoints
    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.base_path.join("sessions").join(session_id)
    }

    /// Get the file path for a checkpoint
    fn checkpoint_path(&self, session_id: &str, checkpoint_id: &str) -> PathBuf {
        let extension = if self.compress { "json.gz" } else { "json" };
        self.session_dir(session_id)
            .join(format!("{}.{}", checkpoint_id, extension))
    }

    /// Get the metadata file path for a checkpoint
    fn metadata_path(&self, session_id: &str, checkpoint_id: &str) -> PathBuf {
        self.session_dir(session_id)
            .join(format!("{}.meta.json", checkpoint_id))
    }

    /// Ensure session directory exists
    async fn ensure_session_dir(&self, session_id: &str) -> Result<(), CheckpointError> {
        let dir = self.session_dir(session_id);
        fs::create_dir_all(&dir).await.map_err(|e| {
            CheckpointError::StorageError(format!("Failed to create directory: {}", e))
        })
    }

    /// Serialize checkpoint data (with optional compression)
    fn serialize(&self, checkpoint: &Checkpoint) -> Result<(Vec<u8>, u64), CheckpointError> {
        let json = serde_json::to_vec_pretty(checkpoint)
            .map_err(|e| CheckpointError::SerializationError(e.to_string()))?;

        if self.compress {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder
                .write_all(&json)
                .map_err(|e| CheckpointError::CompressionError(e.to_string()))?;
            let compressed = encoder
                .finish()
                .map_err(|e| CheckpointError::CompressionError(e.to_string()))?;
            let size = compressed.len() as u64;
            Ok((compressed, size))
        } else {
            let size = json.len() as u64;
            Ok((json, size))
        }
    }

    /// Deserialize checkpoint data (with optional decompression)
    fn deserialize(&self, data: &[u8], is_compressed: bool) -> Result<Checkpoint, CheckpointError> {
        if is_compressed {
            let mut decoder = flate2::read::GzDecoder::new(data);
            let mut json = Vec::new();
            decoder
                .read_to_end(&mut json)
                .map_err(|e| CheckpointError::DecompressionError(e.to_string()))?;
            serde_json::from_slice(&json)
                .map_err(|e| CheckpointError::DeserializationError(e.to_string()))
        } else {
            serde_json::from_slice(data)
                .map_err(|e| CheckpointError::DeserializationError(e.to_string()))
        }
    }
}

#[async_trait]
impl CheckpointManager for FileCheckpointManager {
    async fn save(&self, checkpoint: &Checkpoint) -> Result<String, CheckpointError> {
        checkpoint.validate()?;

        self.ensure_session_dir(&checkpoint.session_id).await?;

        // Serialize checkpoint
        let (data, size) = self.serialize(checkpoint)?;

        // Update metadata with size
        let mut metadata = checkpoint.metadata.clone();
        metadata.size_bytes = Some(size);
        metadata.is_compressed = self.compress;

        // Write checkpoint file
        let checkpoint_path = self.checkpoint_path(&checkpoint.session_id, &checkpoint.id);
        fs::write(&checkpoint_path, &data).await.map_err(|e| {
            CheckpointError::StorageError(format!("Failed to write checkpoint: {}", e))
        })?;

        // Write metadata file for fast listing
        let metadata_path = self.metadata_path(&checkpoint.session_id, &checkpoint.id);
        let metadata_json = serde_json::to_vec_pretty(&CheckpointMetadataWithId {
            id: checkpoint.id.clone(),
            session_id: checkpoint.session_id.clone(),
            created_at: checkpoint.created_at,
            metadata,
        })
        .map_err(|e| CheckpointError::SerializationError(e.to_string()))?;

        fs::write(&metadata_path, &metadata_json)
            .await
            .map_err(|e| {
                CheckpointError::StorageError(format!("Failed to write metadata: {}", e))
            })?;

        tracing::debug!(
            checkpoint_id = %checkpoint.id,
            session_id = %checkpoint.session_id,
            size_bytes = size,
            compressed = self.compress,
            "Checkpoint saved"
        );

        Ok(checkpoint.id.clone())
    }

    async fn load(&self, checkpoint_id: &str) -> Result<Checkpoint, CheckpointError> {
        // First, try to find the checkpoint in any session directory
        let sessions_dir = self.base_path.join("sessions");

        if !sessions_dir.exists() {
            return Err(CheckpointError::NotFound(checkpoint_id.to_string()));
        }

        let mut entries = fs::read_dir(&sessions_dir)
            .await
            .map_err(|e| CheckpointError::StorageError(e.to_string()))?;

        while let Ok(Some(entry)) = entries.next_entry().await {
            let session_id = entry.file_name().to_string_lossy().to_string();

            // Check for uncompressed file
            let path = self.checkpoint_path(&session_id, checkpoint_id);
            if path.exists() {
                let data = fs::read(&path)
                    .await
                    .map_err(|e| CheckpointError::StorageError(e.to_string()))?;
                let checkpoint = self.deserialize(&data, self.compress)?;
                checkpoint.validate()?;
                return Ok(checkpoint);
            }

            // Check for compressed file (in case compression setting changed)
            let compressed_path = self
                .session_dir(&session_id)
                .join(format!("{}.json.gz", checkpoint_id));
            if compressed_path.exists() {
                let data = fs::read(&compressed_path)
                    .await
                    .map_err(|e| CheckpointError::StorageError(e.to_string()))?;
                let checkpoint = self.deserialize(&data, true)?;
                checkpoint.validate()?;
                return Ok(checkpoint);
            }

            // Check for uncompressed file (in case compression setting changed)
            let uncompressed_path = self
                .session_dir(&session_id)
                .join(format!("{}.json", checkpoint_id));
            if uncompressed_path.exists() {
                let data = fs::read(&uncompressed_path)
                    .await
                    .map_err(|e| CheckpointError::StorageError(e.to_string()))?;
                let checkpoint = self.deserialize(&data, false)?;
                checkpoint.validate()?;
                return Ok(checkpoint);
            }
        }

        Err(CheckpointError::NotFound(checkpoint_id.to_string()))
    }

    async fn list(&self, session_id: &str) -> Result<Vec<CheckpointMetadata>, CheckpointError> {
        let session_dir = self.session_dir(session_id);

        if !session_dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = fs::read_dir(&session_dir)
            .await
            .map_err(|e| CheckpointError::StorageError(e.to_string()))?;

        let mut checkpoints = Vec::new();

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();

            // Only process metadata files
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".meta.json"))
                    .unwrap_or(false)
            {
                if let Ok(data) = fs::read(&path).await {
                    if let Ok(meta_with_id) =
                        serde_json::from_slice::<CheckpointMetadataWithId>(&data)
                    {
                        checkpoints.push((meta_with_id.created_at, meta_with_id.metadata));
                    }
                }
            }
        }

        // Sort by created_at descending (newest first)
        checkpoints.sort_by(|a, b| b.0.cmp(&a.0));

        Ok(checkpoints.into_iter().map(|(_, m)| m).collect())
    }

    async fn delete(&self, checkpoint_id: &str) -> Result<(), CheckpointError> {
        let sessions_dir = self.base_path.join("sessions");

        if !sessions_dir.exists() {
            return Err(CheckpointError::NotFound(checkpoint_id.to_string()));
        }

        let mut entries = fs::read_dir(&sessions_dir)
            .await
            .map_err(|e| CheckpointError::StorageError(e.to_string()))?;

        while let Ok(Some(entry)) = entries.next_entry().await {
            let session_id = entry.file_name().to_string_lossy().to_string();

            // Try to delete all possible file extensions
            let files_to_try = vec![
                self.checkpoint_path(&session_id, checkpoint_id),
                self.session_dir(&session_id)
                    .join(format!("{}.json", checkpoint_id)),
                self.session_dir(&session_id)
                    .join(format!("{}.json.gz", checkpoint_id)),
                self.metadata_path(&session_id, checkpoint_id),
            ];

            let mut found = false;
            for path in files_to_try {
                if path.exists() {
                    fs::remove_file(&path)
                        .await
                        .map_err(|e| CheckpointError::StorageError(e.to_string()))?;
                    found = true;
                }
            }

            if found {
                tracing::debug!(
                    checkpoint_id = %checkpoint_id,
                    session_id = %session_id,
                    "Checkpoint deleted"
                );
                return Ok(());
            }
        }

        Err(CheckpointError::NotFound(checkpoint_id.to_string()))
    }

    async fn delete_all(&self, session_id: &str) -> Result<usize, CheckpointError> {
        let session_dir = self.session_dir(session_id);

        if !session_dir.exists() {
            return Ok(0);
        }

        let mut entries = fs::read_dir(&session_dir)
            .await
            .map_err(|e| CheckpointError::StorageError(e.to_string()))?;

        let mut count = 0;

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file() {
                fs::remove_file(&path)
                    .await
                    .map_err(|e| CheckpointError::StorageError(e.to_string()))?;

                // Count only checkpoint files (not metadata files)
                if !path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".meta.json"))
                    .unwrap_or(false)
                {
                    count += 1;
                }
            }
        }

        // Remove empty session directory
        let _ = fs::remove_dir(&session_dir).await;

        tracing::debug!(
            session_id = %session_id,
            count = count,
            "All checkpoints deleted for session"
        );

        Ok(count)
    }

    async fn get_latest(&self, session_id: &str) -> Result<Option<Checkpoint>, CheckpointError> {
        let checkpoints = self.list(session_id).await?;

        if checkpoints.is_empty() {
            return Ok(None);
        }

        // List returns sorted by newest first, so we need to find the ID
        let session_dir = self.session_dir(session_id);

        if !session_dir.exists() {
            return Ok(None);
        }

        let mut entries = fs::read_dir(&session_dir)
            .await
            .map_err(|e| CheckpointError::StorageError(e.to_string()))?;

        let mut latest: Option<(DateTime<Utc>, String)> = None;

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();

            // Only process metadata files
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".meta.json"))
                    .unwrap_or(false)
            {
                if let Ok(data) = fs::read(&path).await {
                    if let Ok(meta_with_id) =
                        serde_json::from_slice::<CheckpointMetadataWithId>(&data)
                    {
                        match &latest {
                            None => latest = Some((meta_with_id.created_at, meta_with_id.id)),
                            Some((current_time, _)) if meta_with_id.created_at > *current_time => {
                                latest = Some((meta_with_id.created_at, meta_with_id.id));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        match latest {
            Some((_, id)) => self.load(&id).await.map(Some),
            None => Ok(None),
        }
    }
}

/// Extended metadata for storage (includes id and timestamps)
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckpointMetadataWithId {
    id: String,
    session_id: String,
    created_at: DateTime<Utc>,
    #[serde(flatten)]
    metadata: CheckpointMetadata,
}

// ============================================================================
// In-Memory Checkpoint Manager (for testing)
// ============================================================================

/// In-memory checkpoint manager for testing
pub struct MemoryCheckpointManager {
    checkpoints: tokio::sync::RwLock<HashMap<String, Checkpoint>>,
}

impl Default for MemoryCheckpointManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryCheckpointManager {
    /// Create a new memory checkpoint manager
    pub fn new() -> Self {
        Self {
            checkpoints: tokio::sync::RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl CheckpointManager for MemoryCheckpointManager {
    async fn save(&self, checkpoint: &Checkpoint) -> Result<String, CheckpointError> {
        checkpoint.validate()?;
        self.checkpoints
            .write()
            .await
            .insert(checkpoint.id.clone(), checkpoint.clone());
        Ok(checkpoint.id.clone())
    }

    async fn load(&self, checkpoint_id: &str) -> Result<Checkpoint, CheckpointError> {
        self.checkpoints
            .read()
            .await
            .get(checkpoint_id)
            .cloned()
            .ok_or_else(|| CheckpointError::NotFound(checkpoint_id.to_string()))
    }

    async fn list(&self, session_id: &str) -> Result<Vec<CheckpointMetadata>, CheckpointError> {
        let checkpoints = self.checkpoints.read().await;
        let mut results: Vec<_> = checkpoints
            .values()
            .filter(|c| c.session_id == session_id)
            .map(|c| (c.created_at, c.metadata.clone()))
            .collect();

        // Sort by created_at descending
        results.sort_by(|a, b| b.0.cmp(&a.0));

        Ok(results.into_iter().map(|(_, m)| m).collect())
    }

    async fn delete(&self, checkpoint_id: &str) -> Result<(), CheckpointError> {
        self.checkpoints
            .write()
            .await
            .remove(checkpoint_id)
            .map(|_| ())
            .ok_or_else(|| CheckpointError::NotFound(checkpoint_id.to_string()))
    }

    async fn delete_all(&self, session_id: &str) -> Result<usize, CheckpointError> {
        let mut checkpoints = self.checkpoints.write().await;
        let ids_to_remove: Vec<_> = checkpoints
            .iter()
            .filter(|(_, c)| c.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect();

        let count = ids_to_remove.len();
        for id in ids_to_remove {
            checkpoints.remove(&id);
        }

        Ok(count)
    }

    async fn get_latest(&self, session_id: &str) -> Result<Option<Checkpoint>, CheckpointError> {
        let checkpoints = self.checkpoints.read().await;
        let latest = checkpoints
            .values()
            .filter(|c| c.session_id == session_id)
            .max_by_key(|c| c.created_at)
            .cloned();

        Ok(latest)
    }
}

// ============================================================================
// Auto-Checkpoint Configuration and Triggers
// ============================================================================

/// Configuration for automatic checkpointing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoCheckpointConfig {
    /// Enable auto-checkpointing
    pub enabled: bool,
    /// Create checkpoint before dangerous operations
    pub before_dangerous_ops: bool,
    /// Dangerous operations that trigger checkpoints
    pub dangerous_operations: Vec<DangerousOperation>,
    /// Create periodic checkpoints every N turns
    pub periodic_turns: Option<u32>,
    /// Create checkpoint before context compaction
    pub before_compaction: bool,
    /// Maximum number of checkpoints to keep per session
    pub max_checkpoints_per_session: Option<usize>,
}

impl Default for AutoCheckpointConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            before_dangerous_ops: true,
            dangerous_operations: vec![
                DangerousOperation::FileWrite,
                DangerousOperation::FileDelete,
                DangerousOperation::BashExec,
            ],
            periodic_turns: Some(10),
            before_compaction: true,
            max_checkpoints_per_session: Some(50),
        }
    }
}

/// Types of dangerous operations that can trigger checkpoints
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DangerousOperation {
    /// File write/create operations
    FileWrite,
    /// File delete operations
    FileDelete,
    /// Bash command execution
    BashExec,
    /// Git operations (commit, push, etc.)
    GitOperation,
    /// Database modifications
    DatabaseWrite,
}

/// Auto-checkpoint trigger logic
#[derive(Clone)]
pub struct AutoCheckpointTrigger {
    config: AutoCheckpointConfig,
    turns_since_checkpoint: u32,
}

impl AutoCheckpointTrigger {
    /// Create a new auto-checkpoint trigger
    pub fn new(config: AutoCheckpointConfig) -> Self {
        Self {
            config,
            turns_since_checkpoint: 0,
        }
    }

    /// Check if a checkpoint should be created before a tool execution
    pub fn should_checkpoint_before_tool(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> Option<CheckpointTrigger> {
        if !self.config.enabled || !self.config.before_dangerous_ops {
            return None;
        }

        // Check if this is a dangerous operation
        for op in &self.config.dangerous_operations {
            if self.is_dangerous_operation(tool_name, input, *op) {
                return Some(CheckpointTrigger::BeforeDangerousOperation);
            }
        }

        None
    }

    /// Check if a checkpoint should be created due to turn count
    pub fn should_checkpoint_periodic(&mut self) -> Option<CheckpointTrigger> {
        if !self.config.enabled {
            return None;
        }

        self.turns_since_checkpoint += 1;

        if let Some(interval) = self.config.periodic_turns {
            if self.turns_since_checkpoint >= interval {
                self.turns_since_checkpoint = 0;
                return Some(CheckpointTrigger::Periodic);
            }
        }

        None
    }

    /// Check if a checkpoint should be created before compaction
    pub fn should_checkpoint_before_compaction(&self) -> Option<CheckpointTrigger> {
        if !self.config.enabled || !self.config.before_compaction {
            return None;
        }

        Some(CheckpointTrigger::BeforeCompaction)
    }

    /// Reset turn counter (after checkpoint is created)
    pub fn reset_periodic_counter(&mut self) {
        self.turns_since_checkpoint = 0;
    }

    /// Check if a tool operation is considered dangerous
    fn is_dangerous_operation(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        op: DangerousOperation,
    ) -> bool {
        match op {
            DangerousOperation::FileWrite => tool_name == "Write" || tool_name == "Edit",
            DangerousOperation::FileDelete => {
                if tool_name == "Bash" {
                    if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                        return cmd.contains("rm ") || cmd.contains("unlink ");
                    }
                }
                false
            }
            DangerousOperation::BashExec => tool_name == "Bash",
            DangerousOperation::GitOperation => {
                if tool_name == "Bash" {
                    if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                        return cmd.contains("git commit")
                            || cmd.contains("git push")
                            || cmd.contains("git reset")
                            || cmd.contains("git rebase");
                    }
                }
                false
            }
            DangerousOperation::DatabaseWrite => {
                // Could be extended for database tools
                false
            }
        }
    }
}

// ============================================================================
// Session Restore
// ============================================================================

/// Result of restoring a session from a checkpoint
#[derive(Debug)]
pub struct RestoreResult {
    /// Checkpoint that was restored
    pub checkpoint: Checkpoint,
    /// Messages that were restored
    pub messages: Vec<AgentMessage>,
    /// Tool states that were restored
    pub tool_states: HashMap<String, serde_json::Value>,
    /// Any warnings during restore
    pub warnings: Vec<String>,
}

/// Restore a session from a checkpoint
pub fn restore_from_checkpoint(checkpoint: Checkpoint) -> Result<RestoreResult, CheckpointError> {
    checkpoint.validate()?;

    let mut warnings = Vec::new();

    // Check for missing tool states (non-fatal)
    // Tools might have changed since checkpoint
    for tool_name in checkpoint.tool_states.keys() {
        // In a real implementation, we'd check if the tool still exists
        tracing::debug!(tool_name = %tool_name, "Restoring tool state");
    }

    // Check schema compatibility
    if checkpoint.metadata.schema_version < CHECKPOINT_SCHEMA_VERSION {
        warnings.push(format!(
            "Checkpoint schema version {} is older than current version {}. Some features may not work correctly.",
            checkpoint.metadata.schema_version,
            CHECKPOINT_SCHEMA_VERSION
        ));
    }

    Ok(RestoreResult {
        messages: checkpoint.conversation_history.clone(),
        tool_states: checkpoint.tool_states.clone(),
        warnings,
        checkpoint,
    })
}

// ============================================================================
// Errors
// ============================================================================

/// Checkpoint-related errors
#[derive(Debug)]
pub enum CheckpointError {
    /// Checkpoint not found
    NotFound(String),
    /// Storage error
    StorageError(String),
    /// Serialization error
    SerializationError(String),
    /// Deserialization error
    DeserializationError(String),
    /// Compression error
    CompressionError(String),
    /// Decompression error
    DecompressionError(String),
    /// Invalid checkpoint data
    InvalidCheckpoint(String),
    /// Incompatible schema version
    IncompatibleVersion {
        checkpoint_version: u32,
        current_version: u32,
    },
}

impl std::fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "Checkpoint not found: {}", id),
            Self::StorageError(msg) => write!(f, "Storage error: {}", msg),
            Self::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            Self::DeserializationError(msg) => write!(f, "Deserialization error: {}", msg),
            Self::CompressionError(msg) => write!(f, "Compression error: {}", msg),
            Self::DecompressionError(msg) => write!(f, "Decompression error: {}", msg),
            Self::InvalidCheckpoint(msg) => write!(f, "Invalid checkpoint: {}", msg),
            Self::IncompatibleVersion {
                checkpoint_version,
                current_version,
            } => write!(
                f,
                "Incompatible checkpoint version: {} (current: {})",
                checkpoint_version, current_version
            ),
        }
    }
}

impl std::error::Error for CheckpointError {}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::{MessageContent, UserMessage};

    fn create_test_messages(count: usize) -> Vec<AgentMessage> {
        (0..count)
            .map(|i| {
                AgentMessage::User(UserMessage {
                    content: MessageContent::text(format!("Test message {}", i)),
                    uuid: Some(uuid::Uuid::new_v4()),
                    parent_tool_use_id: None,
                    tool_use_result: None,
                })
            })
            .collect()
    }

    fn create_test_context() -> ContextState {
        ContextState {
            cwd: Some("/tmp/test".to_string()),
            env: HashMap::new(),
            estimated_tokens: 100,
            turn_count: 5,
            usage: Usage::default(),
            total_cost_usd: 0.01,
            custom: HashMap::new(),
        }
    }

    #[test]
    fn test_checkpoint_creation() {
        let messages = create_test_messages(5);
        let context = create_test_context();

        let checkpoint = Checkpoint::new("session-1", messages.clone(), context);

        assert!(!checkpoint.id.is_empty());
        assert_eq!(checkpoint.session_id, "session-1");
        assert_eq!(checkpoint.conversation_history.len(), 5);
        assert_eq!(checkpoint.metadata.message_count, 5);
    }

    #[test]
    fn test_checkpoint_builder() {
        let messages = create_test_messages(3);
        let context = create_test_context();

        let checkpoint = Checkpoint::new("session-1", messages, context)
            .with_trigger(CheckpointTrigger::BeforeDangerousOperation)
            .with_label("Before file edit")
            .with_tags(vec!["important".to_string(), "backup".to_string()]);

        assert_eq!(
            checkpoint.metadata.trigger,
            CheckpointTrigger::BeforeDangerousOperation
        );
        assert_eq!(
            checkpoint.metadata.label,
            Some("Before file edit".to_string())
        );
        assert_eq!(checkpoint.metadata.tags.len(), 2);
    }

    #[test]
    fn test_checkpoint_validation() {
        let messages = create_test_messages(1);
        let context = create_test_context();

        let checkpoint = Checkpoint::new("session-1", messages, context);
        assert!(checkpoint.validate().is_ok());

        // Test empty session ID
        let invalid = Checkpoint {
            session_id: String::new(),
            ..checkpoint.clone()
        };
        assert!(invalid.validate().is_err());

        // Test empty checkpoint ID
        let invalid = Checkpoint {
            id: String::new(),
            ..checkpoint
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn test_checkpoint_with_tool_states() {
        let messages = create_test_messages(1);
        let context = create_test_context();

        let mut tool_states = HashMap::new();
        tool_states.insert("Bash".to_string(), serde_json::json!({"cwd": "/tmp"}));
        tool_states.insert("Git".to_string(), serde_json::json!({"branch": "main"}));

        let checkpoint =
            Checkpoint::new("session-1", messages, context).with_tool_states(tool_states);

        assert_eq!(checkpoint.tool_states.len(), 2);
        assert!(checkpoint.tool_states.contains_key("Bash"));
    }

    #[tokio::test]
    async fn test_memory_checkpoint_manager() {
        let manager = MemoryCheckpointManager::new();
        let messages = create_test_messages(5);
        let context = create_test_context();

        let checkpoint = Checkpoint::new("session-1", messages, context);
        let id = checkpoint.id.clone();

        // Save
        let saved_id = manager.save(&checkpoint).await.unwrap();
        assert_eq!(saved_id, id);

        // Load
        let loaded = manager.load(&id).await.unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.session_id, "session-1");

        // List
        let list = manager.list("session-1").await.unwrap();
        assert_eq!(list.len(), 1);

        // Get latest
        let latest = manager.get_latest("session-1").await.unwrap();
        assert!(latest.is_some());

        // Delete
        manager.delete(&id).await.unwrap();
        assert!(manager.load(&id).await.is_err());
    }

    #[tokio::test]
    async fn test_memory_checkpoint_manager_multiple() {
        let manager = MemoryCheckpointManager::new();

        // Create multiple checkpoints
        for i in 0..5 {
            let messages = create_test_messages(i + 1);
            let context = create_test_context();
            let checkpoint = Checkpoint::new("session-1", messages, context);
            manager.save(&checkpoint).await.unwrap();
        }

        // List should return all
        let list = manager.list("session-1").await.unwrap();
        assert_eq!(list.len(), 5);

        // Delete all
        let count = manager.delete_all("session-1").await.unwrap();
        assert_eq!(count, 5);

        // List should be empty
        let list = manager.list("session-1").await.unwrap();
        assert_eq!(list.len(), 0);
    }

    #[tokio::test]
    async fn test_file_checkpoint_manager() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let manager = FileCheckpointManager::new(temp_dir.path());

        let messages = create_test_messages(3);
        let context = create_test_context();
        let checkpoint = Checkpoint::new("session-1", messages, context);
        let id = checkpoint.id.clone();

        // Save
        let saved_id = manager.save(&checkpoint).await.unwrap();
        assert_eq!(saved_id, id);

        // Load
        let loaded = manager.load(&id).await.unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.conversation_history.len(), 3);

        // List
        let list = manager.list("session-1").await.unwrap();
        assert_eq!(list.len(), 1);

        // Delete
        manager.delete(&id).await.unwrap();
        assert!(manager.load(&id).await.is_err());
    }

    #[tokio::test]
    async fn test_file_checkpoint_manager_compression() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let manager = FileCheckpointManager::new(temp_dir.path()).with_compression(true);

        let messages = create_test_messages(10);
        let context = create_test_context();
        let checkpoint = Checkpoint::new("session-1", messages, context);
        let id = checkpoint.id.clone();

        // Save with compression
        manager.save(&checkpoint).await.unwrap();

        // Load
        let loaded = manager.load(&id).await.unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.conversation_history.len(), 10);
    }

    #[test]
    fn test_auto_checkpoint_trigger_periodic() {
        let config = AutoCheckpointConfig {
            enabled: true,
            periodic_turns: Some(3),
            ..Default::default()
        };
        let mut trigger = AutoCheckpointTrigger::new(config);

        // First two turns: no checkpoint
        assert!(trigger.should_checkpoint_periodic().is_none());
        assert!(trigger.should_checkpoint_periodic().is_none());

        // Third turn: checkpoint triggered
        let result = trigger.should_checkpoint_periodic();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), CheckpointTrigger::Periodic);

        // Counter reset, next three turns before checkpoint again
        assert!(trigger.should_checkpoint_periodic().is_none());
        assert!(trigger.should_checkpoint_periodic().is_none());
        assert!(trigger.should_checkpoint_periodic().is_some());
    }

    #[test]
    fn test_auto_checkpoint_trigger_dangerous_ops() {
        let config = AutoCheckpointConfig::default();
        let trigger = AutoCheckpointTrigger::new(config);

        // Write tool should trigger
        let result = trigger
            .should_checkpoint_before_tool("Write", &serde_json::json!({"file_path": "/tmp/test"}));
        assert_eq!(result, Some(CheckpointTrigger::BeforeDangerousOperation));

        // Bash should trigger
        let result = trigger
            .should_checkpoint_before_tool("Bash", &serde_json::json!({"command": "ls -la"}));
        assert_eq!(result, Some(CheckpointTrigger::BeforeDangerousOperation));

        // Read should not trigger
        let result = trigger
            .should_checkpoint_before_tool("Read", &serde_json::json!({"file_path": "/tmp/test"}));
        assert!(result.is_none());
    }

    #[test]
    fn test_auto_checkpoint_trigger_disabled() {
        let config = AutoCheckpointConfig {
            enabled: false,
            ..Default::default()
        };
        let mut trigger = AutoCheckpointTrigger::new(config);

        // Nothing should trigger when disabled
        assert!(trigger.should_checkpoint_periodic().is_none());
        assert!(trigger
            .should_checkpoint_before_tool("Write", &serde_json::json!({}))
            .is_none());
        assert!(trigger.should_checkpoint_before_compaction().is_none());
    }

    #[test]
    fn test_restore_from_checkpoint() {
        let messages = create_test_messages(3);
        let context = create_test_context();

        let mut tool_states = HashMap::new();
        tool_states.insert("Bash".to_string(), serde_json::json!({"cwd": "/tmp"}));

        let checkpoint =
            Checkpoint::new("session-1", messages.clone(), context).with_tool_states(tool_states);

        let result = restore_from_checkpoint(checkpoint).unwrap();

        assert_eq!(result.messages.len(), 3);
        assert_eq!(result.tool_states.len(), 1);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_context_state_serialization() {
        let context = ContextState {
            cwd: Some("/test".to_string()),
            env: {
                let mut env = HashMap::new();
                env.insert("PATH".to_string(), "/usr/bin".to_string());
                env
            },
            estimated_tokens: 500,
            turn_count: 10,
            usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
                ..Default::default()
            },
            total_cost_usd: 0.05,
            custom: HashMap::new(),
        };

        let json = serde_json::to_string(&context).unwrap();
        let deserialized: ContextState = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.cwd, Some("/test".to_string()));
        assert_eq!(deserialized.estimated_tokens, 500);
        assert_eq!(deserialized.turn_count, 10);
    }

    #[test]
    fn test_checkpoint_metadata_serialization() {
        let metadata = CheckpointMetadata {
            label: Some("Test checkpoint".to_string()),
            trigger: CheckpointTrigger::BeforeDangerousOperation,
            message_count: 10,
            is_compressed: true,
            size_bytes: Some(1024),
            schema_version: 1,
            tags: vec!["important".to_string()],
        };

        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: CheckpointMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.label, Some("Test checkpoint".to_string()));
        assert_eq!(
            deserialized.trigger,
            CheckpointTrigger::BeforeDangerousOperation
        );
        assert!(deserialized.is_compressed);
    }

    #[test]
    fn test_dangerous_operation_detection() {
        let config = AutoCheckpointConfig::default();
        let trigger = AutoCheckpointTrigger::new(config);

        // Git operations
        let git_push = serde_json::json!({"command": "git push origin main"});
        assert!(trigger.is_dangerous_operation(
            "Bash",
            &git_push,
            DangerousOperation::GitOperation
        ));

        let git_commit = serde_json::json!({"command": "git commit -m 'test'"});
        assert!(trigger.is_dangerous_operation(
            "Bash",
            &git_commit,
            DangerousOperation::GitOperation
        ));

        // File delete
        let rm_cmd = serde_json::json!({"command": "rm -rf /tmp/test"});
        assert!(trigger.is_dangerous_operation("Bash", &rm_cmd, DangerousOperation::FileDelete));

        // Non-dangerous
        let ls_cmd = serde_json::json!({"command": "ls -la"});
        assert!(!trigger.is_dangerous_operation("Bash", &ls_cmd, DangerousOperation::FileDelete));
        assert!(!trigger.is_dangerous_operation("Bash", &ls_cmd, DangerousOperation::GitOperation));
    }

    #[tokio::test]
    async fn test_checkpoint_not_found() {
        let manager = MemoryCheckpointManager::new();
        let result = manager.load("nonexistent").await;
        assert!(matches!(result, Err(CheckpointError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_checkpoint_delete_not_found() {
        let manager = MemoryCheckpointManager::new();
        let result = manager.delete("nonexistent").await;
        assert!(matches!(result, Err(CheckpointError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_get_latest_empty_session() {
        let manager = MemoryCheckpointManager::new();
        let latest = manager.get_latest("nonexistent-session").await.unwrap();
        assert!(latest.is_none());
    }
}
