//! Agent State - Runtime state for agent execution

use crate::agent::types::{
    AgentMessage, PendingPermission, PendingPermissionState, PermissionMode, PermissionRequest,
    PermissionResponse, Usage,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::RwLock;

/// Agent runtime state
pub struct AgentState {
    /// Session ID
    pub session_id: String,
    /// Current turn number
    turn: AtomicU32,
    /// Whether the agent is running
    running: AtomicBool,
    /// Whether interrupt was requested
    interrupted: AtomicBool,
    /// Whether waiting for permission response
    waiting_for_permission: AtomicBool,
    /// Current permission mode
    permission_mode: RwLock<PermissionMode>,
    /// Message history
    messages: RwLock<Vec<AgentMessage>>,
    /// Token usage
    usage: RwLock<Usage>,
    /// Total cost in USD
    total_cost_usd: RwLock<f64>,
    /// Start time (uses std::sync::Mutex to avoid blocking_write in async context)
    start_time: Mutex<Option<Instant>>,
    /// Metadata
    metadata: RwLock<HashMap<String, serde_json::Value>>,
    /// Pending permission requests
    pending_permissions: RwLock<HashMap<String, PendingPermission>>,
    /// Channel for permission responses (wrapped in RwLock to allow reset)
    permission_tx: RwLock<tokio::sync::mpsc::Sender<PermissionResponse>>,
    /// Receiver for permission responses (wrapped in Option for taking)
    permission_rx: RwLock<Option<tokio::sync::mpsc::Receiver<PermissionResponse>>>,
}

impl AgentState {
    /// Create a new agent state
    pub fn new(session_id: impl Into<String>) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        Self {
            session_id: session_id.into(),
            turn: AtomicU32::new(0),
            running: AtomicBool::new(false),
            interrupted: AtomicBool::new(false),
            waiting_for_permission: AtomicBool::new(false),
            permission_mode: RwLock::new(PermissionMode::BypassPermissions),
            messages: RwLock::new(Vec::new()),
            usage: RwLock::new(Usage::default()),
            total_cost_usd: RwLock::new(0.0),
            start_time: Mutex::new(None),
            metadata: RwLock::new(HashMap::new()),
            pending_permissions: RwLock::new(HashMap::new()),
            permission_tx: RwLock::new(tx),
            permission_rx: RwLock::new(Some(rx)),
        }
    }

    /// Create with a specific permission mode
    pub fn with_permission_mode(session_id: impl Into<String>, mode: PermissionMode) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        Self {
            session_id: session_id.into(),
            turn: AtomicU32::new(0),
            running: AtomicBool::new(false),
            interrupted: AtomicBool::new(false),
            waiting_for_permission: AtomicBool::new(false),
            permission_mode: RwLock::new(mode),
            messages: RwLock::new(Vec::new()),
            usage: RwLock::new(Usage::default()),
            total_cost_usd: RwLock::new(0.0),
            start_time: Mutex::new(None),
            metadata: RwLock::new(HashMap::new()),
            pending_permissions: RwLock::new(HashMap::new()),
            permission_tx: RwLock::new(tx),
            permission_rx: RwLock::new(Some(rx)),
        }
    }

    /// Get current turn
    pub fn turn(&self) -> u32 {
        self.turn.load(Ordering::SeqCst)
    }

    /// Increment turn
    pub fn increment_turn(&self) -> u32 {
        self.turn.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Check if running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Set running state
    pub fn set_running(&self, running: bool) {
        self.running.store(running, Ordering::SeqCst);
        if running {
            *self.start_time.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        }
    }

    /// Check if interrupted
    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    /// Request interrupt
    pub fn interrupt(&self) {
        self.interrupted.store(true, Ordering::SeqCst);
    }

    /// Clear interrupt flag
    pub fn clear_interrupt(&self) {
        self.interrupted.store(false, Ordering::SeqCst);
    }

    /// Get permission mode
    pub async fn permission_mode(&self) -> PermissionMode {
        *self.permission_mode.read().await
    }

    /// Set permission mode
    pub async fn set_permission_mode(&self, mode: PermissionMode) {
        *self.permission_mode.write().await = mode;
    }

    /// Get messages
    pub async fn messages(&self) -> Vec<AgentMessage> {
        self.messages.read().await.clone()
    }

    /// Add message
    pub async fn add_message(&self, message: AgentMessage) {
        self.messages.write().await.push(message);
    }

    /// Get message count
    pub async fn message_count(&self) -> usize {
        self.messages.read().await.len()
    }

    /// Replace all messages (used for context compaction)
    pub async fn replace_messages(&self, new_messages: Vec<AgentMessage>) {
        *self.messages.write().await = new_messages;
    }

    /// Get usage
    pub async fn usage(&self) -> Usage {
        self.usage.read().await.clone()
    }

    /// Add usage
    pub async fn add_usage(&self, usage: &Usage) {
        self.usage.write().await.add(usage);
    }

    /// Get total cost
    pub async fn total_cost_usd(&self) -> f64 {
        *self.total_cost_usd.read().await
    }

    /// Add cost
    pub async fn add_cost(&self, cost: f64) {
        *self.total_cost_usd.write().await += cost;
    }

    /// Get elapsed time in milliseconds
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }

    /// Set metadata
    pub async fn set_metadata(&self, key: impl Into<String>, value: serde_json::Value) {
        self.metadata.write().await.insert(key.into(), value);
    }

    /// Get metadata
    pub async fn get_metadata(&self, key: &str) -> Option<serde_json::Value> {
        self.metadata.read().await.get(key).cloned()
    }

    /// Reset state for new run
    pub async fn reset(&self) {
        self.turn.store(0, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
        self.interrupted.store(false, Ordering::SeqCst);
        self.waiting_for_permission.store(false, Ordering::SeqCst);
        self.messages.write().await.clear();
        *self.usage.write().await = Usage::default();
        *self.total_cost_usd.write().await = 0.0;
        *self.start_time.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.pending_permissions.write().await.clear();
    }

    // ========================================================================
    // Permission Request/Response Handling
    // ========================================================================

    /// Check if waiting for permission response
    pub fn is_waiting_for_permission(&self) -> bool {
        self.waiting_for_permission.load(Ordering::SeqCst)
    }

    /// Set waiting for permission state
    pub fn set_waiting_for_permission(&self, waiting: bool) {
        self.waiting_for_permission.store(waiting, Ordering::SeqCst);
    }

    /// Add a pending permission request
    pub async fn add_pending_permission(&self, request: PermissionRequest) {
        let pending = PendingPermission {
            request: request.clone(),
            state: PendingPermissionState::Pending,
            updated_at: chrono::Utc::now(),
        };
        self.pending_permissions
            .write()
            .await
            .insert(request.request_id.clone(), pending);
        self.set_waiting_for_permission(true);
    }

    /// Get a pending permission request
    pub async fn get_pending_permission(&self, request_id: &str) -> Option<PendingPermission> {
        self.pending_permissions
            .read()
            .await
            .get(request_id)
            .cloned()
    }

    /// Get all pending permission requests
    pub async fn get_all_pending_permissions(&self) -> Vec<PendingPermission> {
        self.pending_permissions
            .read()
            .await
            .values()
            .cloned()
            .collect()
    }

    /// Check if there are any pending permissions
    pub async fn has_pending_permissions(&self) -> bool {
        let pending = self.pending_permissions.read().await;
        pending
            .values()
            .any(|p| matches!(p.state, PendingPermissionState::Pending))
    }

    /// Update pending permission state
    pub async fn update_pending_permission(
        &self,
        request_id: &str,
        new_state: PendingPermissionState,
    ) -> Option<PendingPermission> {
        let mut pending = self.pending_permissions.write().await;
        if let Some(perm) = pending.get_mut(request_id) {
            perm.state = new_state;
            perm.updated_at = chrono::Utc::now();
            let result = perm.clone();

            // Check if we're still waiting for any permissions
            // Use the cloned result to avoid borrow conflict
            let still_waiting = pending
                .values()
                .any(|p| matches!(p.state, PendingPermissionState::Pending));
            drop(pending); // Release the lock before calling set_waiting_for_permission
            self.set_waiting_for_permission(still_waiting);

            Some(result)
        } else {
            None
        }
    }

    /// Remove a pending permission request
    pub async fn remove_pending_permission(&self, request_id: &str) -> Option<PendingPermission> {
        let removed = self.pending_permissions.write().await.remove(request_id);

        // Check if we're still waiting for any permissions
        let still_waiting = self
            .pending_permissions
            .read()
            .await
            .values()
            .any(|p| matches!(p.state, PendingPermissionState::Pending));
        self.set_waiting_for_permission(still_waiting);

        removed
    }

    /// Submit a permission response
    ///
    /// This sends the response through the channel to be processed by the agent runner.
    pub async fn submit_permission_response(
        &self,
        response: PermissionResponse,
    ) -> Result<(), String> {
        // Update the pending permission state
        let new_state = if response.granted {
            PendingPermissionState::Granted {
                modified_input: response.modified_input.clone(),
            }
        } else {
            PendingPermissionState::Denied
        };

        if self
            .update_pending_permission(&response.request_id, new_state)
            .await
            .is_none()
        {
            return Err(format!(
                "Permission request {} not found",
                response.request_id
            ));
        }

        // Send through channel
        self.permission_tx
            .read()
            .await
            .send(response)
            .await
            .map_err(|e| format!("Failed to send permission response: {}", e))
    }

    /// Get the permission response sender (for external use)
    pub async fn permission_sender(&self) -> tokio::sync::mpsc::Sender<PermissionResponse> {
        self.permission_tx.read().await.clone()
    }

    /// Take the permission response receiver, creating a fresh channel if needed.
    ///
    /// This ensures each `query()` call gets a working receiver, even if
    /// a previous call already consumed the original one.
    pub async fn take_permission_receiver(
        &self,
    ) -> Option<tokio::sync::mpsc::Receiver<PermissionResponse>> {
        let mut rx_guard = self.permission_rx.write().await;
        if rx_guard.is_some() {
            return rx_guard.take();
        }
        // Previous query already took the receiver — create a fresh channel
        drop(rx_guard);
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        *self.permission_tx.write().await = tx;
        Some(rx)
    }

    /// Cancel all pending permission requests
    pub async fn cancel_pending_permissions(&self) {
        let mut pending = self.pending_permissions.write().await;
        for perm in pending.values_mut() {
            if matches!(perm.state, PendingPermissionState::Pending) {
                perm.state = PendingPermissionState::Cancelled;
                perm.updated_at = chrono::Utc::now();
            }
        }
        self.set_waiting_for_permission(false);
    }

    /// Timeout a specific pending permission request
    pub async fn timeout_pending_permission(&self, request_id: &str) -> Option<PendingPermission> {
        self.update_pending_permission(request_id, PendingPermissionState::TimedOut)
            .await
    }
}

