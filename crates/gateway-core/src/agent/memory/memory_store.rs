//! In-Memory Store
//!
//! Thread-safe in-memory storage for testing and development.

use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;

use super::store::{MemoryError, MemoryStore};
use crate::agent::types::{MemoryEntry, UserMemory};

/// In-memory memory store (for testing)
pub struct InMemoryStore {
    memories: RwLock<HashMap<String, UserMemory>>,
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryStore {
    /// Create a new in-memory store
    pub fn new() -> Self {
        Self {
            memories: RwLock::new(HashMap::new()),
        }
    }

    /// Create with initial data
    pub fn with_data(data: HashMap<String, UserMemory>) -> Self {
        Self {
            memories: RwLock::new(data),
        }
    }

    /// Get all memories (for testing)
    pub async fn all(&self) -> HashMap<String, UserMemory> {
        self.memories.read().await.clone()
    }

    /// Clear all memories (for testing)
    pub async fn clear(&self) {
        self.memories.write().await.clear();
    }
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn load(&self, user_id: &str) -> Result<UserMemory, MemoryError> {
        let memories = self.memories.read().await;
        Ok(memories
            .get(user_id)
            .cloned()
            .unwrap_or_else(|| UserMemory::new(user_id)))
    }

    async fn save(&self, memory: &UserMemory) -> Result<(), MemoryError> {
        let mut memories = self.memories.write().await;
        memories.insert(memory.user_id.clone(), memory.clone());
        Ok(())
    }

    async fn get(&self, user_id: &str, key: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        let memories = self.memories.read().await;
        Ok(memories.get(user_id).and_then(|m| m.get(key)).cloned())
    }

    async fn set(&self, user_id: &str, entry: MemoryEntry) -> Result<(), MemoryError> {
        let mut memories = self.memories.write().await;
        let memory = memories
            .entry(user_id.to_string())
            .or_insert_with(|| UserMemory::new(user_id));
        memory.set(entry);
        Ok(())
    }

    async fn delete(&self, user_id: &str, key: &str) -> Result<bool, MemoryError> {
        let mut memories = self.memories.write().await;
        if let Some(memory) = memories.get_mut(user_id) {
            Ok(memory.remove(key).is_some())
        } else {
            Ok(false)
        }
    }

    async fn delete_all(&self, user_id: &str) -> Result<(), MemoryError> {
        let mut memories = self.memories.write().await;
        memories.remove(user_id);
        Ok(())
    }

    async fn exists(&self, user_id: &str) -> bool {
        self.memories.read().await.contains_key(user_id)
    }

    async fn list_users(&self) -> Result<Vec<String>, MemoryError> {
        let memories = self.memories.read().await;
        Ok(memories.keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::{MemoryCategory, MemorySource};

    #[tokio::test]
    async fn test_in_memory_store_basic() {
        let store = InMemoryStore::new();

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
    async fn test_in_memory_store_atomic_operations() {
        let store = InMemoryStore::new();

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
    async fn test_in_memory_store_exists() {
        let store = InMemoryStore::new();

        assert!(!store.exists("user-1").await);

        store
            .set("user-1", MemoryEntry::text("key", "value"))
            .await
            .unwrap();

        assert!(store.exists("user-1").await);
    }

    #[tokio::test]
    async fn test_in_memory_store_delete_all() {
        let store = InMemoryStore::new();

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
    async fn test_in_memory_store_list_users() {
        let store = InMemoryStore::new();

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
}
