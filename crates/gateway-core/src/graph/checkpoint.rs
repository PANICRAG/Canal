//! Graph checkpointing for state persistence and crash recovery.
//!
//! The checkpointer saves graph state at node boundaries, allowing
//! execution to be resumed from any checkpoint after a crash or pause.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::error::{GraphError, NodeId};
use super::GraphState;

/// Information about a stored checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointInfo {
    /// Unique checkpoint ID.
    pub id: String,
    /// The graph execution ID.
    pub graph_execution_id: String,
    /// The node where the checkpoint was taken.
    pub node_id: NodeId,
    /// When the checkpoint was created.
    pub created_at: DateTime<Utc>,
    /// Optional label for the checkpoint.
    pub label: Option<String>,
}

/// Trait for graph state checkpointing.
///
/// Implementations persist graph state at node boundaries, enabling
/// crash recovery and execution resumption.
#[async_trait]
pub trait GraphCheckpointer<S: GraphState>: Send + Sync {
    /// Save the current state at a node boundary.
    async fn save(
        &self,
        graph_execution_id: &str,
        node_id: &NodeId,
        state: &S,
    ) -> Result<String, GraphError>;

    /// Load a checkpoint by ID, returning the node ID and state.
    async fn load(&self, checkpoint_id: &str) -> Result<(NodeId, S), GraphError>;

    /// List all checkpoints for a graph execution.
    async fn list(&self, graph_execution_id: &str) -> Result<Vec<CheckpointInfo>, GraphError>;

    /// Delete a checkpoint.
    async fn delete(&self, checkpoint_id: &str) -> Result<(), GraphError>;
}

/// In-memory checkpointer for testing and development.
pub struct MemoryCheckpointer<S: GraphState> {
    checkpoints: Arc<RwLock<HashMap<String, (CheckpointInfo, NodeId, S)>>>,
}

impl<S: GraphState> MemoryCheckpointer<S> {
    /// Create a new in-memory checkpointer.
    pub fn new() -> Self {
        Self {
            checkpoints: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl<S: GraphState> Default for MemoryCheckpointer<S> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<S: GraphState> GraphCheckpointer<S> for MemoryCheckpointer<S> {
    async fn save(
        &self,
        graph_execution_id: &str,
        node_id: &NodeId,
        state: &S,
    ) -> Result<String, GraphError> {
        let id = format!("cp_{}", uuid::Uuid::new_v4());
        let info = CheckpointInfo {
            id: id.clone(),
            graph_execution_id: graph_execution_id.to_string(),
            node_id: node_id.clone(),
            created_at: Utc::now(),
            label: None,
        };
        let mut checkpoints = self.checkpoints.write().await;
        checkpoints.insert(id.clone(), (info, node_id.clone(), state.clone()));
        Ok(id)
    }

    async fn load(&self, checkpoint_id: &str) -> Result<(NodeId, S), GraphError> {
        let checkpoints = self.checkpoints.read().await;
        checkpoints
            .get(checkpoint_id)
            .map(|(_, node_id, state)| (node_id.clone(), state.clone()))
            .ok_or_else(|| {
                GraphError::CheckpointError(format!("checkpoint not found: {checkpoint_id}"))
            })
    }

    async fn list(&self, graph_execution_id: &str) -> Result<Vec<CheckpointInfo>, GraphError> {
        let checkpoints = self.checkpoints.read().await;
        Ok(checkpoints
            .values()
            .filter(|(info, _, _)| info.graph_execution_id == graph_execution_id)
            .map(|(info, _, _)| info.clone())
            .collect())
    }

    async fn delete(&self, checkpoint_id: &str) -> Result<(), GraphError> {
        let mut checkpoints = self.checkpoints.write().await;
        checkpoints.remove(checkpoint_id);
        Ok(())
    }
}

/// File-based checkpointer for persistence across restarts.
pub struct FileCheckpointer<S: GraphState> {
    base_dir: PathBuf,
    /// Mutex to serialize index file read-modify-write operations.
    index_lock: tokio::sync::Mutex<()>,
    _phantom: std::marker::PhantomData<S>,
}

impl<S: GraphState> FileCheckpointer<S> {
    /// Create a new file-based checkpointer.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            index_lock: tokio::sync::Mutex::new(()),
            _phantom: std::marker::PhantomData,
        }
    }

    fn checkpoint_path(&self, checkpoint_id: &str) -> Result<PathBuf, GraphError> {
        // Validate checkpoint_id to prevent path traversal.
        if !checkpoint_id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(GraphError::CheckpointError(
                "invalid checkpoint_id: must contain only alphanumeric, '-', or '_' characters"
                    .to_string(),
            ));
        }
        Ok(self.base_dir.join(format!("{checkpoint_id}.json")))
    }