impl Default for AgentState {
    fn default() -> Self {
        Self::new(uuid::Uuid::new_v4().to_string())
    }
}

/// Shared agent state
pub type SharedAgentState = Arc<AgentState>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_state_turn() {
        let state = AgentState::new("test");
        assert_eq!(state.turn(), 0);

        let turn = state.increment_turn();
        assert_eq!(turn, 1);
        assert_eq!(state.turn(), 1);
    }

    #[test]
    fn test_agent_state_running() {
        let state = AgentState::new("test");
        assert!(!state.is_running());

        state.set_running(true);
        assert!(state.is_running());

        state.set_running(false);
        assert!(!state.is_running());
    }

    #[test]
    fn test_agent_state_interrupt() {
        let state = AgentState::new("test");
        assert!(!state.is_interrupted());

        state.interrupt();
        assert!(state.is_interrupted());

        state.clear_interrupt();
        assert!(!state.is_interrupted());
    }

    #[tokio::test]
    async fn test_agent_state_usage() {
        let state = AgentState::new("test");

        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        };

        state.add_usage(&usage).await;
        let total = state.usage().await;
        assert_eq!(total.input_tokens, 100);
        assert_eq!(total.output_tokens, 50);
    }

    #[tokio::test]
    async fn test_pending_permissions() {
        let state = AgentState::new("test-session");

        // Initially no pending permissions
        assert!(!state.has_pending_permissions().await);
        assert!(!state.is_waiting_for_permission());

        // Add a permission request
        let request = PermissionRequest::new(
            "Bash",
            serde_json::json!({"command": "ls -la"}),
            "Allow Bash to execute?",
            "test-session",
        );
        let request_id = request.request_id.clone();

        state.add_pending_permission(request).await;

        // Now we should be waiting
        assert!(state.has_pending_permissions().await);
        assert!(state.is_waiting_for_permission());

        // Get the pending permission
        let pending = state.get_pending_permission(&request_id).await;
        assert!(pending.is_some());
        assert!(matches!(
            pending.unwrap().state,
            PendingPermissionState::Pending
        ));

        // Submit a response
        let response = PermissionResponse::allow(&request_id, "test-session");
        state.submit_permission_response(response).await.unwrap();

        // Check state was updated
        let pending = state.get_pending_permission(&request_id).await;
        assert!(pending.is_some());
        assert!(matches!(
            pending.unwrap().state,
            PendingPermissionState::Granted { .. }
        ));

        // No longer waiting
        assert!(!state.is_waiting_for_permission());
    }

    #[tokio::test]
    async fn test_cancel_pending_permissions() {
        let state = AgentState::new("test-session");

        // Add two permission requests
        let request1 =
            PermissionRequest::new("Bash", serde_json::json!({}), "Allow Bash?", "test-session");
        let request2 = PermissionRequest::new(
            "Write",
            serde_json::json!({}),
            "Allow Write?",
            "test-session",
        );

        state.add_pending_permission(request1).await;
        state.add_pending_permission(request2).await;

        assert!(state.is_waiting_for_permission());
        assert_eq!(state.get_all_pending_permissions().await.len(), 2);

        // Cancel all
        state.cancel_pending_permissions().await;

        assert!(!state.is_waiting_for_permission());

        // All should be cancelled
        for perm in state.get_all_pending_permissions().await {
            assert!(matches!(perm.state, PendingPermissionState::Cancelled));
        }
    }
}
