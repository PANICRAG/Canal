//! Clarification store for human-in-the-loop question/answer gate.
//!
//! Provides a thread-safe store for pending clarification requests using the
//! DashMap + oneshot channel pattern (same as `PendingPlanApprovals` in approval.rs).
//!
//! # Flow
//!
//! 1. ComplexityAssessor determines questions needed → `clarification_gate` calls `register()`
//! 2. SSE `clarification_required` event sent to client with questions
//! 3. Graph execution pauses on `oneshot::Receiver`
//! 4. User answers questions in frontend, POSTs to `/api/chat/clarification`
//! 5. Handler calls `complete()` → oneshot fires → graph resumes with answers

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use uuid::Uuid;

use super::prd::ClarificationResponse;

// ============================================================================
// Pending entry
// ============================================================================

/// Metadata about a pending clarification request awaiting user answers.
pub struct PendingClarification {
    /// Unique ID for this clarification request.
    pub request_id: Uuid,
    /// Session/conversation this request belongs to.
    pub session_id: Uuid,
    /// Summary of what the task is about.
    pub task_summary: String,
    /// When this clarification was registered.
    pub created_at: Instant,
    /// How long to wait before auto-skipping.
    pub timeout: Duration,
    /// Oneshot sender — consumed once when answers arrive.
    resume_tx: Option<oneshot::Sender<ClarificationResponse>>,
}

impl std::fmt::Debug for PendingClarification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingClarification")
            .field("request_id", &self.request_id)
            .field("session_id", &self.session_id)
            .field("task_summary", &self.task_summary)
            .field("created_at", &self.created_at)
            .field("timeout", &self.timeout)
            .field("has_sender", &self.resume_tx.is_some())
            .finish()
    }
}

// ============================================================================
// Store
// ============================================================================

/// Thread-safe store for pending clarification requests.
///
/// Reuses the DashMap + oneshot pattern from `PendingPlanApprovals`.
/// Each pending clarification holds a oneshot sender; when the user's answers
/// arrive via the HTTP endpoint, `complete()` fires the sender and
/// unblocks the graph node.
#[derive(Debug, Clone)]
pub struct PendingClarifications {
    entries: Arc<DashMap<Uuid, PendingClarification>>,
}

impl Default for PendingClarifications {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingClarifications {
    /// Create an empty store.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
        }
    }

    /// Register a new pending clarification request.
    ///
    /// Returns a `oneshot::Receiver` that the graph node awaits.
    /// The receiver resolves when `complete()` is called with a matching `request_id`.
    pub fn register(
        &self,
        request_id: Uuid,
        session_id: Uuid,
        task_summary: String,
        timeout: Duration,
    ) -> oneshot::Receiver<ClarificationResponse> {
        let (tx, rx) = oneshot::channel();
        self.entries.insert(
            request_id,
            PendingClarification {
                request_id,
                session_id,
                task_summary,
                created_at: Instant::now(),
                timeout,
                resume_tx: Some(tx),
            },
        );
        rx
    }

    /// Deliver user's answers to a pending clarification.
    ///
    /// Removes the entry and sends the response through the oneshot channel.
    /// Returns `Ok(())` on success, or an error string if the request was
    /// not found or the receiver was already dropped.
    pub fn complete(
        &self,
        request_id: &Uuid,
        response: ClarificationResponse,
    ) -> Result<(), String> {
        match self.entries.remove(request_id) {
            Some((_, mut entry)) => {
                if let Some(tx) = entry.resume_tx.take() {
                    tx.send(response)
                        .map_err(|_| format!("Receiver dropped for request {}", request_id))
                } else {
                    Err(format!("Already completed: {}", request_id))
                }
            }
            None => Err(format!("Request not found: {}", request_id)),
        }
    }

    /// Remove expired entries (those past `timeout * 2`).
    ///
    /// Returns the number of entries evicted.
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

    /// Number of currently pending clarifications.
    pub fn pending_count(&self) -> usize {
        self.entries.len()
    }

    /// Check if a specific request is pending.
    pub fn is_pending(&self, request_id: &Uuid) -> bool {
        self.entries.contains_key(request_id)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_register_and_complete() {
        let store = PendingClarifications::new();
        let req_id = Uuid::new_v4();
        let rx = store.register(
            req_id,
            Uuid::new_v4(),
            "test task".into(),
            Duration::from_secs(300),
        );
        assert_eq!(store.pending_count(), 1);
        assert!(store.is_pending(&req_id));

        let mut answers = HashMap::new();
        answers.insert(1, "yes".into());
        let response = ClarificationResponse {
            answers,
            skip_remaining: false,
        };

        store.complete(&req_id, response).unwrap();
        let result = rx.await.unwrap();
        assert_eq!(result.answers.get(&1), Some(&"yes".to_string()));
        assert_eq!(store.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_complete_with_skip() {
        let store = PendingClarifications::new();
        let req_id = Uuid::new_v4();
        let rx = store.register(
            req_id,
            Uuid::new_v4(),
            "test".into(),
            Duration::from_secs(300),
        );

        let response = ClarificationResponse {
            answers: HashMap::new(),
            skip_remaining: true,
        };

        store.complete(&req_id, response).unwrap();
        let result = rx.await.unwrap();
        assert!(result.skip_remaining);
        assert!(result.answers.is_empty());
    }

    #[tokio::test]
    async fn test_complete_not_found() {
        let store = PendingClarifications::new();
        let response = ClarificationResponse {
            answers: HashMap::new(),
            skip_remaining: false,
        };
        let result = store.complete(&Uuid::new_v4(), response);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_evict_expired() {
        let store = PendingClarifications::new();
        let req_id = Uuid::new_v4();
        let _rx = store.register(
            req_id,
            Uuid::new_v4(),
            "test".into(),
            Duration::from_millis(0),
        );
        assert_eq!(store.pending_count(), 1);

        tokio::time::sleep(Duration::from_millis(5)).await;
        let evicted = store.evict_expired();
        assert_eq!(evicted, 1);
        assert_eq!(store.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_timeout_receiver() {
        let store = PendingClarifications::new();
        let req_id = Uuid::new_v4();
        let rx = store.register(
            req_id,
            Uuid::new_v4(),
            "test".into(),
            Duration::from_millis(50),
        );

        let result = tokio::time::timeout(Duration::from_millis(100), rx).await;
        assert!(result.is_err()); // timeout
    }

    #[tokio::test]
    async fn test_double_complete() {
        let store = PendingClarifications::new();
        let req_id = Uuid::new_v4();
        let _rx = store.register(
            req_id,
            Uuid::new_v4(),
            "test".into(),
            Duration::from_secs(300),
        );

        let response = ClarificationResponse {
            answers: HashMap::new(),
            skip_remaining: false,
        };
        store.complete(&req_id, response.clone()).unwrap();

        let result = store.complete(&req_id, response);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }
}
