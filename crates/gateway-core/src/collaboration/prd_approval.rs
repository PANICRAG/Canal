//! PRD approval store for human-in-the-loop PRD review gate.
//!
//! Provides a thread-safe store for pending PRD approvals using the
//! DashMap + oneshot channel pattern (same as `PendingPlanApprovals` in approval.rs).
//!
//! # Flow
//!
//! 1. PrdAssembler generates PRD → `prd_approval_gate` calls `register()`
//! 2. SSE `prd_review_required` event sent to client with full PRD
//! 3. Graph execution pauses on `oneshot::Receiver`
//! 4. User reviews PRD, POSTs decision to `/api/chat/prd-approval`
//! 5. Handler calls `complete()` → oneshot fires → graph resumes
//!    - Approve → proceed to StepPlanner with chosen approach
//!    - Revise → loop back to PrdAssembler (max 3 rounds)
//!    - Reject → graph terminates

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use uuid::Uuid;

use super::prd::PrdApprovalDecision;

// ============================================================================
// Pending entry
// ============================================================================

/// Metadata about a pending PRD awaiting user approval.
pub struct PendingPrdApproval {
    /// Unique ID for this approval request.
    pub request_id: Uuid,
    /// Session/conversation this PRD belongs to.
    pub session_id: Uuid,
    /// PRD title for display.
    pub title: String,
    /// Current revision round (1-based).
    pub revision_round: u32,
    /// When this approval was registered.
    pub created_at: Instant,
    /// How long to wait before auto-rejecting.
    pub timeout: Duration,
    /// Oneshot sender — consumed once when decision arrives.
    resume_tx: Option<oneshot::Sender<PrdApprovalDecision>>,
}

impl std::fmt::Debug for PendingPrdApproval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingPrdApproval")
            .field("request_id", &self.request_id)
            .field("session_id", &self.session_id)
            .field("title", &self.title)
            .field("revision_round", &self.revision_round)
            .field("created_at", &self.created_at)
            .field("timeout", &self.timeout)
            .field("has_sender", &self.resume_tx.is_some())
            .finish()
    }
}

// ============================================================================
// Store
// ============================================================================

/// Thread-safe store for pending PRD approvals.
///
/// Reuses the DashMap + oneshot pattern from `PendingPlanApprovals`.
/// Each pending PRD approval holds a oneshot sender; when the user's decision
/// arrives via the HTTP endpoint, `complete()` fires the sender and
/// unblocks the graph node.
#[derive(Debug, Clone)]
pub struct PendingPrdApprovals {
    entries: Arc<DashMap<Uuid, PendingPrdApproval>>,
}

impl Default for PendingPrdApprovals {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingPrdApprovals {
    /// Create an empty store.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
        }
    }

    /// Register a new pending PRD approval.
    ///
    /// Returns a `oneshot::Receiver` that the graph node awaits.
    /// The receiver resolves when `complete()` is called with a matching `request_id`.
    pub fn register(
        &self,
        request_id: Uuid,
        session_id: Uuid,
        title: String,
        revision_round: u32,
        timeout: Duration,
    ) -> oneshot::Receiver<PrdApprovalDecision> {
        let (tx, rx) = oneshot::channel();
        self.entries.insert(
            request_id,
            PendingPrdApproval {
                request_id,
                session_id,
                title,
                revision_round,
                created_at: Instant::now(),
                timeout,
                resume_tx: Some(tx),
            },
        );
        rx
    }

    /// Deliver a user's decision to a pending PRD approval.
    ///
    /// Removes the entry and sends the decision through the oneshot channel.
    /// Returns `Ok(())` on success, or an error string if the request was
    /// not found or the receiver was already dropped.
    pub fn complete(&self, request_id: &Uuid, decision: PrdApprovalDecision) -> Result<(), String> {
        match self.entries.remove(request_id) {
            Some((_, mut entry)) => {
                if let Some(tx) = entry.resume_tx.take() {
                    tx.send(decision)
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

    /// Number of currently pending PRD approvals.
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

    #[tokio::test]
    async fn test_register_and_approve() {
        let store = PendingPrdApprovals::new();
        let req_id = Uuid::new_v4();
        let rx = store.register(
            req_id,
            Uuid::new_v4(),
            "Test PRD".into(),
            1,
            Duration::from_secs(300),
        );
        assert_eq!(store.pending_count(), 1);

        store
            .complete(&req_id, PrdApprovalDecision::Approve { chosen_approach: 0 })
            .unwrap();
        let decision = rx.await.unwrap();
        match decision {
            PrdApprovalDecision::Approve { chosen_approach } => assert_eq!(chosen_approach, 0),
            _ => panic!("Expected Approve"),
        }
        assert_eq!(store.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_register_and_revise() {
        let store = PendingPrdApprovals::new();
        let req_id = Uuid::new_v4();
        let rx = store.register(
            req_id,
            Uuid::new_v4(),
            "Test PRD".into(),
            1,
            Duration::from_secs(300),
        );

        store
            .complete(
                &req_id,
                PrdApprovalDecision::Revise {
                    feedback: "change design section".into(),
                },
            )
            .unwrap();
        let decision = rx.await.unwrap();
        match decision {
            PrdApprovalDecision::Revise { feedback } => {
                assert_eq!(feedback, "change design section");
            }
            _ => panic!("Expected Revise"),
        }
    }

    #[tokio::test]
    async fn test_register_and_reject() {
        let store = PendingPrdApprovals::new();
        let req_id = Uuid::new_v4();
        let rx = store.register(
            req_id,
            Uuid::new_v4(),
            "Test PRD".into(),
            1,
            Duration::from_secs(300),
        );

        store
            .complete(
                &req_id,
                PrdApprovalDecision::Reject {
                    reason: Some("wrong direction".into()),
                },
            )
            .unwrap();
        let decision = rx.await.unwrap();
        match decision {
            PrdApprovalDecision::Reject { reason } => {
                assert_eq!(reason, Some("wrong direction".into()));
            }
            _ => panic!("Expected Reject"),
        }
    }

    #[tokio::test]
    async fn test_complete_not_found() {
        let store = PendingPrdApprovals::new();
        let result = store.complete(
            &Uuid::new_v4(),
            PrdApprovalDecision::Approve { chosen_approach: 0 },
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_evict_expired() {
        let store = PendingPrdApprovals::new();
        let req_id = Uuid::new_v4();
        let _rx = store.register(
            req_id,
            Uuid::new_v4(),
            "Test".into(),
            1,
            Duration::from_millis(0),
        );
        assert_eq!(store.pending_count(), 1);

        tokio::time::sleep(Duration::from_millis(5)).await;
        let evicted = store.evict_expired();
        assert_eq!(evicted, 1);
        assert_eq!(store.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_double_complete() {
        let store = PendingPrdApprovals::new();
        let req_id = Uuid::new_v4();
        let _rx = store.register(
            req_id,
            Uuid::new_v4(),
            "Test".into(),
            1,
            Duration::from_secs(300),
        );

        store
            .complete(&req_id, PrdApprovalDecision::Approve { chosen_approach: 0 })
            .unwrap();
        let result = store.complete(&req_id, PrdApprovalDecision::Approve { chosen_approach: 0 });
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }
}
