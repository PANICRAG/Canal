//! Rollback System for Agent Checkpoints
//!
//! This module provides rollback functionality for undoing tool operations
//! when errors occur or user requests a rollback.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::checkpoint::{Checkpoint, CheckpointManager};

/// Validate that a path from a rollback record does not contain path traversal.
/// Rejects paths with `..`, null bytes, or non-absolute paths that could escape
/// a sandbox directory.
fn validate_rollback_path(path: &str) -> Result<(), RollbackError> {
    if path.contains("..") || path.contains('\0') {
        return Err(RollbackError::ExecutionFailed {
            operation_id: "path_validation".to_string(),
            error: format!("Unsafe path rejected: {}", path),
        });
    }
    Ok(())
}

// ============================================================================
// Rollback Handler Trait
// ============================================================================

/// Trait for implementing tool-specific rollback logic
#[async_trait]
pub trait RollbackHandler: Send + Sync {
    /// Get the tool name this handler supports
    fn tool_name(&self) -> &str;

    /// Check if this operation can be rolled back
    fn can_rollback(&self, operation: &RollbackableOperation) -> bool;

    /// Execute the rollback
    async fn rollback(
        &self,
        operation: &RollbackableOperation,
    ) -> Result<RollbackResult, RollbackError>;

    /// Get the rollback complexity (higher = more risky/complex)
    fn complexity(&self) -> RollbackComplexity {
        RollbackComplexity::Medium
    }
}

/// Complexity level for rollback operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackComplexity {
    /// Simple undo (e.g., delete a created file)
    Simple,
    /// Medium complexity (e.g., restore file from backup)
    Medium,
    /// Complex rollback (e.g., undo multiple related operations)
    Complex,
    /// Cannot be automatically rolled back
    Manual,
}

// ============================================================================
// Rollback Operation Types
// ============================================================================

/// An operation that can potentially be rolled back
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackableOperation {
    /// Unique operation ID
    pub id: String,
    /// Tool that performed the operation
    pub tool_name: String,
    /// Input parameters used
    pub input: serde_json::Value,
    /// Output/result of the operation
    pub output: serde_json::Value,
    /// State before the operation (for restoration)
    pub pre_state: Option<PreOperationState>,
    /// Timestamp when operation was performed
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Associated checkpoint ID
    pub checkpoint_id: Option<String>,
    /// Session ID
    pub session_id: String,
}

impl RollbackableOperation {
    /// Create a new rollbackable operation
    pub fn new(
        tool_name: impl Into<String>,
        input: serde_json::Value,
        output: serde_json::Value,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            id: format!("op_{}", uuid::Uuid::new_v4().to_string().replace("-", "")),
            tool_name: tool_name.into(),
            input,
            output,
            pre_state: None,
            timestamp: chrono::Utc::now(),
            checkpoint_id: None,
            session_id: session_id.into(),
        }
    }

    /// Set pre-operation state for rollback
    pub fn with_pre_state(mut self, state: PreOperationState) -> Self {
        self.pre_state = Some(state);
        self
    }

    /// Set the associated checkpoint
    pub fn with_checkpoint(mut self, checkpoint_id: impl Into<String>) -> Self {
        self.checkpoint_id = Some(checkpoint_id.into());
        self
    }
}

/// State captured before an operation for rollback purposes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PreOperationState {
    /// File content before modification
    FileContent {
        path: String,
        content: String,
        permissions: Option<u32>,
    },
    /// File did not exist before (was created)
    FileCreated { path: String },
    /// Directory contents before modification
    DirectoryState { path: String, entries: Vec<String> },
    /// Git state before operation
    GitState {
        branch: String,
        commit_hash: String,
        staged_files: Vec<String>,
    },
    /// Custom state data
    Custom { data: serde_json::Value },
}

// ============================================================================
// Rollback Result
// ============================================================================

