//! Persistent memory backend trait.
//!
//! Defines the [`MemoryBackend`] interface for persistent storage of memory
//! entries. Implementations live in platform modules (e.g., Session Module's
//! `PgMemoryBackend`). The trait is always available (no feature gate) so
//! that `UnifiedMemoryStore` can optionally delegate to a backend.

use async_trait::async_trait;
use uuid::Uuid;

use super::unified::MemoryEntry;
use crate::error::Result;

/// Backend trait for persistent memory storage.
///
/// Implementations live in platform modules (e.g., Session Module with
/// pgvector). All methods are async and the trait is `Send + Sync` so
/// it can be shared via `Arc<dyn MemoryBackend>`.
///
/// # Example
///
/// ```rust,ignore
/// use gateway_core::memory::persistence::MemoryBackend;
///
/// let backend: Arc<dyn MemoryBackend> = create_pg_backend(pool);
/// backend.store(user_id, &entry).await?;
/// let results = backend.semantic_search(user_id, &embedding, 5, 0.5).await?;
/// ```
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Store or upsert a memory entry for the given user.
    async fn store(&self, user_id: Uuid, entry: &MemoryEntry) -> Result<()>;

    /// Retrieve a single entry by key.
    async fn get(&self, user_id: Uuid, key: &str) -> Result<Option<MemoryEntry>>;

    /// Delete an entry by key. Returns `true` if an entry was deleted.
    async fn delete(&self, user_id: Uuid, key: &str) -> Result<bool>;

    /// List entries by category string, up to `limit`.
    async fn list_by_category(
        &self,
        user_id: Uuid,
        category: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>>;

    /// Perform semantic (vector) search using a pre-computed query embedding.
    ///
    /// Returns `(similarity_score, entry)` pairs sorted by descending similarity,
    /// filtered by `min_similarity` threshold.
    async fn semantic_search(
        &self,
        user_id: Uuid,
        query_embedding: &[f32],
        limit: usize,
        min_similarity: f32,
    ) -> Result<Vec<(f32, MemoryEntry)>>;

    /// Get entries that have no embedding yet, up to `limit`.
    ///
    /// Used by the background embedding worker to find entries that need
    /// vector generation.
    async fn get_unembedded(&self, limit: usize) -> Result<Vec<(Uuid, String, MemoryEntry)>>;

    /// Update the embedding vector for a specific entry.
    async fn update_embedding(&self, user_id: Uuid, key: &str, embedding: Vec<f32>) -> Result<()>;

    /// Load the most recent entries for a user, up to `limit`.
    async fn load_recent(&self, user_id: Uuid, limit: usize) -> Result<Vec<MemoryEntry>>;
}

