//! Session Storage - Persistent storage backends for sessions

use super::{SessionError, SessionMetadata, SessionSnapshot};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;

/// Session storage trait
#[async_trait]
pub trait SessionStorage: Send + Sync {
    /// Save a session snapshot
    async fn save(&self, snapshot: &SessionSnapshot) -> Result<(), SessionError>;

    /// Load a session snapshot
    async fn load(&self, session_id: &str) -> Result<SessionSnapshot, SessionError>;

    /// Delete a session
    async fn delete(&self, session_id: &str) -> Result<(), SessionError>;

    /// List sessions
    async fn list(&self, limit: Option<u32>) -> Result<Vec<SessionMetadata>, SessionError>;

    /// Check if a session exists
    async fn exists(&self, session_id: &str) -> bool;
}

/// File-based session storage
pub struct FileSessionStorage {
    /// Base directory for session files
    base_path: PathBuf,
}

impl FileSessionStorage {
    /// Create a new file storage
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    /// Validate session_id contains only safe characters (prevents path traversal).
    fn validate_session_id(session_id: &str) -> Result<(), SessionError> {
        if session_id.is_empty()
            || session_id.contains('/')
            || session_id.contains('\\')
            || session_id.contains("..")
            || session_id.contains('\0')
            || !session_id
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(SessionError::StorageError(format!(
                "Invalid session_id: contains unsafe characters: {}",
                session_id
            )));
        }
        Ok(())
    }

    /// Get the path for a session (validates session_id first).
    fn session_path(&self, session_id: &str) -> Result<PathBuf, SessionError> {
        Self::validate_session_id(session_id)?;
        Ok(self.base_path.join(format!("{}.json", session_id)))
    }

    /// Get the metadata path for a session (validates session_id first).
    fn metadata_path(&self, session_id: &str) -> Result<PathBuf, SessionError> {
        Self::validate_session_id(session_id)?;
        Ok(self.base_path.join(format!("{}.meta.json", session_id)))
    }

    /// Ensure base directory exists
    async fn ensure_dir(&self) -> Result<(), SessionError> {
        fs::create_dir_all(&self.base_path)
            .await
            .map_err(|e| SessionError::StorageError(e.to_string()))
    }
}

#[async_trait]
impl SessionStorage for FileSessionStorage {
    async fn save(&self, snapshot: &SessionSnapshot) -> Result<(), SessionError> {
        self.ensure_dir().await?;

        let path = self.session_path(&snapshot.metadata.id)?;
        let json = serde_json::to_string_pretty(snapshot)
            .map_err(|e| SessionError::SerializationError(e.to_string()))?;

        fs::write(&path, json)
            .await
            .map_err(|e| SessionError::StorageError(e.to_string()))?;

        // Also save metadata separately for fast listing
        let meta_path = self.metadata_path(&snapshot.metadata.id)?;
        let meta_json = serde_json::to_string_pretty(&snapshot.metadata)
            .map_err(|e| SessionError::SerializationError(e.to_string()))?;

        fs::write(&meta_path, meta_json)
            .await
            .map_err(|e| SessionError::StorageError(e.to_string()))?;

        Ok(())
    }

    async fn load(&self, session_id: &str) -> Result<SessionSnapshot, SessionError> {
        let path = self.session_path(session_id)?;

        if !path.exists() {
            return Err(SessionError::NotFound(session_id.to_string()));
        }

        let json = fs::read_to_string(&path)
            .await
            .map_err(|e| SessionError::StorageError(e.to_string()))?;

        serde_json::from_str(&json).map_err(|e| SessionError::SerializationError(e.to_string()))
    }

    async fn delete(&self, session_id: &str) -> Result<(), SessionError> {
        let path = self.session_path(session_id)?;
        let meta_path = self.metadata_path(session_id)?;

        if path.exists() {
            fs::remove_file(&path)
                .await
                .map_err(|e| SessionError::StorageError(e.to_string()))?;
        }

        if meta_path.exists() {
            fs::remove_file(&meta_path)
                .await
                .map_err(|e| SessionError::StorageError(e.to_string()))?;
        }

        Ok(())
    }

