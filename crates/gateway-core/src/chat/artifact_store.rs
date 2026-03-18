//! Artifact Storage - Persistent storage backends for artifacts
//!
//! Provides storage abstractions for artifacts including:
//! - `ArtifactStore` trait defining the storage interface
//! - `FileArtifactStore` for file-based persistence
//! - `MemoryArtifactStore` for in-memory storage (testing)

use super::artifact::StoredArtifact;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;
use tokio::fs;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Artifact storage error
#[derive(Error, Debug)]
pub enum ArtifactStoreError {
    #[error("Artifact not found: {0}")]
    NotFound(Uuid),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Invalid artifact: {0}")]
    InvalidArtifact(String),
}

/// Result type for artifact operations
pub type ArtifactResult<T> = Result<T, ArtifactStoreError>;

/// Artifact query parameters
#[derive(Debug, Clone, Default)]
pub struct ArtifactQuery {
    /// Filter by session ID
    pub session_id: Option<Uuid>,
    /// Filter by message ID
    pub message_id: Option<Uuid>,
    /// Filter by artifact type
    pub artifact_type: Option<super::artifact::ArtifactType>,
    /// Maximum number of results
    pub limit: Option<u32>,
    /// Offset for pagination
    pub offset: Option<u32>,
}

impl ArtifactQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn session(mut self, session_id: Uuid) -> Self {
        self.session_id = Some(session_id);
        self
    }

    pub fn message(mut self, message_id: Uuid) -> Self {
        self.message_id = Some(message_id);
        self
    }

    pub fn artifact_type(mut self, artifact_type: super::artifact::ArtifactType) -> Self {
        self.artifact_type = Some(artifact_type);
        self
    }

    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }
}

/// Artifact storage trait
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    /// Save an artifact
    async fn save(&self, artifact: &StoredArtifact) -> ArtifactResult<()>;

    /// Load an artifact by ID
    async fn load(&self, artifact_id: Uuid) -> ArtifactResult<StoredArtifact>;

    /// Delete an artifact
    async fn delete(&self, artifact_id: Uuid) -> ArtifactResult<()>;

    /// Query artifacts
    async fn query(&self, query: ArtifactQuery) -> ArtifactResult<Vec<StoredArtifact>>;

    /// List all artifacts (convenience method)
    async fn list(&self, limit: Option<u32>) -> ArtifactResult<Vec<StoredArtifact>> {
        self.query(ArtifactQuery::new().limit(limit.unwrap_or(100)))
            .await
    }

    /// Check if an artifact exists
    async fn exists(&self, artifact_id: Uuid) -> bool;

    /// Get artifacts by session ID
    async fn get_by_session(&self, session_id: Uuid) -> ArtifactResult<Vec<StoredArtifact>> {
        self.query(ArtifactQuery::new().session(session_id)).await
    }

    /// Get artifacts by message ID
    async fn get_by_message(&self, message_id: Uuid) -> ArtifactResult<Vec<StoredArtifact>> {
        self.query(ArtifactQuery::new().message(message_id)).await
    }

    /// Delete all artifacts for a session
    async fn delete_by_session(&self, session_id: Uuid) -> ArtifactResult<u32>;
}

/// File-based artifact storage
pub struct FileArtifactStore {
    /// Base directory for artifact files
    base_path: PathBuf,
}

impl FileArtifactStore {
    /// Create a new file storage
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    /// Get the path for an artifact
    fn artifact_path(&self, artifact_id: Uuid) -> PathBuf {
        self.base_path.join(format!("{}.json", artifact_id))
    }

    /// Get the index path for a session
    fn session_index_path(&self, session_id: Uuid) -> PathBuf {
        self.base_path
            .join(format!("session_{}.index.json", session_id))
    }

    /// Ensure base directory exists
    async fn ensure_dir(&self) -> ArtifactResult<()> {
        fs::create_dir_all(&self.base_path)
            .await
            .map_err(|e| ArtifactStoreError::StorageError(e.to_string()))
    }

