//! RTE delegation logic and pending tool execution store.
//!
//! The `PendingToolExecutions` store tracks tool execution requests
//! that have been sent to native clients. When a client POSTs the
//! result back, the store resumes the waiting agent loop.

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use super::error::RteError;
use super::signing::RteSigner;
use super::types::{
    ClientCapabilities, FallbackStrategy, RteSseEvent, ToolExecuteRequest, ToolExecuteResult,
    ToolFallbackConfig,
};

/// A pending tool execution waiting for the client to respond.
#[derive(Debug)]
pub struct PendingExecution {
    /// Unique request ID
    pub request_id: Uuid,
    /// Tool being executed
    pub tool_name: String,
    /// Session this execution belongs to
    pub session_id: Uuid,
    /// When the request was created
    pub created_at: Instant,
    /// Timeout before fallback triggers
    pub timeout: Duration,
    /// Fallback strategy if client fails
    pub fallback: FallbackStrategy,
    /// Channel to send the result back to the waiting agent loop
    resume_tx: Option<oneshot::Sender<ToolExecuteResult>>,
}

/// Server-side store for pending RTE tool executions.
///
/// Thread-safe (DashMap) with TTL-based eviction.
/// Each entry holds a oneshot channel that the agent loop awaits.
#[derive(Debug, Clone)]
pub struct PendingToolExecutions {
    entries: Arc<DashMap<Uuid, PendingExecution>>,
}

impl PendingToolExecutions {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
        }
    }

    /// Register a new pending tool execution.
    ///
    /// Returns a receiver that the agent loop should `.await` on.
    /// When the client POSTs the result, `complete()` sends it through.
    pub fn register(
        &self,
        request_id: Uuid,
        tool_name: String,
        session_id: Uuid,
        timeout: Duration,
        fallback: FallbackStrategy,
    ) -> oneshot::Receiver<ToolExecuteResult> {
        let (tx, rx) = oneshot::channel();
        self.entries.insert(
            request_id,
            PendingExecution {
                request_id,
                tool_name,
                session_id,
                created_at: Instant::now(),
                timeout,
                fallback,
                resume_tx: Some(tx),
            },
        );
        rx
    }

    /// Complete a pending execution by sending the result to the waiting agent.
    ///
    /// Returns `Ok(())` if the result was delivered, or `Err` if the request
    /// was not found or the receiver was dropped.
    pub fn complete(&self, request_id: &Uuid, result: ToolExecuteResult) -> Result<(), RteError> {
        match self.entries.remove(request_id) {
            Some((_, mut entry)) => {
                if let Some(tx) = entry.resume_tx.take() {
                    tx.send(result)
                        .map_err(|_| RteError::ClientDisconnected(*request_id))
                } else {
                    Err(RteError::ClientDisconnected(*request_id))
                }
            }
            None => Err(RteError::RequestNotFound(*request_id)),
        }
    }

    /// Get the number of pending executions.
    pub fn pending_count(&self) -> usize {
        self.entries.len()
    }

    /// Evict expired entries (timeout * 2 TTL).
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

    /// Get all pending request IDs for a session (for reconnection resume).
    pub fn get_session_pending(&self, session_id: &Uuid) -> Vec<Uuid> {
        self.entries
            .iter()
            .filter(|e| &e.session_id == session_id)
            .map(|e| e.request_id)
            .collect()
    }

    /// Check if a request is still pending.
    pub fn is_pending(&self, request_id: &Uuid) -> bool {
        self.entries.contains_key(request_id)
    }

    /// Get the fallback strategy for a pending request.
    pub fn get_fallback(&self, request_id: &Uuid) -> Option<FallbackStrategy> {
        self.entries.get(request_id).map(|e| e.fallback.clone())
    }
}

impl Default for PendingToolExecutions {
    fn default() -> Self {
        Self::new()
    }
}

/// Determines whether a tool call should be delegated to the native client.
///
/// Decision logic:
/// 1. Client must have RTE enabled
/// 2. Client must support the specific tool
/// 3. Tool must not be in the "always-cloud" list
pub fn should_delegate(
    capabilities: &ClientCapabilities,
    tool_name: &str,
    _fallback_config: &ToolFallbackConfig,
) -> bool {
    capabilities.supports_tool(tool_name)
}

