//! Workflow Checkpoint System
//!
//! Provides checkpoint and recovery capabilities for workflow executions.

use super::engine::{ExecutionStatus, WorkflowExecution};
use crate::error::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Checkpoint data for a workflow execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub execution_id: String,
    pub workflow_id: String,
    pub step_index: usize,
    pub step_id: String,
    pub status: ExecutionStatus,
    pub step_results: HashMap<String, serde_json::Value>,
    pub context: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// Checkpoint store for persisting workflow state
pub struct CheckpointStore {
    checkpoints: Arc<RwLock<HashMap<String, Vec<Checkpoint>>>>,
}

impl CheckpointStore {
    /// Create a new checkpoint store
    pub fn new() -> Self {
        Self {
            checkpoints: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Save a checkpoint for an execution
    pub async fn save(&self, execution: &WorkflowExecution, step_id: &str) -> Result<Checkpoint> {
        let checkpoint = Checkpoint {
            id: Uuid::new_v4().to_string(),
            execution_id: execution.id.clone(),
            workflow_id: execution.workflow_id.clone(),
            step_index: execution.current_step,
            step_id: step_id.to_string(),
            status: execution.status.clone(),
            step_results: execution.step_results.clone(),
            context: execution.input.clone(),
            created_at: Utc::now(),
        };

        let mut checkpoints = self.checkpoints.write().await;
        let exec_checkpoints = checkpoints
            .entry(execution.id.clone())
            .or_insert_with(Vec::new);
        exec_checkpoints.push(checkpoint.clone());

        // R2-M92: Cap checkpoints per execution to prevent unbounded growth
        const MAX_CHECKPOINTS_PER_EXECUTION: usize = 100;
        if exec_checkpoints.len() > MAX_CHECKPOINTS_PER_EXECUTION {
            let drain_count = exec_checkpoints.len() - MAX_CHECKPOINTS_PER_EXECUTION;
            exec_checkpoints.drain(..drain_count);
        }

        tracing::info!(
            checkpoint_id = %checkpoint.id,
            execution_id = %execution.id,
            step_id = %step_id,
            "Checkpoint saved"
        );

        Ok(checkpoint)
    }

    /// Get the latest checkpoint for an execution
    pub async fn get_latest(&self, execution_id: &str) -> Option<Checkpoint> {
        let checkpoints = self.checkpoints.read().await;
        checkpoints
            .get(execution_id)
            .and_then(|cps| cps.last().cloned())
    }

    /// Get all checkpoints for an execution
    pub async fn get_all(&self, execution_id: &str) -> Vec<Checkpoint> {
        let checkpoints = self.checkpoints.read().await;
        checkpoints.get(execution_id).cloned().unwrap_or_default()
    }

    /// Delete all checkpoints for an execution
    pub async fn delete(&self, execution_id: &str) -> Result<usize> {
        let mut checkpoints = self.checkpoints.write().await;
        let count = checkpoints
            .remove(execution_id)
            .map(|cps| cps.len())
            .unwrap_or(0);

        tracing::info!(
            execution_id = %execution_id,
            deleted_count = count,
            "Checkpoints deleted"
        );

        Ok(count)
    }

    /// Restore execution state from checkpoint
    pub async fn restore(&self, execution_id: &str) -> Result<Option<WorkflowExecution>> {
        let checkpoint = match self.get_latest(execution_id).await {
            Some(cp) => cp,
            None => return Ok(None),
        };

        let execution = WorkflowExecution {
            id: checkpoint.execution_id,
            workflow_id: checkpoint.workflow_id,
            status: ExecutionStatus::Paused, // Restored executions start as paused
            current_step: checkpoint.step_index,
            input: checkpoint.context,
            output: None,
            step_results: checkpoint.step_results,
            error: None,
            started_at: Some(checkpoint.created_at),
            completed_at: None,
        };

        tracing::info!(
            execution_id = %execution.id,
            restored_from_checkpoint = %checkpoint.id,
            "Execution restored from checkpoint"
        );

        Ok(Some(execution))
    }
}

impl Default for CheckpointStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_execution() -> WorkflowExecution {
        WorkflowExecution {
            id: "exec-123".to_string(),
            workflow_id: "wf-456".to_string(),
            status: ExecutionStatus::Running,
            current_step: 2,
            input: serde_json::json!({"test": "data"}),
            output: None,
            step_results: HashMap::from([
                ("step-1".to_string(), serde_json::json!({"result": 1})),
                ("step-2".to_string(), serde_json::json!({"result": 2})),
            ]),
            error: None,
            started_at: Some(Utc::now()),
            completed_at: None,
        }
    }

    #[tokio::test]
    async fn test_save_checkpoint() {
        let store = CheckpointStore::new();
        let execution = create_test_execution();

        let checkpoint = store.save(&execution, "step-2").await.unwrap();

        assert_eq!(checkpoint.execution_id, "exec-123");
        assert_eq!(checkpoint.step_id, "step-2");
        assert_eq!(checkpoint.step_index, 2);
    }

    #[tokio::test]
    async fn test_get_latest_checkpoint() {
        let store = CheckpointStore::new();
        let execution = create_test_execution();

        store.save(&execution, "step-1").await.unwrap();
        store.save(&execution, "step-2").await.unwrap();

        let latest = store.get_latest("exec-123").await.unwrap();
        assert_eq!(latest.step_id, "step-2");
    }

    #[tokio::test]
    async fn test_restore_execution() {
        let store = CheckpointStore::new();
        let execution = create_test_execution();

        store.save(&execution, "step-2").await.unwrap();

        let restored = store.restore("exec-123").await.unwrap().unwrap();
        assert_eq!(restored.id, "exec-123");
        assert_eq!(restored.status, ExecutionStatus::Paused);
        assert_eq!(restored.current_step, 2);
    }

    #[tokio::test]
    async fn test_delete_checkpoints() {
        let store = CheckpointStore::new();
        let execution = create_test_execution();

        store.save(&execution, "step-1").await.unwrap();
        store.save(&execution, "step-2").await.unwrap();

        let deleted = store.delete("exec-123").await.unwrap();
        assert_eq!(deleted, 2);

        let latest = store.get_latest("exec-123").await;
        assert!(latest.is_none());
    }
}
