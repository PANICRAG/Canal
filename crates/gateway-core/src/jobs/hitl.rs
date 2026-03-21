//! Human-in-the-loop (HITL) input system for async jobs.
//!
//! When a running job needs user input (text, choice, or confirmation),
//! it registers a pending input request via `PendingHITLInputs`. The
//! frontend receives a `hitl_input_required` SSE event and prompts the
//! user. The user's response is submitted via the `/api/jobs/:id/input`
//! endpoint, which calls `complete()` to deliver the value back to the
//! waiting job task.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use uuid::Uuid;

/// Type of HITL input requested from the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HITLInputType {
    /// Free-form text input.
    Text,
    /// Multiple-choice selection.
    Choice(Vec<String>),
    /// Yes/no confirmation.
    Confirmation,
}

/// Response from the user to a HITL input request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HITLResponse {
    /// The user's response value.
    pub value: String,
    /// Optional metadata about the response.
    pub metadata: Option<serde_json::Value>,
}

/// A pending HITL input request waiting for user response.
struct PendingInput {
    request_id: Uuid,
    job_id: Uuid,
    prompt: String,
    input_type: HITLInputType,
    created_at: Instant,
    timeout: Duration,
    resume_tx: Option<oneshot::Sender<HITLResponse>>,
}

/// Thread-safe store of pending HITL input requests.
///
/// Uses `DashMap` for concurrent access from multiple job tasks and
/// the HTTP handler that delivers user responses.
pub struct PendingHITLInputs {
    entries: Arc<DashMap<Uuid, PendingInput>>,
}

impl PendingHITLInputs {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
        }
    }

    /// Register a new HITL input request and return a receiver for the response.
    ///
    /// The calling job task should `.await` on the returned receiver.
    pub fn register(
        &self,
        request_id: Uuid,
        job_id: Uuid,
        prompt: &str,
        input_type: HITLInputType,
        timeout: Duration,
    ) -> oneshot::Receiver<HITLResponse> {
        let (tx, rx) = oneshot::channel();
        self.entries.insert(
            request_id,
            PendingInput {
                request_id,
                job_id,
                prompt: prompt.to_string(),
                input_type,
                created_at: Instant::now(),
                timeout,
                resume_tx: Some(tx),
            },
        );
        rx
    }

    /// Complete a pending HITL request with the user's response.
    ///
    /// Returns `Ok(())` if the response was delivered, or `Err` if the
    /// request was not found (already completed, expired, or invalid ID).
    pub fn complete(&self, request_id: &Uuid, response: HITLResponse) -> Result<(), String> {
        match self.entries.remove(request_id) {
            Some((_, mut entry)) => {
                if let Some(tx) = entry.resume_tx.take() {
                    tx.send(response)
                        .map_err(|_| "Job task already dropped the receiver".to_string())
                } else {
                    Err("Request already completed".to_string())
                }
            }
            None => Err(format!("HITL request {} not found", request_id)),
        }
    }

    /// Evict expired entries and return the count of evicted entries.
    pub fn evict_expired(&self) -> usize {
        let now = Instant::now();
        let mut evicted = 0;
        self.entries.retain(|_, entry| {
            let expired = now.duration_since(entry.created_at) > entry.timeout;
            if expired {
                evicted += 1;
            }
            !expired
        });
        evicted
    }

    /// Get the number of pending requests.
    pub fn pending_count(&self) -> usize {
        self.entries.len()
    }

    /// Get the job ID for a pending request.
    pub fn get_job_id(&self, request_id: &Uuid) -> Option<Uuid> {
        self.entries.get(request_id).map(|e| e.job_id)
    }
}

