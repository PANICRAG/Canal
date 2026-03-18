//! Background task management API endpoints
//!
//! Provides API routes for managing background shell tasks.
//! Supports listing, status checking, output retrieval, and stopping tasks.

use axum::{
    extract::{Path, State},
    routing::{delete, get},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::ExitStatus;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::process::Child;
use tokio::sync::RwLock;

use crate::{error::ApiError, state::AppState};

/// Maximum output length to return in API responses (64KB)
const MAX_OUTPUT_LENGTH: usize = 64 * 1024;

/// Create the background task routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_tasks))
        .route("/{id}", get(get_task))
        .route("/{id}/output", get(get_task_output))
        .route("/{id}", delete(stop_task))
}

// ============ Background Task Types ============

/// Background task status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// Task is currently running
    Running,
    /// Task completed successfully
    Completed,
    /// Task failed
    Failed,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Running => write!(f, "running"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Failed => write!(f, "failed"),
        }
    }
}

/// A background shell task (similar to Claude Code's BackgroundShell)
pub struct BackgroundTask {
    /// Unique task identifier
    pub id: String,
    /// The command being executed
    pub command: String,
    /// Current status
    pub status: TaskStatus,
    /// Accumulated stdout output
    pub stdout: String,
    /// Accumulated stderr output
    pub stderr: String,
    /// Exit code (if completed)
    pub exit_code: Option<i32>,
    /// When the task was started
    pub started_at: DateTime<Utc>,
    /// When the task completed (if completed)
    pub completed_at: Option<DateTime<Utc>>,
    /// The underlying child process (if still running)
    pub child: Option<Child>,
}

#[allow(dead_code)]
impl BackgroundTask {
    /// Create a new background task
    pub fn new(id: String, command: String, child: Child) -> Self {
        Self {
            id,
            command,
            status: TaskStatus::Running,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            started_at: Utc::now(),
            completed_at: None,
            child: Some(child),
        }
    }

    /// Mark the task as completed
    pub fn complete(&mut self, exit_status: ExitStatus) {
        self.status = if exit_status.success() {
            TaskStatus::Completed
        } else {
            TaskStatus::Failed
        };
        self.exit_code = exit_status.code();
        self.completed_at = Some(Utc::now());
        self.child = None;
    }

    /// Mark the task as failed with an error
    pub fn fail(&mut self, error: &str) {
        self.status = TaskStatus::Failed;
        self.stderr.push_str(error);
        self.completed_at = Some(Utc::now());
        self.child = None;
    }

    /// Get the combined output (stdout + stderr)
    pub fn combined_output(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.clone()
        } else if self.stdout.is_empty() {
            self.stderr.clone()
        } else {
            format!("{}\n{}", self.stdout, self.stderr)
        }
    }

    /// Get truncated output
    pub fn truncated_output(&self, max_len: usize) -> String {
        let output = self.combined_output();
        if output.len() <= max_len {
            output
        } else {
            format!(
                "{}...\n[truncated, {} bytes total]",
                &output[..max_len],
                output.len()
            )
        }
    }
}

/// Shared storage for background tasks
#[derive(Default)]
pub struct BackgroundTaskStore {
    tasks: HashMap<String, BackgroundTask>,
}

impl BackgroundTaskStore {
    /// Create a new task store
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    /// Add a task to the store
    #[allow(dead_code)]
    pub fn add_task(&mut self, task: BackgroundTask) {
        self.tasks.insert(task.id.clone(), task);
    }

    /// Get a task by ID
    pub fn get_task(&self, id: &str) -> Option<&BackgroundTask> {
        self.tasks.get(id)
    }

    /// Get a mutable reference to a task by ID
    pub fn get_task_mut(&mut self, id: &str) -> Option<&mut BackgroundTask> {
        self.tasks.get_mut(id)
    }

    /// Remove a task from the store
    #[allow(dead_code)]
    pub fn remove_task(&mut self, id: &str) -> Option<BackgroundTask> {
        self.tasks.remove(id)
    }

    /// List all tasks
    pub fn list_tasks(&self) -> Vec<&BackgroundTask> {
        self.tasks.values().collect()
    }
}

/// Thread-safe wrapper for BackgroundTaskStore
pub type SharedTaskStore = Arc<RwLock<BackgroundTaskStore>>;

/// Create a new shared task store
pub fn create_task_store() -> SharedTaskStore {
    Arc::new(RwLock::new(BackgroundTaskStore::new()))
}

// ============ Response Types ============

/// Task info response
#[derive(Debug, Serialize)]
pub struct TaskResponse {
    /// Unique task identifier
    pub id: String,
    /// The command being executed
    pub command: String,
    /// Current status: "running", "completed", or "failed"
    pub status: TaskStatus,
    /// Output (truncated to reasonable length)
    pub output: String,
    /// Exit code (null if still running)
    pub exit_code: Option<i32>,
    /// When the task was started (ISO 8601)
    pub started_at: String,
    /// When the task completed (ISO 8601, null if still running)
    pub completed_at: Option<String>,
}