    /// Update session index
    async fn update_session_index(
        &self,
        session_id: Uuid,
        artifact_id: Uuid,
        remove: bool,
    ) -> ArtifactResult<()> {
        let index_path = self.session_index_path(session_id);
        // R3-H8: Use tokio::fs::try_exists instead of blocking std::path::Path::exists
        let mut index: Vec<Uuid> = if fs::try_exists(&index_path).await.unwrap_or(false) {
            let json = fs::read_to_string(&index_path)
                .await
                .map_err(|e| ArtifactStoreError::StorageError(e.to_string()))?;
            serde_json::from_str(&json)
                .map_err(|e| ArtifactStoreError::SerializationError(e.to_string()))?
        } else {
            Vec::new()
        };

        if remove {
            index.retain(|id| *id != artifact_id);
        } else if !index.contains(&artifact_id) {
            index.push(artifact_id);
        }

        let json = serde_json::to_string_pretty(&index)
            .map_err(|e| ArtifactStoreError::SerializationError(e.to_string()))?;

        fs::write(&index_path, json)
            .await
            .map_err(|e| ArtifactStoreError::StorageError(e.to_string()))?;

        Ok(())
    }
}

#[async_trait]
impl ArtifactStore for FileArtifactStore {
    async fn save(&self, artifact: &StoredArtifact) -> ArtifactResult<()> {
        self.ensure_dir().await?;

        let path = self.artifact_path(artifact.id);
        let json = serde_json::to_string_pretty(artifact)
            .map_err(|e| ArtifactStoreError::SerializationError(e.to_string()))?;

        fs::write(&path, json)
            .await
            .map_err(|e| ArtifactStoreError::StorageError(e.to_string()))?;

        // Update session index if session_id is set
        if let Some(session_id) = artifact.session_id {
            self.update_session_index(session_id, artifact.id, false)
                .await?;
        }

        Ok(())
    }

    async fn load(&self, artifact_id: Uuid) -> ArtifactResult<StoredArtifact> {
        let path = self.artifact_path(artifact_id);

        // R3-H8: Use async exists check instead of blocking
        if !fs::try_exists(&path).await.unwrap_or(false) {
            return Err(ArtifactStoreError::NotFound(artifact_id));
        }

        let json = fs::read_to_string(&path)
            .await
            .map_err(|e| ArtifactStoreError::StorageError(e.to_string()))?;

        serde_json::from_str(&json)
            .map_err(|e| ArtifactStoreError::SerializationError(e.to_string()))
    }

    async fn delete(&self, artifact_id: Uuid) -> ArtifactResult<()> {
        let path = self.artifact_path(artifact_id);

        // Load artifact to get session_id for index update
        if fs::try_exists(&path).await.unwrap_or(false) {
            if let Ok(artifact) = self.load(artifact_id).await {
                if let Some(session_id) = artifact.session_id {
                    let _ = self
                        .update_session_index(session_id, artifact_id, true)
                        .await;
                }
            }

            fs::remove_file(&path)
                .await
                .map_err(|e| ArtifactStoreError::StorageError(e.to_string()))?;
        }

        Ok(())
    }

