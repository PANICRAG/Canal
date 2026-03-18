//! Memory Store Trait
//!
//! Defines the interface for memory storage backends.

use async_trait::async_trait;
use thiserror::Error;

use crate::agent::types::{MemoryEntry, UserMemory};

/// Errors that can occur during memory operations
#[derive(Debug, Error)]
pub enum MemoryError {
    /// User memory not found
    #[error("Memory not found for user: {0}")]
    NotFound(String),

    /// Storage error
    #[error("Storage error: {0}")]
    StorageError(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Version conflict for optimistic locking
    #[error("Version conflict: expected {expected}, found {found}")]
    VersionConflict { expected: u64, found: u64 },

    /// Invalid memory key
    #[error("Invalid memory key: {0}")]
    InvalidKey(String),
}

/// Memory storage trait
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Load user memory
    ///
    /// Returns `Ok(UserMemory)` if found, or creates a new empty one if not found.
    async fn load(&self, user_id: &str) -> Result<UserMemory, MemoryError>;

    /// Save user memory
    ///
    /// Persists the entire memory collection.
    async fn save(&self, memory: &UserMemory) -> Result<(), MemoryError>;

    /// Get a specific memory entry
    async fn get(&self, user_id: &str, key: &str) -> Result<Option<MemoryEntry>, MemoryError>;

    /// Set a specific memory entry
    ///
    /// This is an atomic operation that updates only the specified entry.
    async fn set(&self, user_id: &str, entry: MemoryEntry) -> Result<(), MemoryError>;

    /// Delete a specific memory entry
    async fn delete(&self, user_id: &str, key: &str) -> Result<bool, MemoryError>;

    /// Delete all memories for a user
    async fn delete_all(&self, user_id: &str) -> Result<(), MemoryError>;

    /// Check if a user has any memories
    async fn exists(&self, user_id: &str) -> bool;

    /// List all user IDs with memories
    async fn list_users(&self) -> Result<Vec<String>, MemoryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_error_display() {
        let err = MemoryError::NotFound("user-1".to_string());
        assert!(err.to_string().contains("user-1"));

        let err = MemoryError::VersionConflict {
            expected: 1,
            found: 2,
        };
        assert!(err.to_string().contains("expected 1"));
        assert!(err.to_string().contains("found 2"));
    }
}