/// Result of a rollback operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackResult {
    /// Operation ID that was rolled back
    pub operation_id: String,
    /// Whether rollback was successful
    pub success: bool,
    /// Description of what was done
    pub description: String,
    /// Any warnings during rollback
    pub warnings: Vec<String>,
    /// Side effects of the rollback
    pub side_effects: Vec<String>,
}

impl RollbackResult {
    /// Create a successful rollback result
    pub fn success(operation_id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            success: true,
            description: description.into(),
            warnings: Vec::new(),
            side_effects: Vec::new(),
        }
    }

    /// Create a failed rollback result
    pub fn failure(operation_id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            success: false,
            description: description.into(),
            warnings: Vec::new(),
            side_effects: Vec::new(),
        }
    }

    /// Add a warning
    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    /// Add a side effect
    pub fn with_side_effect(mut self, effect: impl Into<String>) -> Self {
        self.side_effects.push(effect.into());
        self
    }
}

// ============================================================================
// Rollback Error
// ============================================================================

/// Errors that can occur during rollback
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RollbackError {
    /// Operation cannot be rolled back
    NotRollbackable {
        operation_id: String,
        reason: String,
    },
    /// Pre-state is missing
    MissingPreState { operation_id: String },
    /// Rollback failed during execution
    ExecutionFailed { operation_id: String, error: String },
    /// State has changed since operation (conflict)
    StateConflict {
        operation_id: String,
        description: String,
    },
    /// Handler not found for tool
    HandlerNotFound { tool_name: String },
    /// Checkpoint not found
    CheckpointNotFound { checkpoint_id: String },
}

impl std::fmt::Display for RollbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotRollbackable {
                operation_id,
                reason,
            } => {
                write!(
                    f,
                    "Operation {} cannot be rolled back: {}",
                    operation_id, reason
                )
            }
            Self::MissingPreState { operation_id } => {
                write!(f, "Missing pre-state for operation {}", operation_id)
            }
            Self::ExecutionFailed {
                operation_id,
                error,
            } => {
                write!(
                    f,
                    "Rollback execution failed for {}: {}",
                    operation_id, error
                )
            }
            Self::StateConflict {
                operation_id,
                description,
            } => {
                write!(f, "State conflict for {}: {}", operation_id, description)
            }
            Self::HandlerNotFound { tool_name } => {
                write!(f, "No rollback handler found for tool: {}", tool_name)
            }
            Self::CheckpointNotFound { checkpoint_id } => {
                write!(f, "Checkpoint not found: {}", checkpoint_id)
            }
        }
    }
}

impl std::error::Error for RollbackError {}

// ============================================================================
// Checkpoint Rollback Manager
// ============================================================================

/// Manager for checkpoint-based rollback operations
pub struct CheckpointRollbackManager {
    /// Checkpoint storage
    checkpoint_manager: Arc<dyn CheckpointManager>,
    /// Rollback handlers by tool name
    handlers: Arc<RwLock<HashMap<String, Arc<dyn RollbackHandler>>>>,
    /// Operation history for rollback
    operations: Arc<RwLock<Vec<RollbackableOperation>>>,
    /// Maximum operations to keep
    max_operations: usize,
}

impl CheckpointRollbackManager {
    /// Create a new checkpoint rollback manager
    pub fn new(checkpoint_manager: Arc<dyn CheckpointManager>) -> Self {
        Self {
            checkpoint_manager,
            handlers: Arc::new(RwLock::new(HashMap::new())),
            operations: Arc::new(RwLock::new(Vec::new())),
            max_operations: 100,
        }
    }

    /// Set maximum operations to keep
    pub fn with_max_operations(mut self, max: usize) -> Self {
        self.max_operations = max;
        self
    }

    /// Register a rollback handler
    pub async fn register_handler(&self, handler: Arc<dyn RollbackHandler>) {
        let mut handlers = self.handlers.write().await;
        handlers.insert(handler.tool_name().to_string(), handler);
    }