impl From<&BackgroundTask> for TaskResponse {
    fn from(task: &BackgroundTask) -> Self {
        Self {
            id: task.id.clone(),
            command: task.command.clone(),
            status: task.status,
            output: task.truncated_output(MAX_OUTPUT_LENGTH),
            exit_code: task.exit_code,
            started_at: task.started_at.to_rfc3339(),
            completed_at: task.completed_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// Task list response
#[derive(Debug, Serialize)]
pub struct TaskListResponse {
    /// List of tasks
    pub tasks: Vec<TaskResponse>,
    /// Total count
    pub count: usize,
}

/// Task output response (full output)
#[derive(Debug, Serialize)]
pub struct TaskOutputResponse {
    /// Task ID
    pub id: String,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Combined output length
    pub total_bytes: usize,
}

/// Task stop response
#[derive(Debug, Serialize)]
pub struct TaskStopResponse {
    /// Whether the task was stopped
    pub stopped: bool,
    /// Task ID
    pub id: String,
    /// Message
    pub message: String,
}

// ============ Handlers ============

/// List all background tasks
pub async fn list_tasks(State(state): State<AppState>) -> Result<Json<TaskListResponse>, ApiError> {
    let store = state.task_store.read().await;
    let tasks: Vec<TaskResponse> = store.list_tasks().iter().map(|t| (*t).into()).collect();
    let count = tasks.len();

    Ok(Json(TaskListResponse { tasks, count }))
}

/// Get a specific task's status
pub async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskResponse>, ApiError> {
    // First, try to update the task status if it's running
    {
        let mut store = state.task_store.write().await;
        if let Some(task) = store.get_task_mut(&id) {
            if task.status == TaskStatus::Running {
                if let Some(ref mut child) = task.child {
                    // Try to get the exit status without blocking
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            // Process has completed
                            // Read remaining output
                            if let Some(ref mut stdout) = child.stdout {
                                let mut buf = String::new();
                                if let Ok(_) = stdout.read_to_string(&mut buf).await {
                                    task.stdout.push_str(&buf);
                                }
                            }
                            if let Some(ref mut stderr) = child.stderr {
                                let mut buf = String::new();
                                if let Ok(_) = stderr.read_to_string(&mut buf).await {
                                    task.stderr.push_str(&buf);
                                }
                            }
                            task.complete(status);
                        }
                        Ok(None) => {
                            // Still running, read available output
                            // Note: This is a simplified version; a real implementation
                            // would use non-blocking reads
                        }
                        Err(e) => {
                            task.fail(&format!("Failed to check process status: {}", e));
                        }
                    }
                }
            }
        }
    }

    // Now get the task for response
    let store = state.task_store.read().await;
    let task = store
        .get_task(&id)
        .ok_or_else(|| ApiError::not_found(format!("Task not found: {}", id)))?;

    Ok(Json(task.into()))
}

/// Get a task's full output
pub async fn get_task_output(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskOutputResponse>, ApiError> {
    let store = state.task_store.read().await;
    let task = store
        .get_task(&id)
        .ok_or_else(|| ApiError::not_found(format!("Task not found: {}", id)))?;

    let total_bytes = task.stdout.len() + task.stderr.len();

    Ok(Json(TaskOutputResponse {
        id: task.id.clone(),
        stdout: task.stdout.clone(),
        stderr: task.stderr.clone(),
        total_bytes,
    }))
}

/// Stop a running task
pub async fn stop_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TaskStopResponse>, ApiError> {
    let mut store = state.task_store.write().await;
    let task = store
        .get_task_mut(&id)
        .ok_or_else(|| ApiError::not_found(format!("Task not found: {}", id)))?;

    if task.status != TaskStatus::Running {
        return Ok(Json(TaskStopResponse {
            stopped: false,
            id: id.clone(),
            message: format!("Task is not running (status: {})", task.status),
        }));
    }

    // Kill the process
    if let Some(ref mut child) = task.child {
        match child.kill().await {
            Ok(()) => {
                tracing::info!(task_id = %id, "Background task killed");
                task.fail("Task was stopped by user");
                Ok(Json(TaskStopResponse {
                    stopped: true,
                    id,
                    message: "Task stopped successfully".to_string(),
                }))
            }
            Err(e) => {
                tracing::error!(task_id = %id, error = %e, "Failed to kill background task");
                Err(ApiError::internal(format!("Failed to stop task: {}", e)))
            }
        }
    } else {
        // No child process, mark as failed
        task.fail("Task has no associated process");
        Ok(Json(TaskStopResponse {
            stopped: true,
            id,
            message: "Task marked as stopped (no process)".to_string(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_status_display() {
        assert_eq!(TaskStatus::Running.to_string(), "running");
        assert_eq!(TaskStatus::Completed.to_string(), "completed");
        assert_eq!(TaskStatus::Failed.to_string(), "failed");
    }

    #[test]
    fn test_truncated_output() {
        let mut task = BackgroundTask {
            id: "test".to_string(),
            command: "test".to_string(),
            status: TaskStatus::Completed,
            stdout: "a".repeat(100),
            stderr: String::new(),
            exit_code: Some(0),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            child: None,
        };

        // Output under limit
        let output = task.truncated_output(200);
        assert_eq!(output.len(), 100);

        // Output over limit
        task.stdout = "a".repeat(1000);
        let output = task.truncated_output(100);
        assert!(output.contains("truncated"));
        assert!(output.contains("1000 bytes total"));
    }

    #[test]
    fn test_task_store() {
        let mut store = BackgroundTaskStore::new();

        // Create a mock task (without actual process)
        let task = BackgroundTask {
            id: "task-1".to_string(),
            command: "echo hello".to_string(),
            status: TaskStatus::Completed,
            stdout: "hello\n".to_string(),
            stderr: String::new(),
            exit_code: Some(0),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            child: None,
        };

        store.add_task(task);
        assert!(store.get_task("task-1").is_some());
        assert!(store.get_task("nonexistent").is_none());

        let tasks = store.list_tasks();
        assert_eq!(tasks.len(), 1);

        store.remove_task("task-1");
        assert!(store.get_task("task-1").is_none());
    }
}
