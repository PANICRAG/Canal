//! # Step Delegation Protocol (A43)
//!
//! Enables the backend to delegate plan step execution to the client (Swift/Web)
//! via SSE events. The client executes the step locally and sends the result
//! back through a REST endpoint.
//!
//! Architecture mirrors `rte::delegate::PendingToolExecutions`:
//! - DashMap + oneshot channels for async-wait semantics
//! - Register → SSE event → client executes → REST result → resume

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use uuid::Uuid;

// ============================================================================
// Types
// ============================================================================

/// A step execution request sent to the client via SSE.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepExecuteRequest {
    /// Unique request ID for correlation.
    pub request_id: Uuid,
    /// Session ID this step belongs to.
    pub session_id: Uuid,
    /// Step index in the plan.
    pub step_index: u32,
    /// Step title/description for the client to display.
    pub step_title: String,
    /// The action to execute (e.g., "bash", "file_edit", "browser", "api_call").
    pub action: String,
    /// Action parameters as JSON.
    pub parameters: serde_json::Value,
    /// Timeout in seconds for the client to respond.
    pub timeout_secs: u64,
}

/// Result of step execution sent from client to server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepExecuteResult {
    /// Must match the request's request_id.
    pub request_id: Uuid,
    /// Whether the step succeeded.
    pub success: bool,
    /// The output/result of the step.
    pub output: serde_json::Value,
    /// Error message if success is false.
    pub error: Option<String>,
    /// Execution time in milliseconds.
    pub execution_time_ms: u64,
}

/// Errors from the step delegation protocol.
#[derive(Debug, thiserror::Error)]
pub enum StepDelegateError {
    #[error("step request not found: {0}")]
    RequestNotFound(Uuid),
    #[error("client disconnected for request: {0}")]
    ClientDisconnected(Uuid),
}

// ============================================================================
// Pending Store
// ============================================================================

/// A pending step execution waiting for client completion.
#[derive(Debug)]
struct PendingStepEntry {
    request_id: Uuid,
    session_id: Uuid,
    step_index: u32,
    created_at: Instant,
    timeout: Duration,
    resume_tx: Option<oneshot::Sender<StepExecuteResult>>,
}

/// Thread-safe store for pending step executions.
///
/// Mirrors `PendingToolExecutions` from `rte::delegate`.
#[derive(Debug, Clone)]
pub struct PendingStepExecutions {
    entries: Arc<DashMap<Uuid, PendingStepEntry>>,
}

impl PendingStepExecutions {
    /// Create an empty store.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
        }
    }

    /// Register a pending step execution.
    ///
    /// Returns a oneshot receiver that the graph node awaits.
    /// The node sends a `StepExecuteRequest` via SSE, then awaits this receiver.
    pub fn register(
        &self,
        request_id: Uuid,
        session_id: Uuid,
        step_index: u32,
        timeout: Duration,
    ) -> oneshot::Receiver<StepExecuteResult> {
        let (tx, rx) = oneshot::channel();
        self.entries.insert(
            request_id,
            PendingStepEntry {
                request_id,
                session_id,
                step_index,
                created_at: Instant::now(),
                timeout,
                resume_tx: Some(tx),
            },
        );
        rx
    }

    /// Complete a pending step execution by delivering the client's result.
    ///
    /// This unblocks the graph node waiting on the oneshot receiver.
    pub fn complete(
        &self,
        request_id: &Uuid,
        result: StepExecuteResult,
    ) -> Result<(), StepDelegateError> {
        match self.entries.remove(request_id) {
            Some((_, mut entry)) => {
                if let Some(tx) = entry.resume_tx.take() {
                    tx.send(result)
                        .map_err(|_| StepDelegateError::ClientDisconnected(*request_id))
                } else {
                    Err(StepDelegateError::ClientDisconnected(*request_id))
                }
            }
            None => Err(StepDelegateError::RequestNotFound(*request_id)),
        }
    }

    /// Number of pending step executions.
    pub fn pending_count(&self) -> usize {
        self.entries.len()
    }

    /// Check if a request is pending.
    pub fn is_pending(&self, request_id: &Uuid) -> bool {
        self.entries.contains_key(request_id)
    }

    /// Get all pending request IDs for a session.
    pub fn get_session_pending(&self, session_id: &Uuid) -> Vec<Uuid> {
        self.entries
            .iter()
            .filter(|e| &e.session_id == session_id)
            .map(|e| e.request_id)
            .collect()
    }

    /// Evict expired entries (timeout × 2 grace period).
    ///
    /// Returns the number of evicted entries.
    pub fn evict_expired(&self) -> usize {
        let now = Instant::now();
        let expired: Vec<Uuid> = self
            .entries
            .iter()
            .filter(|e| now.duration_since(e.created_at) > e.timeout * 2)
            .map(|e| e.request_id)
            .collect();
        let count = expired.len();
        for id in expired {
            self.entries.remove(&id);
        }
        count
    }
}

impl Default for PendingStepExecutions {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pending_step_register_complete() {
        let store = PendingStepExecutions::new();
        let request_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        // Register
        let rx = store.register(request_id, session_id, 0, Duration::from_secs(30));
        assert_eq!(store.pending_count(), 1);
        assert!(store.is_pending(&request_id));