/// Build a `ToolExecuteRequest` for sending to the client.
pub fn build_tool_request(
    signer: &RteSigner,
    tool_name: &str,
    tool_input: serde_json::Value,
    fallback_config: &ToolFallbackConfig,
) -> ToolExecuteRequest {
    let request_id = Uuid::new_v4();
    let timeout_ms = fallback_config.timeout_for(tool_name);
    let fallback = fallback_config.fallback_for(tool_name);
    let hmac_signature = signer.sign_request(&request_id, tool_name);

    ToolExecuteRequest {
        request_id,
        tool_name: tool_name.to_string(),
        tool_input,
        timeout_ms,
        fallback,
        hmac_signature,
    }
}

/// Context for RTE delegation within the agent loop.
///
/// Bundles all the components needed to delegate a tool call to a native client:
/// - Client capabilities (what tools the client supports)
/// - SSE event sender (to send `tool_execute_request` events)
/// - Pending executions store (to register and await results)
/// - Signer (for HMAC signatures)
/// - Fallback config (timeouts and fallback strategies)
///
/// Created per-session and passed to the `AgentRunner`.
#[derive(Clone)]
pub struct RteDelegationContext {
    /// Client capabilities from the initial StreamChatRequest
    pub capabilities: ClientCapabilities,
    /// Session ID for tracking pending executions
    pub session_id: Uuid,
    /// Channel to send SSE events to the streaming response
    pub sse_tx: mpsc::Sender<RteSseEvent>,
    /// Server-wide pending tool execution store
    pub pending: Arc<PendingToolExecutions>,
    /// HMAC signer for this session
    pub signer: RteSigner,
    /// Fallback configuration
    pub fallback_config: Arc<ToolFallbackConfig>,
}

/// Result of attempting RTE delegation.
pub enum DelegationResult {
    /// Tool was delegated and the client returned a result
    Completed(ToolExecuteResult),
    /// Tool was delegated but timed out — includes fallback strategy
    TimedOut(FallbackStrategy),
    /// Tool was not eligible for delegation (run locally)
    NotDelegated,
}

