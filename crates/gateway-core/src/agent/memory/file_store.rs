//! File-based Memory Store
//!
//! Persistent storage using JSON files.

use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;

use super::store::{MemoryError, MemoryStore};
use crate::agent::types::{MemoryEntry, UserMemory};

/// File-based memory storage
pub struct FileMemoryStore {
    /// Base directory for memory files
    base_path: PathBuf,
}

impl FileMemoryStore {
    /// Create a new file-based memory store
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    /// Get the file path for a user's memory
    fn user_path(&self, user_id: &str) -> PathBuf {
        // Sanitize user_id to prevent path traversal
        let safe_id = user_id.replace(['/', '\\', '.'], "_");
        self.base_path.join(format!("{}.json", safe_id))
    }

    /// Ensure the base directory exists
    async fn ensure_dir(&self) -> Result<(), MemoryError> {
        fs::create_dir_all(&self.base_path)
            .await
            .map_err(|e| MemoryError::StorageError(e.to_string()))
    }
}

#[async_trait]
impl MemoryStore for FileMemoryStore {
    async fn load(&self, user_id: &str) -> Result<UserMemory, MemoryError> {
        let path = self.user_path(user_id);

        if !path.exists() {
            return Ok(UserMemory::new(user_id));
        }

        let json = fs::read_to_string(&path)
            .await
            .map_err(|e| MemoryError::StorageError(e.to_string()))?;

        serde_json::from_str(&json).map_err(|e| MemoryError::SerializationError(e.to_string()))
    }

    async fn save(&self, memory: &UserMemory) -> Result<(), MemoryError> {
        self.ensure_dir().await?;

        let path = self.user_path(&memory.user_id);
        let json = serde_json::to_string_pretty(memory)
            .map_err(|e| MemoryError::SerializationError(e.to_string()))?;

        // R1-M: Atomic write via temp file + rename to prevent data loss on concurrent access
        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, json)
            .await
            .map_err(|e| MemoryError::StorageError(e.to_string()))?;
        fs::rename(&temp_path, &path)
            .await
            .map_err(|e| MemoryError::StorageError(e.to_string()))?;

        Ok(())
    }

    async fn get(&self, user_id: &str, key: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        let memory = self.load(user_id).await?;
        Ok(memory.get(key).cloned())
    }

    async fn set(&self, user_id: &str, entry: MemoryEntry) -> Result<(), MemoryError> {
        let mut memory = self.load(user_id).await?;
        memory.set(entry);
        self.save(&memory).await
    }

    async fn delete(&self, user_id: &str, key: &str) -> Result<bool, MemoryError> {
        let mut memory = self.load(user_id).await?;
        let removed = memory.remove(key).is_some();
        if removed {
            self.save(&memory).await?;
        }
        Ok(removed)
    }

    async fn delete_all(&self, user_id: &str) -> Result<(), MemoryError> {
        let path = self.user_path(user_id);

        if path.exists() {
            fs::remove_file(&path)
                .await
                .map_err(|e| MemoryError::StorageError(e.to_string()))?;
        }

        Ok(())
    }

    async fn exists(&self, user_id: &str) -> bool {
        self.user_path(user_id).exists()
    }

    async fn list_users(&self) -> Result<Vec<String>, MemoryError> {
        self.ensure_dir().await?;

        let mut users = Vec::new();
        let mut entries = fs::read_dir(&self.base_path)
            .await
            .map_err(|e| MemoryError::StorageError(e.to_string()))?;

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    users.push(stem.to_string());
                }
            }
        }

        Ok(users)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::{MemoryCategory, MemorySource};
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_file_store_basic() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileMemoryStore::new(temp_dir.path());

        // Load non-existent user creates empty memory
        let memory = store.load("user-1").await.unwrap();
        assert_eq!(memory.user_id, "user-1");
        assert!(memory.is_empty());

        // Save and load
        let mut memory = UserMemory::new("user-1");
        memory.set_text("name", "Alice");
        store.save(&memory).await.unwrap();

        let loaded = store.load("user-1").await.unwrap();
        assert_eq!(loaded.get_str("name"), Some("Alice"));
    }

    #[tokio::test]
    async fn test_file_store_atomic_operations() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileMemoryStore::new(temp_dir.path());

        // Set entry directly
        let entry = MemoryEntry::text("language", "Rust")
            .with_source(MemorySource::UserStated)
            .with_category(MemoryCategory::Technical);

        store.set("user-1", entry).await.unwrap();

        // Get entry
        let retrieved = store.get("user-1", "language").await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().as_str(), Some("Rust"));

        // Delete entry
        let deleted = store.delete("user-1", "language").await.unwrap();
        assert!(deleted);

        let retrieved = store.get("user-1", "language").await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_file_store_exists() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileMemoryStore::new(temp_dir.path());

        assert!(!store.exists("user-1").await);

        store
            .set("user-1", MemoryEntry::text("key", "value"))
            .await
            .unwrap();

        assert!(store.exists("user-1").await);
    }

    #[tokio::test]
    async fn test_file_store_delete_all() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileMemoryStore::new(temp_dir.path());

        store
            .set("user-1", MemoryEntry::text("key1", "value1"))
            .await
            .unwrap();
        store
            .set("user-1", MemoryEntry::text("key2", "value2"))
            .await
            .unwrap();

        store.delete_all("user-1").await.unwrap();

        assert!(!store.exists("user-1").await);
    }

    #[tokio::test]
    async fn test_file_store_list_users() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileMemoryStore::new(temp_dir.path());

        store
            .set("user-1", MemoryEntry::text("key", "value"))
            .await
            .unwrap();
        store
            .set("user-2", MemoryEntry::text("key", "value"))
            .await
            .unwrap();

        let users = store.list_users().await.unwrap();
        assert_eq!(users.len(), 2);
        assert!(users.contains(&"user-1".to_string()));
        assert!(users.contains(&"user-2".to_string()));
    }

    #[tokio::test]
    async fn test_file_store_persistence() {
        let temp_dir = TempDir::new().unwrap();

        // Create store and save data
        {
            let store = FileMemoryStore::new(temp_dir.path());
            let mut memory = UserMemory::new("user-1");
            memory.set_text("name", "Alice");
            memory.set_text("language", "Rust");
            store.save(&memory).await.unwrap();
        }

        // Create new store and load data
        {
            let store = FileMemoryStore::new(temp_dir.path());
            let memory = store.load("user-1").await.unwrap();
            assert_eq!(memory.get_str("name"), Some("Alice"));
            assert_eq!(memory.get_str("language"), Some("Rust"));
        }
    }

    #[tokio::test]
    async fn test_file_store_path_sanitization() {
        let temp_dir = TempDir::new().unwrap();
        let store = FileMemoryStore::new(temp_dir.path());

        // Attempt path traversal - should be sanitized
        store
            .set("../../../etc/passwd", MemoryEntry::text("key", "value"))
            .await
            .unwrap();

        // Check that the file is in the correct location
        let path = store.user_path("../../../etc/passwd");
        assert!(path.starts_with(temp_dir.path()));
    }
}