        // Complete
        let result = StepExecuteResult {
            request_id,
            success: true,
            output: serde_json::json!({"file": "main.rs", "lines_changed": 5}),
            error: None,
            execution_time_ms: 250,
        };
        store.complete(&request_id, result).unwrap();
        assert_eq!(store.pending_count(), 0);
        assert!(!store.is_pending(&request_id));

        // Verify the receiver got the result
        let received = rx.await.unwrap();
        assert!(received.success);
        assert_eq!(received.execution_time_ms, 250);
    }

    #[tokio::test]
    async fn test_pending_step_timeout_evict() {
        let store = PendingStepExecutions::new();
        let request_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        // Register with very short timeout
        let _rx = store.register(request_id, session_id, 0, Duration::from_millis(1));
        assert_eq!(store.pending_count(), 1);

        // Wait for timeout * 2 + margin
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Evict
        let evicted = store.evict_expired();
        assert_eq!(evicted, 1);
        assert_eq!(store.pending_count(), 0);
    }

    #[test]
    fn test_step_execute_request_serialize() {
        let req = StepExecuteRequest {
            request_id: Uuid::nil(),
            session_id: Uuid::nil(),
            step_index: 2,
            step_title: "Edit config file".into(),
            action: "file_edit".into(),
            parameters: serde_json::json!({"path": "/etc/config.yaml", "content": "key: value"}),
            timeout_secs: 60,
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("file_edit"));
        assert!(json.contains("step_index"));

        // Roundtrip
        let deserialized: StepExecuteRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.step_index, 2);
        assert_eq!(deserialized.action, "file_edit");
    }

    #[test]
    fn test_step_execute_result_deserialize() {
        let json = r#"{
            "request_id": "00000000-0000-0000-0000-000000000000",
            "success": true,
            "output": {"status": "done", "lines": 42},
            "error": null,
            "execution_time_ms": 1500
        }"#;

        let result: StepExecuteResult = serde_json::from_str(json).unwrap();
        assert!(result.success);
        assert_eq!(result.execution_time_ms, 1500);
        assert!(result.error.is_none());
        assert_eq!(result.output["lines"], 42);
    }

    #[tokio::test]
    async fn test_step_result_endpoint_200() {
        // Simulates the REST endpoint flow: register → complete via "endpoint"
        let store = PendingStepExecutions::new();
        let request_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        let rx = store.register(request_id, session_id, 1, Duration::from_secs(30));

        // Simulate endpoint receiving result
        let result = StepExecuteResult {
            request_id,
            success: true,
            output: serde_json::json!({"result": "compiled successfully"}),
            error: None,
            execution_time_ms: 800,
        };

        assert!(store.complete(&request_id, result).is_ok());

        let received = rx.await.unwrap();
        assert!(received.success);
        assert_eq!(received.output["result"], "compiled successfully");
    }

    #[test]
    fn test_step_result_endpoint_404_unknown_request() {
        let store = PendingStepExecutions::new();
        let unknown_id = Uuid::new_v4();

        let result = StepExecuteResult {
            request_id: unknown_id,
            success: true,
            output: serde_json::json!({}),
            error: None,
            execution_time_ms: 0,
        };

        let err = store.complete(&unknown_id, result).unwrap_err();
        assert!(matches!(err, StepDelegateError::RequestNotFound(_)));
    }

    #[tokio::test]
    async fn test_executor_node_sends_sse_event() {
        // Simulates the full flow: register → SSE event → client response → resume
        let store = PendingStepExecutions::new();
        let request_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        // 1. Executor node registers pending step
        let rx = store.register(request_id, session_id, 3, Duration::from_secs(60));

        // 2. Build SSE event (would be sent via EventSource)
        let sse_event = StepExecuteRequest {
            request_id,
            session_id,
            step_index: 3,
            step_title: "Run tests".into(),
            action: "bash".into(),
            parameters: serde_json::json!({"command": "cargo test"}),
            timeout_secs: 60,
        };
        let sse_json = serde_json::to_string(&sse_event).unwrap();
        assert!(sse_json.contains("cargo test"));

        // 3. Client responds with result
        let client_result = StepExecuteResult {
            request_id,
            success: true,
            output: serde_json::json!({"exit_code": 0, "stdout": "test result: ok"}),
            error: None,
            execution_time_ms: 5000,
        };
        store.complete(&request_id, client_result).unwrap();

        // 4. Executor node resumes
        let result = rx.await.unwrap();
        assert!(result.success);
        assert_eq!(result.output["exit_code"], 0);
    }

    #[tokio::test]
    async fn test_existing_tool_execution_unchanged() {
        // Regression: step delegation store is independent of tool execution
        let step_store = PendingStepExecutions::new();

        // Register a step
        let step_id = Uuid::new_v4();
        let _rx = step_store.register(step_id, Uuid::new_v4(), 0, Duration::from_secs(30));

        // Step store operations don't affect tool store (they're separate types)
        assert_eq!(step_store.pending_count(), 1);

        // Session-scoped queries work independently
        let other_session = Uuid::new_v4();
        assert!(step_store.get_session_pending(&other_session).is_empty());
    }
}