    async fn query(&self, query: ArtifactQuery) -> ArtifactResult<Vec<StoredArtifact>> {
        self.ensure_dir().await?;

        let mut artifacts = Vec::new();

        // If session_id is specified, use the session index
        if let Some(session_id) = query.session_id {
            let index_path = self.session_index_path(session_id);
            if fs::try_exists(&index_path).await.unwrap_or(false) {
                let json = fs::read_to_string(&index_path)
                    .await
                    .map_err(|e| ArtifactStoreError::StorageError(e.to_string()))?;
                let artifact_ids: Vec<Uuid> = serde_json::from_str(&json)
                    .map_err(|e| ArtifactStoreError::SerializationError(e.to_string()))?;

                for id in artifact_ids {
                    if let Ok(artifact) = self.load(id).await {
                        artifacts.push(artifact);
                    }
                }
            }
        } else {
            // Scan all files
            let mut entries = fs::read_dir(&self.base_path)
                .await
                .map_err(|e| ArtifactStoreError::StorageError(e.to_string()))?;

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().map(|e| e == "json").unwrap_or(false)
                    && !path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.contains(".index."))
                        .unwrap_or(false)
                {
                    if let Ok(json) = fs::read_to_string(&path).await {
                        if let Ok(artifact) = serde_json::from_str::<StoredArtifact>(&json) {
                            artifacts.push(artifact);
                        }
                    }
                }
            }
        }

        // Apply filters
        if let Some(message_id) = query.message_id {
            artifacts.retain(|a| a.message_id == Some(message_id));
        }

        if let Some(artifact_type) = query.artifact_type {
            artifacts.retain(|a| a.artifact_type == artifact_type);
        }

        // Sort by created_at descending
        artifacts.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        // Apply pagination
        let offset = query.offset.unwrap_or(0) as usize;
        let limit = query.limit.unwrap_or(100) as usize;

        Ok(artifacts.into_iter().skip(offset).take(limit).collect())
    }

    async fn exists(&self, artifact_id: Uuid) -> bool {
        self.artifact_path(artifact_id).exists()
    }

    async fn delete_by_session(&self, session_id: Uuid) -> ArtifactResult<u32> {
        let artifacts = self.get_by_session(session_id).await?;
        let count = artifacts.len() as u32;

        for artifact in artifacts {
            self.delete(artifact.id).await?;
        }

        // Remove the session index file
        let index_path = self.session_index_path(session_id);
        if fs::try_exists(&index_path).await.unwrap_or(false) {
            let _ = fs::remove_file(&index_path).await;
        }

        Ok(count)
    }
}

/// In-memory artifact storage (for testing)
pub struct MemoryArtifactStore {
    artifacts: RwLock<HashMap<Uuid, StoredArtifact>>,
}