    /// Record an operation for potential rollback
    pub async fn record_operation(&self, operation: RollbackableOperation) {
        let mut operations = self.operations.write().await;
        operations.push(operation);

        // R1-M107: Trim old operations using drain instead of O(n) remove(0) loop
        if operations.len() > self.max_operations {
            let drain_count = operations.len() - self.max_operations;
            operations.drain(..drain_count);
        }
    }

    /// Get operations since a checkpoint
    pub async fn get_operations_since_checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> Vec<RollbackableOperation> {
        let operations = self.operations.read().await;

        // Find the checkpoint boundary
        let checkpoint_idx = operations
            .iter()
            .position(|op| op.checkpoint_id.as_deref() == Some(checkpoint_id));

        match checkpoint_idx {
            Some(idx) => operations[idx..].to_vec(),
            None => Vec::new(),
        }
    }

    /// Rollback to a specific checkpoint
    pub async fn rollback_to_checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> Result<CheckpointRollbackResult, RollbackError> {
        // Load the checkpoint
        let checkpoint = self
            .checkpoint_manager
            .load(checkpoint_id)
            .await
            .map_err(|_| RollbackError::CheckpointNotFound {
                checkpoint_id: checkpoint_id.to_string(),
            })?;

        // Get operations to rollback (in reverse order)
        let operations = self.get_operations_since_checkpoint(checkpoint_id).await;
        let operations_to_rollback: Vec<_> = operations.into_iter().rev().collect();

        let mut results = Vec::new();
        let mut warnings = Vec::new();

        // Rollback each operation
        for operation in operations_to_rollback {
            match self.rollback_operation(&operation).await {
                Ok(result) => {
                    warnings.extend(result.warnings.clone());
                    results.push(result);
                }
                Err(RollbackError::NotRollbackable { reason, .. }) => {
                    warnings.push(format!(
                        "Operation {} could not be rolled back: {}",
                        operation.id, reason
                    ));
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        // Clear operations after the checkpoint
        {
            let mut ops = self.operations.write().await;
            ops.retain(|op| op.checkpoint_id.as_deref() != Some(checkpoint_id));
        }

        Ok(CheckpointRollbackResult {
            checkpoint_id: checkpoint_id.to_string(),
            checkpoint,
            operations_rolled_back: results.len(),
            results,
            warnings,
        })
    }

    /// Rollback a single operation
    pub async fn rollback_operation(
        &self,
        operation: &RollbackableOperation,
    ) -> Result<RollbackResult, RollbackError> {
        let handlers = self.handlers.read().await;

        let handler =
            handlers
                .get(&operation.tool_name)
                .ok_or_else(|| RollbackError::HandlerNotFound {
                    tool_name: operation.tool_name.clone(),
                })?;

        if !handler.can_rollback(operation) {
            return Err(RollbackError::NotRollbackable {
                operation_id: operation.id.clone(),
                reason: "Operation marked as not rollbackable".to_string(),
            });
        }

        handler.rollback(operation).await
    }

    /// Check if an operation can be rolled back
    pub async fn can_rollback(&self, operation: &RollbackableOperation) -> bool {
        let handlers = self.handlers.read().await;
        handlers
            .get(&operation.tool_name)
            .map(|h| h.can_rollback(operation))
            .unwrap_or(false)
    }

    /// Get the rollback complexity for an operation
    pub async fn get_rollback_complexity(
        &self,
        operation: &RollbackableOperation,
    ) -> RollbackComplexity {
        let handlers = self.handlers.read().await;
        handlers
            .get(&operation.tool_name)
            .map(|h| h.complexity())
            .unwrap_or(RollbackComplexity::Manual)
    }
}

/// Result of rolling back to a checkpoint
#[derive(Debug, Clone)]
pub struct CheckpointRollbackResult {
    /// Checkpoint ID that was restored
    pub checkpoint_id: String,
    /// The checkpoint that was restored
    pub checkpoint: Checkpoint,
    /// Number of operations rolled back
    pub operations_rolled_back: usize,
    /// Individual rollback results
    pub results: Vec<RollbackResult>,
    /// Warnings during rollback
    pub warnings: Vec<String>,
}

// ============================================================================
// Built-in Rollback Handlers
// ============================================================================

/// Rollback handler for file write operations
pub struct FileWriteRollbackHandler;

#[async_trait]
impl RollbackHandler for FileWriteRollbackHandler {
    fn tool_name(&self) -> &str {
        "Write"
    }

    fn can_rollback(&self, operation: &RollbackableOperation) -> bool {
        operation.pre_state.is_some()
    }

    async fn rollback(
        &self,
        operation: &RollbackableOperation,
    ) -> Result<RollbackResult, RollbackError> {
        let pre_state =
            operation
                .pre_state
                .as_ref()
                .ok_or_else(|| RollbackError::MissingPreState {
                    operation_id: operation.id.clone(),
                })?;

        match pre_state {
            PreOperationState::FileContent {
                path,
                content,
                permissions,
            } => {
                // Validate path before writing (prevent path traversal)
                validate_rollback_path(path)?;
                // Restore original content
                tokio::fs::write(path, content).await.map_err(|e| {
                    RollbackError::ExecutionFailed {
                        operation_id: operation.id.clone(),
                        error: e.to_string(),
                    }
                })?;

                // Restore permissions if available
                #[cfg(unix)]
                if let Some(perms) = permissions {
                    use std::os::unix::fs::PermissionsExt;
                    let metadata = tokio::fs::metadata(path).await.map_err(|e| {
                        RollbackError::ExecutionFailed {
                            operation_id: operation.id.clone(),
                            error: e.to_string(),
                        }
                    })?;
                    let mut new_perms = metadata.permissions();
                    new_perms.set_mode(*perms);
                    tokio::fs::set_permissions(path, new_perms)
                        .await
                        .map_err(|e| RollbackError::ExecutionFailed {
                            operation_id: operation.id.clone(),
                            error: e.to_string(),
                        })?;
                }
                let _ = permissions; // Silence unused warning on non-unix

                Ok(RollbackResult::success(
                    &operation.id,
                    format!("Restored original content of {}", path),
                ))
            }
            PreOperationState::FileCreated { path } => {
                // Validate path before deletion to prevent path traversal
                validate_rollback_path(path)?;
                // Delete the created file
                tokio::fs::remove_file(path)
                    .await
                    .map_err(|e| RollbackError::ExecutionFailed {
                        operation_id: operation.id.clone(),
                        error: e.to_string(),
                    })?;

                Ok(RollbackResult::success(
                    &operation.id,
                    format!("Deleted created file {}", path),
                ))
            }
            _ => Err(RollbackError::NotRollbackable {
                operation_id: operation.id.clone(),
                reason: "Unsupported pre-state type for Write tool".to_string(),
            }),
        }
    }

    fn complexity(&self) -> RollbackComplexity {
        RollbackComplexity::Simple
    }
}

/// Rollback handler for file edit operations
pub struct FileEditRollbackHandler;

#[async_trait]
impl RollbackHandler for FileEditRollbackHandler {
    fn tool_name(&self) -> &str {
        "Edit"
    }

    fn can_rollback(&self, operation: &RollbackableOperation) -> bool {
        operation.pre_state.is_some()
    }

    async fn rollback(
        &self,
        operation: &RollbackableOperation,
    ) -> Result<RollbackResult, RollbackError> {
        let pre_state =
            operation
                .pre_state
                .as_ref()
                .ok_or_else(|| RollbackError::MissingPreState {
                    operation_id: operation.id.clone(),
                })?;

        match pre_state {
            PreOperationState::FileContent { path, content, .. } => {
                validate_rollback_path(path)?;
                tokio::fs::write(path, content).await.map_err(|e| {
                    RollbackError::ExecutionFailed {
                        operation_id: operation.id.clone(),
                        error: e.to_string(),
                    }
                })?;

                Ok(RollbackResult::success(
                    &operation.id,
                    format!("Reverted edit to {}", path),
                ))
            }
            _ => Err(RollbackError::NotRollbackable {
                operation_id: operation.id.clone(),
                reason: "Unsupported pre-state type for Edit tool".to_string(),
            }),
        }
    }

    fn complexity(&self) -> RollbackComplexity {
        RollbackComplexity::Simple
    }
}

/// Rollback handler for Bash commands (limited support)
pub struct BashRollbackHandler;

#[async_trait]
impl RollbackHandler for BashRollbackHandler {
    fn tool_name(&self) -> &str {
        "Bash"
    }

    fn can_rollback(&self, operation: &RollbackableOperation) -> bool {
        // Only support rollback for certain bash commands with pre-state
        if operation.pre_state.is_none() {
            return false;
        }

        // Check if the command is in our supported list
        if let Some(cmd) = operation.input.get("command").and_then(|v| v.as_str()) {
            // Support rollback for file operations
            cmd.starts_with("mkdir ")
                || cmd.starts_with("touch ")
                || cmd.starts_with("cp ")
                || cmd.starts_with("mv ")
        } else {
            false
        }
    }

    async fn rollback(
        &self,
        operation: &RollbackableOperation,
    ) -> Result<RollbackResult, RollbackError> {
        let pre_state =
            operation
                .pre_state
                .as_ref()
                .ok_or_else(|| RollbackError::MissingPreState {
                    operation_id: operation.id.clone(),
                })?;

        let cmd = operation
            .input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RollbackError::NotRollbackable {
                operation_id: operation.id.clone(),
                reason: "Missing command in input".to_string(),
            })?;

        match pre_state {
            PreOperationState::DirectoryState { path, .. } if cmd.starts_with("mkdir ") => {
                validate_rollback_path(path)?;
                // Remove created directory
                tokio::fs::remove_dir_all(path).await.map_err(|e| {
                    RollbackError::ExecutionFailed {
                        operation_id: operation.id.clone(),
                        error: e.to_string(),
                    }
                })?;

                Ok(RollbackResult::success(
                    &operation.id,
                    format!("Removed created directory {}", path),
                ))
            }
            PreOperationState::FileCreated { path } if cmd.starts_with("touch ") => {
                validate_rollback_path(path)?;
                tokio::fs::remove_file(path)
                    .await
                    .map_err(|e| RollbackError::ExecutionFailed {
                        operation_id: operation.id.clone(),
                        error: e.to_string(),
                    })?;

                Ok(RollbackResult::success(
                    &operation.id,
                    format!("Removed created file {}", path),
                ))
            }
            _ => Err(RollbackError::NotRollbackable {
                operation_id: operation.id.clone(),
                reason: "Bash command type not supported for automatic rollback".to_string(),
            }),
        }
    }

    fn complexity(&self) -> RollbackComplexity {
        RollbackComplexity::Complex
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::session::checkpoint::MemoryCheckpointManager;

    #[test]
    fn test_rollbackable_operation() {
        let op = RollbackableOperation::new(
            "Write",
            serde_json::json!({"path": "/tmp/test.txt"}),
            serde_json::json!({"success": true}),
            "session-1",
        )
        .with_pre_state(PreOperationState::FileCreated {
            path: "/tmp/test.txt".to_string(),
        })
        .with_checkpoint("chk_123");

        assert_eq!(op.tool_name, "Write");
        assert!(op.pre_state.is_some());
        assert_eq!(op.checkpoint_id, Some("chk_123".to_string()));
    }

    #[test]
    fn test_rollback_result() {
        let result = RollbackResult::success("op_1", "Restored file")
            .with_warning("File permissions changed")
            .with_side_effect("Triggered file watcher");

        assert!(result.success);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.side_effects.len(), 1);
    }

    #[test]
    fn test_rollback_complexity_ordering() {
        assert!(RollbackComplexity::Simple < RollbackComplexity::Medium);
        assert!(RollbackComplexity::Medium < RollbackComplexity::Complex);
        assert!(RollbackComplexity::Complex < RollbackComplexity::Manual);
    }

    #[test]
    fn test_file_write_rollback_handler() {
        let handler = FileWriteRollbackHandler;
        assert_eq!(handler.tool_name(), "Write");
        assert_eq!(handler.complexity(), RollbackComplexity::Simple);

        // Without pre-state
        let op = RollbackableOperation::new(
            "Write",
            serde_json::json!({}),
            serde_json::json!({}),
            "session-1",
        );
        assert!(!handler.can_rollback(&op));

        // With pre-state
        let op_with_state = op.with_pre_state(PreOperationState::FileCreated {
            path: "/tmp/test.txt".to_string(),
        });
        assert!(handler.can_rollback(&op_with_state));
    }

    #[test]
    fn test_bash_rollback_handler() {
        let handler = BashRollbackHandler;

        // mkdir command with pre-state - should be rollbackable
        let mkdir_op = RollbackableOperation::new(
            "Bash",
            serde_json::json!({"command": "mkdir /tmp/testdir"}),
            serde_json::json!({}),
            "session-1",
        )
        .with_pre_state(PreOperationState::DirectoryState {
            path: "/tmp/testdir".to_string(),
            entries: vec![],
        });
        assert!(handler.can_rollback(&mkdir_op));

        // Arbitrary command - not rollbackable
        let arbitrary_op = RollbackableOperation::new(
            "Bash",
            serde_json::json!({"command": "echo hello"}),
            serde_json::json!({}),
            "session-1",
        );
        assert!(!handler.can_rollback(&arbitrary_op));
    }

    #[tokio::test]
    async fn test_checkpoint_rollback_manager() {
        let checkpoint_manager = Arc::new(MemoryCheckpointManager::new());
        let rollback_manager = CheckpointRollbackManager::new(checkpoint_manager);

        // Register handlers
        rollback_manager
            .register_handler(Arc::new(FileWriteRollbackHandler))
            .await;
        rollback_manager
            .register_handler(Arc::new(FileEditRollbackHandler))
            .await;

        // Record an operation
        let op = RollbackableOperation::new(
            "Write",
            serde_json::json!({"path": "/tmp/test.txt"}),
            serde_json::json!({"success": true}),
            "session-1",
        )
        .with_pre_state(PreOperationState::FileCreated {
            path: "/tmp/test.txt".to_string(),
        });

        rollback_manager.record_operation(op.clone()).await;

        // Check if it can be rolled back
        assert!(rollback_manager.can_rollback(&op).await);

        // Check complexity
        let complexity = rollback_manager.get_rollback_complexity(&op).await;
        assert_eq!(complexity, RollbackComplexity::Simple);
    }

    #[test]
    fn test_pre_operation_state_serialization() {
        let state = PreOperationState::FileContent {
            path: "/tmp/test.txt".to_string(),
            content: "original content".to_string(),
            permissions: Some(0o644),
        };

        let json = serde_json::to_string(&state).unwrap();
        let deserialized: PreOperationState = serde_json::from_str(&json).unwrap();

        match deserialized {
            PreOperationState::FileContent {
                path,
                content,
                permissions,
            } => {
                assert_eq!(path, "/tmp/test.txt");
                assert_eq!(content, "original content");
                assert_eq!(permissions, Some(0o644));
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_rollback_error_display() {
        let err = RollbackError::NotRollbackable {
            operation_id: "op_123".to_string(),
            reason: "No pre-state available".to_string(),
        };

        let msg = err.to_string();
        assert!(msg.contains("op_123"));
        assert!(msg.contains("No pre-state available"));
    }
}
