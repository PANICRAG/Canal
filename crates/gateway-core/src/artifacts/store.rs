//! Artifact Store
//!
//! In-memory storage for artifacts with future database integration support.

use super::types::*;
use crate::error::{Error, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Artifact store for managing artifact persistence
pub struct ArtifactStore {
    artifacts: Arc<RwLock<HashMap<ArtifactId, Artifact>>>,
}

impl ArtifactStore {
    /// Create a new in-memory artifact store
    pub fn new() -> Self {
        Self {
            artifacts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Save an artifact
    pub async fn save(&self, artifact: &Artifact) -> Result<()> {
        let mut artifacts = self.artifacts.write().await;
        artifacts.insert(artifact.id, artifact.clone());
        Ok(())
    }

    /// Get an artifact by ID
    pub async fn get(&self, id: ArtifactId) -> Result<Option<Artifact>> {
        let artifacts = self.artifacts.read().await;
        Ok(artifacts.get(&id).cloned())
    }

    /// List all artifacts for a session
    pub async fn list_by_session(&self, session_id: &str) -> Result<Vec<Artifact>> {
        let artifacts = self.artifacts.read().await;
        let mut result: Vec<Artifact> = artifacts
            .values()
            .filter(|a| a.session_id == session_id)
            .cloned()
            .collect();

        // Sort by created_at descending
        result.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(result)
    }

    /// List all artifacts for a message
    pub async fn list_by_message(&self, message_id: &str) -> Result<Vec<Artifact>> {
        let artifacts = self.artifacts.read().await;
        let mut result: Vec<Artifact> = artifacts
            .values()
            .filter(|a| a.message_id == message_id)
            .cloned()
            .collect();

        result.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(result)
    }

    /// Update artifact content
    pub async fn update_content(&self, id: ArtifactId, content: ArtifactContent) -> Result<()> {
        let mut artifacts = self.artifacts.write().await;
        if let Some(artifact) = artifacts.get_mut(&id) {
            artifact.content = content;
            artifact.updated_at = chrono::Utc::now();
            Ok(())
        } else {
            Err(Error::NotFound(format!("Artifact not found: {}", id)))
        }
    }

    /// Update approval status
    pub async fn update_approval(&self, id: ArtifactId, approved: bool) -> Result<()> {
        let mut artifacts = self.artifacts.write().await;
        if let Some(artifact) = artifacts.get_mut(&id) {
            if let ArtifactContent::ApprovalRequest {
                action,
                description,
                details,
                deadline,
                ..
            } = artifact.content.clone()
            {
                artifact.content = ArtifactContent::ApprovalRequest {
                    action,
                    description,
                    details,
                    deadline,
                    approved: Some(approved),
                };
                artifact.updated_at = chrono::Utc::now();
                Ok(())
            } else {
                Err(Error::Internal(
                    "Artifact is not an approval request".to_string(),
                ))
            }
        } else {
            Err(Error::NotFound(format!("Artifact not found: {}", id)))
        }
    }

    /// Delete an artifact
    pub async fn delete(&self, id: ArtifactId) -> Result<bool> {
        let mut artifacts = self.artifacts.write().await;
        Ok(artifacts.remove(&id).is_some())
    }

    /// Delete all artifacts for a session
    pub async fn delete_by_session(&self, session_id: &str) -> Result<usize> {
        let mut artifacts = self.artifacts.write().await;
        let ids_to_remove: Vec<ArtifactId> = artifacts
            .values()
            .filter(|a| a.session_id == session_id)
            .map(|a| a.id)
            .collect();

        let count = ids_to_remove.len();
        for id in ids_to_remove {
            artifacts.remove(&id);
        }

        Ok(count)
    }

    /// Get artifact count
    pub async fn count(&self) -> usize {
        let artifacts = self.artifacts.read().await;
        artifacts.len()
    }

    /// Get artifact count by type
    pub async fn count_by_type(&self, artifact_type: ArtifactType) -> usize {
        let artifacts = self.artifacts.read().await;
        artifacts
            .values()
            .filter(|a| a.artifact_type == artifact_type)
            .count()
    }

    /// List all artifacts
    pub async fn list_all(&self) -> Vec<Artifact> {
        let artifacts = self.artifacts.read().await;
        let mut result: Vec<Artifact> = artifacts.values().cloned().collect();
        result.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        result
    }
}

impl Default for ArtifactStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn create_test_artifact(session_id: &str, message_id: &str) -> Artifact {
        Artifact {
            id: Uuid::new_v4(),
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            artifact_type: ArtifactType::Document,
            title: "Test Document".to_string(),
            content: ArtifactContent::Document {
                format: DocumentFormat::Markdown,
                content: "# Test".to_string(),
                sections: vec![],
            },
            metadata: ArtifactMetadata::default(),
            actions: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn create_approval_artifact(session_id: &str, message_id: &str) -> Artifact {
        Artifact {
            id: Uuid::new_v4(),
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            artifact_type: ArtifactType::ApprovalRequest,
            title: "Test Approval".to_string(),
            content: ArtifactContent::ApprovalRequest {
                action: "publish".to_string(),
                description: "Publish video".to_string(),
                details: serde_json::json!({}),
                deadline: None,
                approved: None,
            },
            metadata: ArtifactMetadata::default(),
            actions: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_save_and_get() {
        let store = ArtifactStore::new();
        let artifact = create_test_artifact("session-1", "msg-1");
        let id = artifact.id;

        store.save(&artifact).await.unwrap();

        let retrieved = store.get(id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().title, "Test Document");
    }

    #[tokio::test]
    async fn test_get_not_found() {
        let store = ArtifactStore::new();
        let retrieved = store.get(Uuid::new_v4()).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_list_by_session() {
        let store = ArtifactStore::new();

        let artifact1 = create_test_artifact("session-1", "msg-1");
        let artifact2 = create_test_artifact("session-1", "msg-2");
        let artifact3 = create_test_artifact("session-2", "msg-3");

        store.save(&artifact1).await.unwrap();
        store.save(&artifact2).await.unwrap();
        store.save(&artifact3).await.unwrap();

        let session1_artifacts = store.list_by_session("session-1").await.unwrap();
        assert_eq!(session1_artifacts.len(), 2);

        let session2_artifacts = store.list_by_session("session-2").await.unwrap();
        assert_eq!(session2_artifacts.len(), 1);

        let session3_artifacts = store.list_by_session("session-3").await.unwrap();
        assert_eq!(session3_artifacts.len(), 0);
    }

    #[tokio::test]
    async fn test_list_by_message() {
        let store = ArtifactStore::new();

        let artifact1 = create_test_artifact("session-1", "msg-1");
        let artifact2 = create_test_artifact("session-1", "msg-1");
        let artifact3 = create_test_artifact("session-1", "msg-2");

        store.save(&artifact1).await.unwrap();
        store.save(&artifact2).await.unwrap();
        store.save(&artifact3).await.unwrap();

        let msg1_artifacts = store.list_by_message("msg-1").await.unwrap();
        assert_eq!(msg1_artifacts.len(), 2);

        let msg2_artifacts = store.list_by_message("msg-2").await.unwrap();
        assert_eq!(msg2_artifacts.len(), 1);
    }

    #[tokio::test]
    async fn test_update_content() {
        let store = ArtifactStore::new();
        let artifact = create_test_artifact("session-1", "msg-1");
        let id = artifact.id;

        store.save(&artifact).await.unwrap();

        let new_content = ArtifactContent::Document {
            format: DocumentFormat::Html,
            content: "<h1>Updated</h1>".to_string(),
            sections: vec![],
        };

        store.update_content(id, new_content).await.unwrap();

        let retrieved = store.get(id).await.unwrap().unwrap();
        if let ArtifactContent::Document {
            format, content, ..
        } = retrieved.content
        {
            assert_eq!(format, DocumentFormat::Html);
            assert_eq!(content, "<h1>Updated</h1>");
        } else {
            panic!("Expected Document content");
        }
    }

    #[tokio::test]
    async fn test_update_approval() {
        let store = ArtifactStore::new();
        let artifact = create_approval_artifact("session-1", "msg-1");
        let id = artifact.id;

        store.save(&artifact).await.unwrap();

        // Initially not approved
        let retrieved = store.get(id).await.unwrap().unwrap();
        if let ArtifactContent::ApprovalRequest { approved, .. } = retrieved.content {
            assert!(approved.is_none());
        }

        // Update to approved
        store.update_approval(id, true).await.unwrap();

        let retrieved = store.get(id).await.unwrap().unwrap();
        if let ArtifactContent::ApprovalRequest { approved, .. } = retrieved.content {
            assert_eq!(approved, Some(true));
        } else {
            panic!("Expected ApprovalRequest content");
        }
    }

    #[tokio::test]
    async fn test_update_approval_wrong_type() {
        let store = ArtifactStore::new();
        let artifact = create_test_artifact("session-1", "msg-1"); // Document, not ApprovalRequest
        let id = artifact.id;

        store.save(&artifact).await.unwrap();

        let result = store.update_approval(id, true).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delete() {
        let store = ArtifactStore::new();
        let artifact = create_test_artifact("session-1", "msg-1");
        let id = artifact.id;

        store.save(&artifact).await.unwrap();
        assert!(store.get(id).await.unwrap().is_some());

        let deleted = store.delete(id).await.unwrap();
        assert!(deleted);

        assert!(store.get(id).await.unwrap().is_none());

        // Delete again should return false
        let deleted = store.delete(id).await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_delete_by_session() {
        let store = ArtifactStore::new();

        let artifact1 = create_test_artifact("session-1", "msg-1");
        let artifact2 = create_test_artifact("session-1", "msg-2");
        let artifact3 = create_test_artifact("session-2", "msg-3");

        store.save(&artifact1).await.unwrap();
        store.save(&artifact2).await.unwrap();
        store.save(&artifact3).await.unwrap();

        assert_eq!(store.count().await, 3);

        let deleted = store.delete_by_session("session-1").await.unwrap();
        assert_eq!(deleted, 2);

        assert_eq!(store.count().await, 1);
        assert!(store.get(artifact3.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_count_by_type() {
        let store = ArtifactStore::new();

        let doc1 = create_test_artifact("session-1", "msg-1");
        let doc2 = create_test_artifact("session-1", "msg-2");
        let approval = create_approval_artifact("session-1", "msg-3");

        store.save(&doc1).await.unwrap();
        store.save(&doc2).await.unwrap();
        store.save(&approval).await.unwrap();

        assert_eq!(store.count_by_type(ArtifactType::Document).await, 2);
        assert_eq!(store.count_by_type(ArtifactType::ApprovalRequest).await, 1);
        assert_eq!(store.count_by_type(ArtifactType::Chart).await, 0);
    }
}