impl RteDelegationContext {
    /// Attempt to delegate a tool call to the native client.
    ///
    /// Returns `DelegationResult::Completed` if the client executed the tool,
    /// `DelegationResult::TimedOut` if the client didn't respond in time,
    /// or `DelegationResult::NotDelegated` if the tool shouldn't be delegated.
    pub async fn try_delegate(
        &self,
        tool_name: &str,
        tool_input: serde_json::Value,
    ) -> DelegationResult {
        // Check if this tool should be delegated
        if !should_delegate(&self.capabilities, tool_name, &self.fallback_config) {
            return DelegationResult::NotDelegated;
        }

        // Build the tool request
        let request =
            build_tool_request(&self.signer, tool_name, tool_input, &self.fallback_config);
        let request_id = request.request_id;
        let timeout_ms = request.timeout_ms;
        let fallback = request.fallback.clone();

        // Register the pending execution (get a receiver to await)
        let rx = self.pending.register(
            request_id,
            tool_name.to_string(),
            self.session_id,
            Duration::from_millis(timeout_ms),
            fallback.clone(),
        );

        // Send the tool execute request to the client via SSE
        let event = RteSseEvent::ToolExecuteRequest(request);
        if self.sse_tx.send(event).await.is_err() {
            tracing::warn!(tool_name = %tool_name, "SSE channel closed, cannot delegate tool");
            // Remove the pending entry since we can't send the request
            self.pending.entries.remove(&request_id);
            return DelegationResult::NotDelegated;
        }

        tracing::info!(
            tool_name = %tool_name,
            request_id = %request_id,
            timeout_ms = timeout_ms,
            "RTE: delegated tool execution to native client"
        );

        // Wait for the client to respond (with timeout)
        match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
            Ok(Ok(result)) => {
                tracing::info!(
                    tool_name = %tool_name,
                    request_id = %request_id,
                    success = result.success,
                    execution_time_ms = result.execution_time_ms,
                    "RTE: received tool result from client"
                );
                DelegationResult::Completed(result)
            }
            Ok(Err(_)) => {
                // Sender dropped (client disconnected)
                tracing::warn!(tool_name = %tool_name, "RTE: client disconnected before responding");
                DelegationResult::TimedOut(fallback)
            }
            Err(_) => {
                // Timeout
                tracing::warn!(
                    tool_name = %tool_name,
                    request_id = %request_id,
                    timeout_ms = timeout_ms,
                    "RTE: tool execution timed out"
                );
                // Clean up the pending entry
                self.pending.entries.remove(&request_id);
                DelegationResult::TimedOut(fallback)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_complete() {
        let store = PendingToolExecutions::new();
        let request_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();

        let rx = store.register(
            request_id,
            "code_execute".to_string(),
            session_id,
            Duration::from_secs(30),
            FallbackStrategy::CloudExecution,
        );

        assert_eq!(store.pending_count(), 1);

        let result = ToolExecuteResult {
            request_id,
            result: serde_json::json!({"output": "hello"}),
            success: true,
            error: None,
            execution_time_ms: 100,
            hmac_signature: "test".to_string(),
        };

        store.complete(&request_id, result).unwrap();
        assert_eq!(store.pending_count(), 0);

        let received = rx.await.unwrap();
        assert!(received.success);
    }

    #[tokio::test]
    async fn test_complete_not_found() {
        let store = PendingToolExecutions::new();
        let request_id = Uuid::new_v4();

        let result = ToolExecuteResult {
            request_id,
            result: serde_json::Value::Null,
            success: false,
            error: Some("test".to_string()),
            execution_time_ms: 0,
            hmac_signature: "test".to_string(),
        };

        let err = store.complete(&request_id, result).unwrap_err();
        assert!(matches!(err, RteError::RequestNotFound(_)));
    }

    #[test]
    fn test_should_delegate_rte_capable() {
        let caps = ClientCapabilities {
            protocol_version: "1.0".to_string(),
            supported_tools: vec!["code_execute".to_string()],
            platform: "windows".to_string(),
            rte_enabled: true,
            max_concurrent_tools: 3,
        };
        let config = ToolFallbackConfig::default();

        assert!(should_delegate(&caps, "code_execute", &config));
        assert!(!should_delegate(&caps, "file_read", &config));
    }

    #[test]
    fn test_should_delegate_rte_disabled() {
        let caps = ClientCapabilities {
            protocol_version: "1.0".to_string(),
            supported_tools: vec!["code_execute".to_string()],
            platform: "web".to_string(),
            rte_enabled: false,
            max_concurrent_tools: 0,
        };
        let config = ToolFallbackConfig::default();

        assert!(!should_delegate(&caps, "code_execute", &config));
    }

    #[test]
    fn test_build_tool_request() {
        let signer = RteSigner::new(b"test-secret".to_vec());
        let config = ToolFallbackConfig::default();

        let req = build_tool_request(
            &signer,
            "code_execute",
            serde_json::json!({"code": "print('hello')"}),
            &config,
        );

        assert_eq!(req.tool_name, "code_execute");
        assert_eq!(req.timeout_ms, 30_000);
        assert!(!req.hmac_signature.is_empty());
        assert!(signer.verify_request(&req.request_id, "code_execute", &req.hmac_signature));
    }

    #[test]
    fn test_evict_expired() {
        let store = PendingToolExecutions::new();
        let session_id = Uuid::new_v4();

        // Register with a very short timeout
        let _rx = store.register(
            Uuid::new_v4(),
            "tool".to_string(),
            session_id,
            Duration::from_millis(1),
            FallbackStrategy::Error,
        );

        // Wait for expiry (timeout * 2 = 2ms)
        std::thread::sleep(Duration::from_millis(5));

        let evicted = store.evict_expired();
        assert_eq!(evicted, 1);
        assert_eq!(store.pending_count(), 0);
    }

    #[test]
    fn test_get_session_pending() {
        let store = PendingToolExecutions::new();
        let session_1 = Uuid::new_v4();
        let session_2 = Uuid::new_v4();
        let req_1 = Uuid::new_v4();
        let req_2 = Uuid::new_v4();
        let req_3 = Uuid::new_v4();

        let _rx1 = store.register(
            req_1,
            "t1".to_string(),
            session_1,
            Duration::from_secs(30),
            FallbackStrategy::Error,
        );
        let _rx2 = store.register(
            req_2,
            "t2".to_string(),
            session_1,
            Duration::from_secs(30),
            FallbackStrategy::Error,
        );
        let _rx3 = store.register(
            req_3,
            "t3".to_string(),
            session_2,
            Duration::from_secs(30),
            FallbackStrategy::Error,
        );

        let pending = store.get_session_pending(&session_1);
        assert_eq!(pending.len(), 2);
        assert!(pending.contains(&req_1));
        assert!(pending.contains(&req_2));
    }

    #[tokio::test]
    async fn test_delegation_not_delegated() {
        let (sse_tx, _sse_rx) = mpsc::channel(16);
        let signer = RteSigner::new(b"test-secret".to_vec());
        let ctx = RteDelegationContext {
            capabilities: ClientCapabilities {
                protocol_version: "1.0".to_string(),
                supported_tools: vec!["code_execute".to_string()],
                platform: "windows".to_string(),
                rte_enabled: true,
                max_concurrent_tools: 3,
            },
            session_id: Uuid::new_v4(),
            sse_tx,
            pending: Arc::new(PendingToolExecutions::new()),
            signer,
            fallback_config: Arc::new(ToolFallbackConfig::default()),
        };

        // Tool not in supported_tools → NotDelegated
        let result = ctx.try_delegate("file_read", serde_json::json!({})).await;
        assert!(matches!(result, DelegationResult::NotDelegated));
    }

    #[tokio::test]
    async fn test_delegation_completed() {
        let (sse_tx, mut sse_rx) = mpsc::channel(16);
        let signer = RteSigner::new(b"test-secret".to_vec());
        let pending = Arc::new(PendingToolExecutions::new());

        let ctx = RteDelegationContext {
            capabilities: ClientCapabilities {
                protocol_version: "1.0".to_string(),
                supported_tools: vec!["code_execute".to_string()],
                platform: "windows".to_string(),
                rte_enabled: true,
                max_concurrent_tools: 3,
            },
            session_id: Uuid::new_v4(),
            sse_tx,
            pending: pending.clone(),
            signer,
            fallback_config: Arc::new(ToolFallbackConfig::default()),
        };

        // Spawn task to simulate client completing the tool
        let pending_clone = pending.clone();
        tokio::spawn(async move {
            // Receive the SSE event
            let event = sse_rx.recv().await.unwrap();
            if let RteSseEvent::ToolExecuteRequest(req) = event {
                // Simulate client completing
                let result = ToolExecuteResult {
                    request_id: req.request_id,
                    result: serde_json::json!({"output": "executed"}),
                    success: true,
                    error: None,
                    execution_time_ms: 50,
                    hmac_signature: "sig".to_string(),
                };
                pending_clone.complete(&req.request_id, result).unwrap();
            }
        });

        let result = ctx
            .try_delegate("code_execute", serde_json::json!({"code": "test"}))
            .await;
        match result {
            DelegationResult::Completed(r) => {
                assert!(r.success);
                assert_eq!(r.result, serde_json::json!({"output": "executed"}));
            }
            _ => panic!(
                "Expected Completed, got {:?}",
                std::mem::discriminant(&result)
            ),
        }
    }

    #[tokio::test]
    async fn test_delegation_timeout() {
        let (sse_tx, _sse_rx) = mpsc::channel(16);
        let signer = RteSigner::new(b"test-secret".to_vec());

        let mut config = ToolFallbackConfig::default();
        config.default_timeout_ms = 50; // Very short timeout for test

        let ctx = RteDelegationContext {
            capabilities: ClientCapabilities {
                protocol_version: "1.0".to_string(),
                supported_tools: vec!["code_execute".to_string()],
                platform: "windows".to_string(),
                rte_enabled: true,
                max_concurrent_tools: 3,
            },
            session_id: Uuid::new_v4(),
            sse_tx,
            pending: Arc::new(PendingToolExecutions::new()),
            signer,
            fallback_config: Arc::new(config),
        };

        // Don't simulate client response → timeout
        let result = ctx
            .try_delegate("code_execute", serde_json::json!({}))
            .await;
        assert!(matches!(
            result,
            DelegationResult::TimedOut(FallbackStrategy::CloudExecution)
        ));
    }
}