// ==========================================================================
// MockMemoryBackend (for tests)
// ==========================================================================

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    /// In-memory mock backend for unit testing.
    pub struct MockMemoryBackend {
        entries: Arc<RwLock<HashMap<(Uuid, String), MemoryEntry>>>,
    }

    impl MockMemoryBackend {
        pub fn new() -> Self {
            Self {
                entries: Arc::new(RwLock::new(HashMap::new())),
            }
        }
    }

    #[async_trait]
    impl MemoryBackend for MockMemoryBackend {
        async fn store(&self, user_id: Uuid, entry: &MemoryEntry) -> Result<()> {
            let mut entries = self.entries.write().await;
            entries.insert((user_id, entry.key.clone()), entry.clone());
            Ok(())
        }

        async fn get(&self, user_id: Uuid, key: &str) -> Result<Option<MemoryEntry>> {
            let entries = self.entries.read().await;
            Ok(entries.get(&(user_id, key.to_string())).cloned())
        }

        async fn delete(&self, user_id: Uuid, key: &str) -> Result<bool> {
            let mut entries = self.entries.write().await;
            Ok(entries.remove(&(user_id, key.to_string())).is_some())
        }

        async fn list_by_category(
            &self,
            user_id: Uuid,
            category: &str,
            limit: usize,
        ) -> Result<Vec<MemoryEntry>> {
            let entries = self.entries.read().await;
            let results: Vec<MemoryEntry> = entries
                .iter()
                .filter(|((uid, _), entry)| {
                    *uid == user_id && format!("{:?}", entry.category).to_lowercase() == category
                })
                .map(|(_, entry)| entry.clone())
                .take(limit)
                .collect();
            Ok(results)
        }

        async fn semantic_search(
            &self,
            _user_id: Uuid,
            _query_embedding: &[f32],
            _limit: usize,
            _min_similarity: f32,
        ) -> Result<Vec<(f32, MemoryEntry)>> {
            // Mock: no embedding support — return empty
            Ok(vec![])
        }

        async fn get_unembedded(&self, _limit: usize) -> Result<Vec<(Uuid, String, MemoryEntry)>> {
            // Mock: all entries are "unembedded"
            let entries = self.entries.read().await;
            let results: Vec<_> = entries
                .iter()
                .map(|((uid, key), entry)| (*uid, key.clone(), entry.clone()))
                .collect();
            Ok(results)
        }

        async fn update_embedding(
            &self,
            _user_id: Uuid,
            _key: &str,
            _embedding: Vec<f32>,
        ) -> Result<()> {
            // Mock: no-op
            Ok(())
        }

        async fn load_recent(&self, user_id: Uuid, limit: usize) -> Result<Vec<MemoryEntry>> {
            let entries = self.entries.read().await;
            let mut results: Vec<MemoryEntry> = entries
                .iter()
                .filter(|((uid, _), _)| *uid == user_id)
                .map(|(_, entry)| entry.clone())
                .collect();
            results.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            results.truncate(limit);
            Ok(results)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockMemoryBackend;
    use super::*;
    use crate::unified::{MemoryCategory, MemoryEntry};

    #[tokio::test]
    async fn test_mock_backend_store_and_get() {
        let backend = MockMemoryBackend::new();
        let user = Uuid::new_v4();
        let entry = MemoryEntry::new("test_key", MemoryCategory::Knowledge, "test content");
        backend.store(user, &entry).await.unwrap();
        let got = backend.get(user, "test_key").await.unwrap().unwrap();
        assert_eq!(got.content, "test content");
    }

    #[tokio::test]
    async fn test_mock_backend_delete() {
        let backend = MockMemoryBackend::new();
        let user = Uuid::new_v4();
        backend
            .store(user, &MemoryEntry::new("k", MemoryCategory::Knowledge, "v"))
            .await
            .unwrap();
        assert!(backend.delete(user, "k").await.unwrap());
        assert!(!backend.delete(user, "nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn test_mock_backend_list_by_category() {
        let backend = MockMemoryBackend::new();
        let user = Uuid::new_v4();
        backend
            .store(
                user,
                &MemoryEntry::new("k1", MemoryCategory::Knowledge, "v1"),
            )
            .await
            .unwrap();
        backend
            .store(
                user,
                &MemoryEntry::new("k2", MemoryCategory::Preference, "v2"),
            )
            .await
            .unwrap();
        let results = backend
            .list_by_category(user, "knowledge", 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_mock_backend_get_unembedded() {
        let backend = MockMemoryBackend::new();
        let user = Uuid::new_v4();
        backend
            .store(
                user,
                &MemoryEntry::new("k1", MemoryCategory::Knowledge, "v1"),
            )
            .await
            .unwrap();
        let unembedded = backend.get_unembedded(10).await.unwrap();
        assert_eq!(unembedded.len(), 1);
    }

    #[tokio::test]
    async fn test_mock_backend_load_recent() {
        let backend = MockMemoryBackend::new();
        let user = Uuid::new_v4();
        backend
            .store(
                user,
                &MemoryEntry::new("k1", MemoryCategory::Knowledge, "v1"),
            )
            .await
            .unwrap();
        backend
            .store(
                user,
                &MemoryEntry::new("k2", MemoryCategory::Knowledge, "v2"),
            )
            .await
            .unwrap();
        let recent = backend.load_recent(user, 1).await.unwrap();
        assert_eq!(recent.len(), 1);
    }
}