impl Default for MemoryArtifactStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryArtifactStore {
    /// Create a new memory storage
    pub fn new() -> Self {
        Self {
            artifacts: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl ArtifactStore for MemoryArtifactStore {
    async fn save(&self, artifact: &StoredArtifact) -> ArtifactResult<()> {
        self.artifacts
            .write()
            .await
            .insert(artifact.id, artifact.clone());
        Ok(())
    }

    async fn load(&self, artifact_id: Uuid) -> ArtifactResult<StoredArtifact> {
        self.artifacts
            .read()
            .await
            .get(&artifact_id)
            .cloned()
            .ok_or_else(|| ArtifactStoreError::NotFound(artifact_id))
    }

    async fn delete(&self, artifact_id: Uuid) -> ArtifactResult<()> {
        self.artifacts.write().await.remove(&artifact_id);
        Ok(())
    }

    async fn query(&self, query: ArtifactQuery) -> ArtifactResult<Vec<StoredArtifact>> {
        let artifacts = self.artifacts.read().await;
        let mut result: Vec<_> = artifacts
            .values()
            .filter(|a| {
                if let Some(session_id) = query.session_id {
                    if a.session_id != Some(session_id) {
                        return false;
                    }
                }
                if let Some(message_id) = query.message_id {
                    if a.message_id != Some(message_id) {
                        return false;
                    }
                }
                if let Some(ref artifact_type) = query.artifact_type {
                    if &a.artifact_type != artifact_type {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        // Sort by created_at descending
        result.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        // Apply pagination
        let offset = query.offset.unwrap_or(0) as usize;
        let limit = query.limit.unwrap_or(100) as usize;

        Ok(result.into_iter().skip(offset).take(limit).collect())
    }

    async fn exists(&self, artifact_id: Uuid) -> bool {
        self.artifacts.read().await.contains_key(&artifact_id)
    }

    async fn delete_by_session(&self, session_id: Uuid) -> ArtifactResult<u32> {
        let mut artifacts = self.artifacts.write().await;
        let ids_to_remove: Vec<Uuid> = artifacts
            .values()
            .filter(|a| a.session_id == Some(session_id))
            .map(|a| a.id)
            .collect();

        let count = ids_to_remove.len() as u32;
        for id in ids_to_remove {
            artifacts.remove(&id);
        }

        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::artifact::ArtifactType;

    #[tokio::test]
    async fn test_memory_storage_save_load() {
        let store = MemoryArtifactStore::new();

        let artifact = StoredArtifact::document("Test Doc", "Content here");
        store.save(&artifact).await.unwrap();

        assert!(store.exists(artifact.id).await);

        let loaded = store.load(artifact.id).await.unwrap();
        assert_eq!(loaded.title, "Test Doc");
        assert_eq!(loaded.artifact_type, ArtifactType::Document);
    }

    #[tokio::test]
    async fn test_memory_storage_delete() {
        let store = MemoryArtifactStore::new();

        let artifact = StoredArtifact::document("Test Doc", "Content");
        store.save(&artifact).await.unwrap();
        assert!(store.exists(artifact.id).await);

        store.delete(artifact.id).await.unwrap();
        assert!(!store.exists(artifact.id).await);
    }

    #[tokio::test]
    async fn test_memory_storage_query_by_session() {
        let store = MemoryArtifactStore::new();
        let session_id = Uuid::new_v4();

        let artifact1 = StoredArtifact::document("Doc 1", "Content 1").with_session_id(session_id);
        let artifact2 = StoredArtifact::document("Doc 2", "Content 2").with_session_id(session_id);
        let artifact3 = StoredArtifact::document("Doc 3", "Content 3"); // No session

        store.save(&artifact1).await.unwrap();
        store.save(&artifact2).await.unwrap();
        store.save(&artifact3).await.unwrap();

        let results = store.get_by_session(session_id).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_memory_storage_query_by_type() {
        let store = MemoryArtifactStore::new();

        let doc = StoredArtifact::document("Doc", "Content");
        let code = StoredArtifact::code_block("Code", "rust", "fn main() {}");

        store.save(&doc).await.unwrap();
        store.save(&code).await.unwrap();

        let results = store
            .query(ArtifactQuery::new().artifact_type(ArtifactType::CodeBlock))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].artifact_type, ArtifactType::CodeBlock);
    }

    #[tokio::test]
    async fn test_memory_storage_pagination() {
        let store = MemoryArtifactStore::new();

        for i in 0..10 {
            let artifact = StoredArtifact::document(format!("Doc {}", i), "Content");
            store.save(&artifact).await.unwrap();
        }

        let results = store
            .query(ArtifactQuery::new().limit(5).offset(3))
            .await
            .unwrap();
        assert_eq!(results.len(), 5);
    }

    #[tokio::test]
    async fn test_memory_storage_delete_by_session() {
        let store = MemoryArtifactStore::new();
        let session_id = Uuid::new_v4();

        for i in 0..5 {
            let artifact = StoredArtifact::document(format!("Doc {}", i), "Content")
                .with_session_id(session_id);
            store.save(&artifact).await.unwrap();
        }

        let deleted = store.delete_by_session(session_id).await.unwrap();
        assert_eq!(deleted, 5);

        let results = store.get_by_session(session_id).await.unwrap();
        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_file_storage() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = FileArtifactStore::new(temp_dir.path());

        let artifact = StoredArtifact::document("Test Doc", "Content here");
        store.save(&artifact).await.unwrap();

        assert!(store.exists(artifact.id).await);

        let loaded = store.load(artifact.id).await.unwrap();
        assert_eq!(loaded.title, "Test Doc");

        store.delete(artifact.id).await.unwrap();
        assert!(!store.exists(artifact.id).await);
    }

    #[tokio::test]
    async fn test_file_storage_session_index() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = FileArtifactStore::new(temp_dir.path());
        let session_id = Uuid::new_v4();

        let artifact1 = StoredArtifact::document("Doc 1", "Content 1").with_session_id(session_id);
        let artifact2 = StoredArtifact::document("Doc 2", "Content 2").with_session_id(session_id);

        store.save(&artifact1).await.unwrap();
        store.save(&artifact2).await.unwrap();

        let results = store.get_by_session(session_id).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_file_storage_delete_by_session() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let store = FileArtifactStore::new(temp_dir.path());
        let session_id = Uuid::new_v4();

        for i in 0..3 {
            let artifact = StoredArtifact::document(format!("Doc {}", i), "Content")
                .with_session_id(session_id);
            store.save(&artifact).await.unwrap();
        }

        let deleted = store.delete_by_session(session_id).await.unwrap();
        assert_eq!(deleted, 3);

        let results = store.get_by_session(session_id).await.unwrap();
        assert_eq!(results.len(), 0);
    }
}