    fn index_path(&self) -> PathBuf {
        self.base_dir.join("index.json")
    }
}

#[derive(Serialize, Deserialize)]
struct FileCheckpointData<S> {
    info: CheckpointInfo,
    node_id: NodeId,
    state: S,
}

#[async_trait]
impl<S: GraphState> GraphCheckpointer<S> for FileCheckpointer<S> {
    async fn save(
        &self,
        graph_execution_id: &str,
        node_id: &NodeId,
        state: &S,
    ) -> Result<String, GraphError> {
        tokio::fs::create_dir_all(&self.base_dir)
            .await
            .map_err(|e| GraphError::CheckpointError(format!("create dir failed: {e}")))?;

        let id = format!("cp_{}", uuid::Uuid::new_v4());
        let info = CheckpointInfo {
            id: id.clone(),
            graph_execution_id: graph_execution_id.to_string(),
            node_id: node_id.clone(),
            created_at: Utc::now(),
            label: None,
        };
        let data = FileCheckpointData {
            info: info.clone(),
            node_id: node_id.clone(),
            state: state.clone(),
        };
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| GraphError::SerializationError(e.to_string()))?;
        let cp_path = self.checkpoint_path(&id)?;
        tokio::fs::write(cp_path, json)
            .await
            .map_err(|e| GraphError::CheckpointError(format!("write failed: {e}")))?;