impl Default for PendingHITLInputs {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for a HITL input request.
///
/// Used by graph nodes to describe what input they need from the user.
#[derive(Debug, Clone)]
pub struct HITLRequest {
    /// Human-readable prompt describing what input is needed.
    pub prompt: String,
    /// Type of input expected.
    pub input_type: HITLInputType,
    /// Timeout before auto-expiry.
    pub timeout: Duration,
    /// Additional context for the user (e.g., what the agent was doing).
    pub context: Option<String>,
}

/// Outcome of a HITL input request.
#[derive(Debug, Clone)]
pub enum HITLOutcome {
    /// User provided a response.
    Response(HITLResponse),
    /// Request timed out waiting for user input.
    Timeout,
    /// The receiver was dropped (job cancelled or task aborted).
    Cancelled,
}

/// Request human input during graph execution.
///
/// This is the primary entry point for graph nodes that need to pause and
/// wait for user input. It performs three coordinated actions:
///
/// 1. Registers the pending request in `PendingHITLInputs` (so the HTTP
///    handler can deliver the response).
/// 2. Sends an `HITLInputRequired` event via the SSE stream channel (so
///    the frontend knows to prompt the user).
/// 3. Records the event in the `ExecutionStore` (so it appears in the
///    job's event history / replay).
/// 4. Awaits the user's response with the configured timeout.
///
/// # Arguments
///
/// * `pending` - The shared HITL input store.
/// * `execution_id` - Current graph execution ID.
/// * `job_id` - Job UUID (for the frontend to POST the response).
/// * `request` - Description of the input needed.
/// * `stream_tx` - Optional SSE stream sender (from `StreamingObserver::sender()`).
/// * `execution_store` - Optional execution store for recording the event.
///
/// # Returns
///
/// An `HITLOutcome` indicating whether the user responded, the request timed
/// out, or the request was cancelled.
///
/// # Example
///
/// ```ignore
/// use gateway_core::jobs::hitl::*;
/// use std::time::Duration;
///
/// let outcome = request_human_input(
///     &pending_hitl_inputs,
///     "exec_123",
///     job_id,
///     HITLRequest {
///         prompt: "Which deployment target?".into(),
///         input_type: HITLInputType::Choice(vec!["staging".into(), "production".into()]),
///         timeout: Duration::from_secs(300),
///         context: Some("The agent needs to know where to deploy.".into()),
///     },
///     Some(stream_tx.clone()),
///     Some(execution_store.clone()),
/// ).await;
///
/// match outcome {
///     HITLOutcome::Response(resp) => println!("User chose: {}", resp.value),
///     HITLOutcome::Timeout => println!("No response, using default"),
///     HITLOutcome::Cancelled => println!("Job was cancelled"),
/// }
/// ```
#[tracing::instrument(skip(pending, stream_tx, execution_store), fields(request_id))]
pub async fn request_human_input(
    pending: &PendingHITLInputs,
    execution_id: &str,
    job_id: Uuid,
    request: HITLRequest,
    stream_tx: Option<tokio::sync::mpsc::Sender<crate::graph::GraphStreamEvent>>,
    execution_store: Option<Arc<crate::graph::ExecutionStore>>,
) -> HITLOutcome {
    let request_id = Uuid::new_v4();
    tracing::Span::current().record("request_id", tracing::field::display(&request_id));

    // Derive the input_type string and options for the SSE event.
    let (input_type_str, options) = match &request.input_type {
        HITLInputType::Text => ("text".to_string(), None),
        HITLInputType::Choice(opts) => ("choice".to_string(), Some(opts.clone())),
        HITLInputType::Confirmation => ("confirmation".to_string(), None),
    };
    let timeout_seconds = Some(request.timeout.as_secs());

    // 1. Register the pending request (creates the oneshot channel).
    let rx = pending.register(
        request_id,
        job_id,
        &request.prompt,
        request.input_type.clone(),
        request.timeout,
    );

    tracing::info!(
        %execution_id,
        %job_id,
        %request_id,
        prompt = %request.prompt,
        input_type = %input_type_str,
        "HITL input requested — waiting for user response"
    );

    // 2. Send the SSE event so the frontend can prompt the user.
    if let Some(tx) = &stream_tx {
        let event = crate::graph::GraphStreamEvent::HITLInputRequired {
            execution_id: execution_id.to_string(),
            request_id: request_id.to_string(),
            job_id: job_id.to_string(),
            prompt: request.prompt.clone(),
            input_type: input_type_str.clone(),
            options: options.clone(),
            timeout_seconds,
            context: request.context.clone(),
        };
        if let Err(e) = tx.try_send(event) {
            tracing::warn!("Failed to send HITL SSE event: {}", e);
        }
    }

    // 3. Record the event in the execution store for replay.
    if let Some(store) = &execution_store {
        store
            .append_event(
                execution_id,
                crate::graph::execution_store::EventPayload::HITLInputRequired {
                    request_id: request_id.to_string(),
                    job_id: job_id.to_string(),
                    prompt: request.prompt.clone(),
                    input_type: input_type_str,
                    options,
                    timeout_seconds,
                    context: request.context,
                },
            )
            .await;
    }

    // 4. Wait for user response with timeout.
    match tokio::time::timeout(request.timeout, rx).await {
        Ok(Ok(response)) => {
            tracing::info!(
                %request_id,
                value = %response.value,
                "HITL input received from user"
            );
            HITLOutcome::Response(response)
        }
        Ok(Err(_)) => {
            // Oneshot sender was dropped (job cancelled or task aborted).
            tracing::warn!(%request_id, "HITL request cancelled (sender dropped)");
            HITLOutcome::Cancelled
        }
        Err(_) => {
            // Timeout expired. Clean up the pending entry.
            tracing::warn!(%request_id, "HITL request timed out");
            // The evict_expired sweep will also catch this, but do it eagerly.
            pending.entries.remove(&request_id);
            HITLOutcome::Timeout
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_complete_hitl_input() {
        let store = PendingHITLInputs::new();
        let req_id = Uuid::new_v4();
        let job_id = Uuid::new_v4();
        let rx = store.register(
            req_id,
            job_id,
            "What color?",
            HITLInputType::Text,
            Duration::from_secs(30),
        );
        store
            .complete(
                &req_id,
                HITLResponse {
                    value: "blue".into(),
                    metadata: None,
                },
            )
            .unwrap();
        let resp = rx.await.unwrap();
        assert_eq!(resp.value, "blue");
    }

    #[tokio::test]
    async fn test_complete_unknown_request_returns_error() {
        let store = PendingHITLInputs::new();
        let result = store.complete(
            &Uuid::new_v4(),
            HITLResponse {
                value: "x".into(),
                metadata: None,
            },
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_double_complete_returns_error() {
        let store = PendingHITLInputs::new();
        let req_id = Uuid::new_v4();
        let _rx = store.register(
            req_id,
            Uuid::new_v4(),
            "p",
            HITLInputType::Text,
            Duration::from_secs(5),
        );
        store
            .complete(
                &req_id,
                HITLResponse {
                    value: "a".into(),
                    metadata: None,
                },
            )
            .unwrap();
        let result = store.complete(
            &req_id,
            HITLResponse {
                value: "b".into(),
                metadata: None,
            },
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_evict_expired_removes_old_entries() {
        let store = PendingHITLInputs::new();
        let _rx = store.register(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "p",
            HITLInputType::Text,
            Duration::from_millis(1),
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        let evicted = store.evict_expired();
        assert_eq!(evicted, 1);
        assert_eq!(store.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_evict_keeps_non_expired() {
        let store = PendingHITLInputs::new();
        let _rx = store.register(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "p",
            HITLInputType::Text,
            Duration::from_secs(60),
        );
        let evicted = store.evict_expired();
        assert_eq!(evicted, 0);
        assert_eq!(store.pending_count(), 1);
    }

    #[test]
    fn test_hitl_input_type_variants() {
        assert!(matches!(HITLInputType::Text, HITLInputType::Text));
        let choice = HITLInputType::Choice(vec!["a".into()]);
        assert!(matches!(choice, HITLInputType::Choice(_)));
        assert!(matches!(
            HITLInputType::Confirmation,
            HITLInputType::Confirmation
        ));
    }

    #[tokio::test]
    async fn test_concurrent_register_complete() {
        let store = Arc::new(PendingHITLInputs::new());
        let mut handles = vec![];
        for _ in 0..10 {
            let s = store.clone();
            handles.push(tokio::spawn(async move {
                let req_id = Uuid::new_v4();
                let rx = s.register(
                    req_id,
                    Uuid::new_v4(),
                    "q",
                    HITLInputType::Text,
                    Duration::from_secs(5),
                );
                s.complete(
                    &req_id,
                    HITLResponse {
                        value: "ok".into(),
                        metadata: None,
                    },
                )
                .unwrap();
                let resp = rx.await.unwrap();
                assert_eq!(resp.value, "ok");
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    }

    #[test]
    fn test_get_job_id() {
        let store = PendingHITLInputs::new();
        let req_id = Uuid::new_v4();
        let job_id = Uuid::new_v4();
        let _rx = store.register(
            req_id,
            job_id,
            "q",
            HITLInputType::Text,
            Duration::from_secs(5),
        );
        assert_eq!(store.get_job_id(&req_id), Some(job_id));
        assert_eq!(store.get_job_id(&Uuid::new_v4()), None);
    }

    #[test]
    fn test_default_impl() {
        let store = PendingHITLInputs::default();
        assert_eq!(store.pending_count(), 0);
    }
}