    async fn list(&self, limit: Option<u32>) -> Result<Vec<SessionMetadata>, SessionError> {
        self.ensure_dir().await?;

        let mut entries = fs::read_dir(&self.base_path)
            .await
            .map_err(|e| SessionError::StorageError(e.to_string()))?;

        let mut metadata_list = Vec::new();
        let limit = limit.unwrap_or(100) as usize;

        while let Ok(Some(entry)) = entries.next_entry().await {
            if metadata_list.len() >= limit {
                break;
            }

            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".meta.json"))
                    .unwrap_or(false)
            {
                if let Ok(json) = fs::read_to_string(&path).await {
                    if let Ok(metadata) = serde_json::from_str::<SessionMetadata>(&json) {
                        metadata_list.push(metadata);
                    }
                }
            }
        }

        // Sort by updated_at descending
        metadata_list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        Ok(metadata_list)
    }

    async fn exists(&self, session_id: &str) -> bool {
        self.session_path(session_id)
            .map(|p| p.exists())
            .unwrap_or(false)
    }
}

/// In-memory session storage (for testing)
pub struct MemorySessionStorage {
    sessions: tokio::sync::RwLock<std::collections::HashMap<String, SessionSnapshot>>,
}

impl Default for MemorySessionStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl MemorySessionStorage {
    /// Create a new memory storage
    pub fn new() -> Self {
        Self {
            sessions: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

#[async_trait]
impl SessionStorage for MemorySessionStorage {
    async fn save(&self, snapshot: &SessionSnapshot) -> Result<(), SessionError> {
        self.sessions
            .write()
            .await
            .insert(snapshot.metadata.id.clone(), snapshot.clone());
        Ok(())
    }

    async fn load(&self, session_id: &str) -> Result<SessionSnapshot, SessionError> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| SessionError::NotFound(session_id.to_string()))
    }

    async fn delete(&self, session_id: &str) -> Result<(), SessionError> {
        self.sessions.write().await.remove(session_id);
        Ok(())
    }

    async fn list(&self, limit: Option<u32>) -> Result<Vec<SessionMetadata>, SessionError> {
        let sessions = self.sessions.read().await;
        let limit = limit.unwrap_or(100) as usize;

        let mut metadata: Vec<_> = sessions.values().map(|s| s.metadata.clone()).collect();

        metadata.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        metadata.truncate(limit);

        Ok(metadata)
    }

    async fn exists(&self, session_id: &str) -> bool {
        self.sessions.read().await.contains_key(session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_storage() {
        let storage = MemorySessionStorage::new();

        let metadata = SessionMetadata::new("test-session", "/tmp");
        let snapshot = SessionSnapshot::new(metadata, vec![]);

        // Save
        storage.save(&snapshot).await.unwrap();
        assert!(storage.exists("test-session").await);

        // Load
        let loaded = storage.load("test-session").await.unwrap();
        assert_eq!(loaded.metadata.id, "test-session");

        // List
        let list = storage.list(None).await.unwrap();
        assert_eq!(list.len(), 1);

        // Delete
        storage.delete("test-session").await.unwrap();
        assert!(!storage.exists("test-session").await);
    }

    #[tokio::test]
    async fn test_file_storage() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let storage = FileSessionStorage::new(temp_dir.path());

        let metadata = SessionMetadata::new("file-test", "/tmp");
        let snapshot = SessionSnapshot::new(metadata, vec![]);

        // Save
        storage.save(&snapshot).await.unwrap();
        assert!(storage.exists("file-test").await);

        // Load
        let loaded = storage.load("file-test").await.unwrap();
        assert_eq!(loaded.metadata.id, "file-test");

        // Delete
        storage.delete("file-test").await.unwrap();
        assert!(!storage.exists("file-test").await);
    }
}
