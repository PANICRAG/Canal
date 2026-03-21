//! Mock RTE (Remote Tool Execution) Infrastructure
//!
//! Provides mock SSE client, tool execution simulator, and HMAC
//! utilities for testing the A28 RTE protocol end-to-end.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

// ============================================================
// RTE Protocol Types (shared between tests and implementation)
// ============================================================

/// Client capabilities sent in StreamChatRequest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockClientCapabilities {
    pub protocol_version: String,
    pub supported_tools: Vec<String>,
    pub platform: String,
    pub rte_enabled: bool,
    pub max_concurrent_tools: u32,
}

impl MockClientCapabilities {
    /// Windows client with full tool support
    pub fn windows_full() -> Self {
        Self {
            protocol_version: "1.0".to_string(),
            supported_tools: vec![
                "code_execute".to_string(),
                "file_read".to_string(),
                "file_write".to_string(),
                "browser_screenshot".to_string(),
                "browser_click".to_string(),
            ],
            platform: "windows".to_string(),
            rte_enabled: true,
            max_concurrent_tools: 3,
        }
    }

    /// macOS client with full tool support
    pub fn macos_full() -> Self {
        Self {
            protocol_version: "1.0".to_string(),
            supported_tools: vec![
                "code_execute".to_string(),
                "file_read".to_string(),
                "file_write".to_string(),
                "browser_screenshot".to_string(),
                "browser_click".to_string(),
            ],
            platform: "macos".to_string(),
            rte_enabled: true,
            max_concurrent_tools: 3,
        }
    }

    /// Web client with no RTE (backward compat test)
    pub fn web_no_rte() -> Self {
        Self {
            protocol_version: "1.0".to_string(),
            supported_tools: vec![],
            platform: "web".to_string(),
            rte_enabled: false,
            max_concurrent_tools: 0,
        }
    }

    /// Client that only supports code execution
    pub fn code_only() -> Self {
        Self {
            protocol_version: "1.0".to_string(),
            supported_tools: vec!["code_execute".to_string()],
            platform: "windows".to_string(),
            rte_enabled: true,
            max_concurrent_tools: 1,
        }
    }
}

/// Tool execution request from server to client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockToolExecuteRequest {
    pub request_id: Uuid,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub timeout_ms: u64,
    pub fallback: String,
    pub hmac_signature: String,
}

/// Tool result from client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockToolResult {
    pub request_id: Uuid,
    pub result: serde_json::Value,
    pub success: bool,
    pub error: Option<String>,
    pub execution_time_ms: u64,
    pub hmac_signature: String,
}

/// SSE event types for RTE
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum MockSseEvent {
    #[serde(rename = "session_start")]
    SessionStart {
        session_id: Uuid,
        session_secret: String,
    },
    #[serde(rename = "content_delta")]
    ContentDelta { delta: String },
    #[serde(rename = "tool_execute_request")]
    ToolExecuteRequest(MockToolExecuteRequest),
    #[serde(rename = "auth_refresh_required")]
    AuthRefreshRequired {
        expires_at: String,
        refresh_url: String,
    },
    #[serde(rename = "done")]
    Done { conversation_id: Uuid },
}

// ============================================================
// HMAC Utilities
// ============================================================