        // Update index (serialized via mutex to prevent TOCTOU races)
        let _guard = self.index_lock.lock().await;
        let mut index: Vec<CheckpointInfo> =
            if let Ok(content) = tokio::fs::read_to_string(self.index_path()).await {
                match serde_json::from_str(&content) {
                    Ok(index) => index,
                    Err(e) => {
                        tracing::warn!(error = %e, "Corrupt checkpoint index, starting fresh");
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
        index.push(info);
        let index_json = serde_json::to_string_pretty(&index)
            .map_err(|e| GraphError::SerializationError(e.to_string()))?;
        tokio::fs::write(self.index_path(), index_json)
            .await
            .map_err(|e| GraphError::CheckpointError(format!("index write failed: {e}")))?;
        drop(_guard);

        Ok(id)
    }

    async fn load(&self, checkpoint_id: &str) -> Result<(NodeId, S), GraphError> {
        let path = self.checkpoint_path(checkpoint_id)?;
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| GraphError::CheckpointError(format!("read failed: {e}")))?;
        let data: FileCheckpointData<S> = serde_json::from_str(&content)
            .map_err(|e| GraphError::SerializationError(e.to_string()))?;
        Ok((data.node_id, data.state))
    }

    async fn list(&self, graph_execution_id: &str) -> Result<Vec<CheckpointInfo>, GraphError> {
        let content = match tokio::fs::read_to_string(self.index_path()).await {
            Ok(c) => c,
            Err(_) => return Ok(Vec::new()),
        };
        let index: Vec<CheckpointInfo> = serde_json::from_str(&content)
            .map_err(|e| GraphError::SerializationError(e.to_string()))?;
        Ok(index
            .into_iter()
            .filter(|info| info.graph_execution_id == graph_execution_id)
            .collect())
    }

    async fn delete(&self, checkpoint_id: &str) -> Result<(), GraphError> {
        let path = self.checkpoint_path(checkpoint_id)?;
        if let Err(e) = tokio::fs::remove_file(&path).await {
            tracing::warn!(checkpoint_id, error = %e, "Failed to remove checkpoint file");
        }
        // Update index (serialized via mutex)
        let _guard = self.index_lock.lock().await;
        if let Ok(content) = tokio::fs::read_to_string(self.index_path()).await {
            if let Ok(mut index) = serde_json::from_str::<Vec<CheckpointInfo>>(&content) {
                index.retain(|info| info.id != checkpoint_id);
                if let Ok(json) = serde_json::to_string_pretty(&index) {
                    let _ = tokio::fs::write(self.index_path(), json).await;
                }
            }
        }
        drop(_guard);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct TestState {
        value: i32,
    }

    impl GraphState for TestState {
        fn merge(&mut self, other: Self) {
            self.value += other.value;
        }
    }

    #[tokio::test]
    async fn test_memory_checkpointer_save_load() {
        let cp = MemoryCheckpointer::<TestState>::new();
        let state = TestState { value: 42 };
        let id = cp.save("exec_1", &"node_a".into(), &state).await.unwrap();
        let (node_id, loaded_state) = cp.load(&id).await.unwrap();
        assert_eq!(node_id, "node_a");
        assert_eq!(loaded_state, state);
    }

    #[tokio::test]
    async fn test_memory_checkpointer_list() {
        let cp = MemoryCheckpointer::<TestState>::new();
        cp.save("exec_1", &"a".into(), &TestState { value: 1 })
            .await
            .unwrap();
        cp.save("exec_1", &"b".into(), &TestState { value: 2 })
            .await
            .unwrap();
        cp.save("exec_2", &"c".into(), &TestState { value: 3 })
            .await
            .unwrap();

        let list = cp.list("exec_1").await.unwrap();
        assert_eq!(list.len(), 2);

        let list = cp.list("exec_2").await.unwrap();
        assert_eq!(list.len(), 1);
    }

    #[tokio::test]
    async fn test_memory_checkpointer_delete() {
        let cp = MemoryCheckpointer::<TestState>::new();
        let id = cp
            .save("exec_1", &"a".into(), &TestState { value: 1 })
            .await
            .unwrap();
        cp.delete(&id).await.unwrap();
        assert!(cp.load(&id).await.is_err());
    }

    #[tokio::test]
    async fn test_memory_checkpointer_load_nonexistent() {
        let cp = MemoryCheckpointer::<TestState>::new();
        assert!(cp.load("nonexistent").await.is_err());
    }

    #[tokio::test]
    async fn test_file_checkpointer_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let cp = FileCheckpointer::<TestState>::new(dir.path());
        let state = TestState { value: 99 };
        let id = cp.save("exec_1", &"node_x".into(), &state).await.unwrap();
        let (node_id, loaded_state) = cp.load(&id).await.unwrap();
        assert_eq!(node_id, "node_x");
        assert_eq!(loaded_state, state);
    }

    #[tokio::test]
    async fn test_file_checkpointer_list() {
        let dir = tempfile::tempdir().unwrap();
        let cp = FileCheckpointer::<TestState>::new(dir.path());
        cp.save("exec_1", &"a".into(), &TestState { value: 1 })
            .await
            .unwrap();
        cp.save("exec_1", &"b".into(), &TestState { value: 2 })
            .await
            .unwrap();
        let list = cp.list("exec_1").await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_file_checkpointer_delete() {
        let dir = tempfile::tempdir().unwrap();
        let cp = FileCheckpointer::<TestState>::new(dir.path());
        let id = cp
            .save("exec_1", &"a".into(), &TestState { value: 1 })
            .await
            .unwrap();
        cp.delete(&id).await.unwrap();
        assert!(cp.load(&id).await.is_err());
    }
}