/// Compute HMAC-SHA256 for RTE request signing
pub fn compute_hmac(secret: &[u8], data: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
    mac.update(data.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// Verify HMAC signature in constant time
pub fn verify_hmac(secret: &[u8], data: &str, signature: &str) -> bool {
    let expected = compute_hmac(secret, data);
    constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

/// Constant-time byte comparison
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Sign a tool execute request
pub fn sign_tool_request(secret: &[u8], request_id: &Uuid, tool_name: &str) -> String {
    compute_hmac(secret, &format!("{}:{}", request_id, tool_name))
}

/// Sign a tool result
pub fn sign_tool_result(secret: &[u8], request_id: &Uuid, success: bool) -> String {
    compute_hmac(secret, &format!("{}:{}", request_id, success))
}

// ============================================================
// Mock RTE Client (simulates native client behavior)
// ============================================================

/// Simulates a native client handling RTE protocol
#[derive(Debug)]
pub struct MockRteClient {
    pub session_secret: Arc<RwLock<Option<Vec<u8>>>>,
    pub pending_requests: Arc<Mutex<Vec<MockToolExecuteRequest>>>,
    pub completed_results: Arc<Mutex<Vec<MockToolResult>>>,
    pub received_events: Arc<Mutex<Vec<MockSseEvent>>>,
    /// Configurable response behavior
    pub response_behavior: Arc<RwLock<RteResponseBehavior>>,
}

/// How the mock client responds to tool execution requests
#[derive(Debug, Clone)]
pub enum RteResponseBehavior {
    /// Execute immediately with success result
    ImmediateSuccess,
    /// Execute with a delay (simulates real tool execution)
    DelayedSuccess(Duration),
    /// Return an error result
    Error(String),
    /// Never respond (simulates timeout)
    Timeout,
    /// Respond with invalid HMAC
    InvalidHmac,
    /// Custom response per tool name
    Custom(HashMap<String, RteResponseBehavior>),
}

impl MockRteClient {
    pub fn new() -> Self {
        Self {
            session_secret: Arc::new(RwLock::new(None)),
            pending_requests: Arc::new(Mutex::new(Vec::new())),
            completed_results: Arc::new(Mutex::new(Vec::new())),
            received_events: Arc::new(Mutex::new(Vec::new())),
            response_behavior: Arc::new(RwLock::new(RteResponseBehavior::ImmediateSuccess)),
        }
    }

    /// Handle a session_start event
    pub async fn handle_session_start(&self, session_id: Uuid, session_secret: String) {
        let decoded = base64_decode(&session_secret);
        *self.session_secret.write().await = Some(decoded);
        self.received_events
            .lock()
            .await
            .push(MockSseEvent::SessionStart {
                session_id,
                session_secret,
            });
    }

    /// Handle a tool_execute_request event
    pub async fn handle_tool_request(
        &self,
        request: MockToolExecuteRequest,
    ) -> Option<MockToolResult> {
        self.pending_requests.lock().await.push(request.clone());
        self.received_events
            .lock()
            .await
            .push(MockSseEvent::ToolExecuteRequest(request.clone()));

        let secret = self.session_secret.read().await.clone()?;

        // Verify HMAC from server
        let expected_hmac = sign_tool_request(&secret, &request.request_id, &request.tool_name);
        if expected_hmac != request.hmac_signature {
            return None; // Invalid HMAC — reject
        }

        let behavior = self.response_behavior.read().await.clone();
        let effective_behavior = match &behavior {
            RteResponseBehavior::Custom(map) => map
                .get(&request.tool_name)
                .cloned()
                .unwrap_or(RteResponseBehavior::ImmediateSuccess),
            other => other.clone(),
        };

        match effective_behavior {
            RteResponseBehavior::ImmediateSuccess => {
                let result = MockToolResult {
                    request_id: request.request_id,
                    result: serde_json::json!({"output": "mock execution result", "exit_code": 0}),
                    success: true,
                    error: None,
                    execution_time_ms: 50,
                    hmac_signature: sign_tool_result(&secret, &request.request_id, true),
                };
                self.completed_results.lock().await.push(result.clone());
                Some(result)
            }
            RteResponseBehavior::DelayedSuccess(delay) => {
                tokio::time::sleep(delay).await;
                let result = MockToolResult {
                    request_id: request.request_id,
                    result: serde_json::json!({"output": "delayed result", "exit_code": 0}),
                    success: true,
                    error: None,
                    execution_time_ms: delay.as_millis() as u64,
                    hmac_signature: sign_tool_result(&secret, &request.request_id, true),
                };
                self.completed_results.lock().await.push(result.clone());
                Some(result)
            }
            RteResponseBehavior::Error(msg) => {
                let result = MockToolResult {
                    request_id: request.request_id,
                    result: serde_json::Value::Null,
                    success: false,
                    error: Some(msg),
                    execution_time_ms: 10,
                    hmac_signature: sign_tool_result(&secret, &request.request_id, false),
                };
                self.completed_results.lock().await.push(result.clone());
                Some(result)
            }
            RteResponseBehavior::Timeout => {
                // Never respond
                None
            }
            RteResponseBehavior::InvalidHmac => {
                let result = MockToolResult {
                    request_id: request.request_id,
                    result: serde_json::json!({"output": "result"}),
                    success: true,
                    error: None,
                    execution_time_ms: 50,
                    hmac_signature: "invalid-hmac-signature".to_string(),
                };
                Some(result)
            }
            RteResponseBehavior::Custom(_) => unreachable!(),
        }
    }

    /// Get count of received tool requests
    pub async fn request_count(&self) -> usize {
        self.pending_requests.lock().await.len()
    }

    /// Get count of completed results
    pub async fn result_count(&self) -> usize {
        self.completed_results.lock().await.len()
    }
}

fn base64_decode(input: &str) -> Vec<u8> {
    // Simple base64 decode for test secrets
    input.as_bytes().to_vec()
}

// ============================================================
// Mock Pending Tool Executions Store
// ============================================================

/// Simulates the server-side PendingToolExecutions store
#[derive(Debug)]
pub struct MockPendingStore {
    pub entries: Arc<dashmap::DashMap<Uuid, PendingEntry>>,
}

#[derive(Debug)]
pub struct PendingEntry {
    pub request_id: Uuid,
    pub tool_name: String,
    pub session_id: Uuid,
    pub created_at: Instant,
    pub timeout: Duration,
    pub resume_tx: Option<tokio::sync::oneshot::Sender<MockToolResult>>,
}

impl MockPendingStore {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(dashmap::DashMap::new()),
        }
    }

    /// Add a pending tool execution
    pub fn add(
        &self,
        request_id: Uuid,
        tool_name: String,
        session_id: Uuid,
        timeout: Duration,
    ) -> tokio::sync::oneshot::Receiver<MockToolResult> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.entries.insert(
            request_id,
            PendingEntry {
                request_id,
                tool_name,
                session_id,
                created_at: Instant::now(),
                timeout,
                resume_tx: Some(tx),
            },
        );
        rx
    }

    /// Complete a pending execution with result
    pub fn complete(&self, request_id: &Uuid, result: MockToolResult) -> bool {
        if let Some((_, mut entry)) = self.entries.remove(request_id) {
            if let Some(tx) = entry.resume_tx.take() {
                return tx.send(result).is_ok();
            }
        }
        false
    }

    /// Get count of pending executions
    pub fn pending_count(&self) -> usize {
        self.entries.len()
    }

    /// Evict expired entries
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

    /// Get all pending requests for a session (for reconnection)
    pub fn get_session_pending(&self, session_id: &Uuid) -> Vec<Uuid> {
        self.entries
            .iter()
            .filter(|e| &e.session_id == session_id)
            .map(|e| e.request_id)
            .collect()
    }
}

// ============================================================
// Test Assertion Helpers
// ============================================================

/// Assert that an SSE event stream contains expected event types in order
pub fn assert_event_sequence(events: &[MockSseEvent], expected: &[&str]) {
    let actual: Vec<&str> = events
        .iter()
        .map(|e| match e {
            MockSseEvent::SessionStart { .. } => "session_start",
            MockSseEvent::ContentDelta { .. } => "content_delta",
            MockSseEvent::ToolExecuteRequest(_) => "tool_execute_request",
            MockSseEvent::AuthRefreshRequired { .. } => "auth_refresh_required",
            MockSseEvent::Done { .. } => "done",
        })
        .collect();

    assert_eq!(
        actual.len(),
        expected.len(),
        "Event count mismatch: got {:?}, expected {:?}",
        actual,
        expected,
    );

    for (i, (got, want)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(
            got, want,
            "Event at index {} mismatch: got {}, expected {}",
            i, got, want,
        );
    }
}
