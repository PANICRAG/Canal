//! Plan approval store for human-in-the-loop PlanExecute mode.
//!
//! Provides a thread-safe store for pending plan approvals using the
//! DashMap + oneshot channel pattern (same as `PendingToolExecutions`).
//!
//! # Flow
//!
//! 1. Planner generates plan → `approval_gate` node calls `register()`
//! 2. SSE `plan_approval_required` event sent to client
//! 3. Graph execution pauses on `oneshot::Receiver`
//! 4. User reviews plan in frontend, POSTs decision to `/api/chat/plan-approval`
//! 5. Handler calls `complete()` → oneshot fires → graph resumes

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use uuid::Uuid;

use super::planner::{PlanStep, ToolCategory};

// ============================================================================
// Decision types
// ============================================================================

/// User's decision on a proposed execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum PlanApprovalDecision {
    /// Approve the plan as-is — proceed to execution.
    Approve,
    /// Approve with user-edited steps — update plan, then execute.
    ApproveWithEdits { edited_steps: Vec<PlanStep> },
    /// Send feedback to re-generate the plan — planner runs again.
    Revise { feedback: String },
    /// Reject the plan entirely — graph terminates.
    Reject { reason: Option<String> },
}

/// Enriched step info sent to frontend for review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepReview {
    /// Step identifier.
    pub id: u32,
    /// Concrete action to perform.
    pub action: String,
    /// Tool category (Browser, Shell, LLM, etc.).
    pub tool_category: String,
    /// Dependency type (Sequential, Parallel, None).
    pub dependency: String,
    /// What success looks like for this step.
    pub expected_output: Option<String>,
    /// Risk level: "low", "medium", "high".
    pub risk_level: String,
    /// Which model will execute this step (if known).
    pub estimated_model: Option<String>,
}

impl PlanStepReview {
    /// Create a review step from a `PlanStep` with auto-classified risk.
    pub fn from_plan_step(step: &PlanStep, estimated_model: Option<String>) -> Self {
        let risk_level = classify_risk(&step.tool_category);
        Self {
            id: step.id,
            action: step.action.clone(),
            tool_category: step.tool_category.to_string(),
            dependency: format!("{:?}", step.dependency),
            expected_output: step.expected_output.clone(),
            risk_level,
            estimated_model,
        }
    }
}

/// Classify risk level based on tool category.
pub fn classify_risk(category: &ToolCategory) -> String {
    match category {
        ToolCategory::Shell | ToolCategory::Browser => "high".into(),
        ToolCategory::File | ToolCategory::Code => "medium".into(),
        ToolCategory::Llm | ToolCategory::Search => "low".into(),
    }
}

/// Compute the maximum risk level across a set of steps.
pub fn max_risk_level(steps: &[PlanStep]) -> String {
    let mut has_high = false;
    let mut has_medium = false;
    for step in steps {
        match classify_risk(&step.tool_category).as_str() {
            "high" => has_high = true,
            "medium" => has_medium = true,
            _ => {}
        }
    }
    if has_high {
        "high".into()
    } else if has_medium {
        "medium".into()
    } else {
        "low".into()
    }
}

// ============================================================================
// Pending approval entry
// ============================================================================

/// Metadata about a pending plan awaiting user approval.
pub struct PendingApproval {
    /// Unique ID for this approval request.
    pub request_id: Uuid,
    /// Session/conversation this plan belongs to.
    pub session_id: Uuid,
    /// High-level goal of the plan.
    pub goal: String,
    /// When this approval was registered.
    pub created_at: Instant,
    /// How long to wait before auto-rejecting.
    pub timeout: Duration,
    /// Oneshot sender — consumed once when decision arrives.
    resume_tx: Option<oneshot::Sender<PlanApprovalDecision>>,
}

impl std::fmt::Debug for PendingApproval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingApproval")
            .field("request_id", &self.request_id)
            .field("session_id", &self.session_id)
            .field("goal", &self.goal)
            .field("created_at", &self.created_at)
            .field("timeout", &self.timeout)
            .field("has_sender", &self.resume_tx.is_some())
            .finish()
    }
}

// ============================================================================
// Store
// ============================================================================

/// Thread-safe store for pending plan approvals.
///
/// Reuses the DashMap + oneshot pattern from `PendingToolExecutions`.
/// Each pending approval holds a oneshot sender; when the user's decision
/// arrives via the HTTP endpoint, `complete()` fires the sender and
/// unblocks the graph node.
#[derive(Debug, Clone)]
pub struct PendingPlanApprovals {
    entries: Arc<DashMap<Uuid, PendingApproval>>,
}

impl Default for PendingPlanApprovals {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingPlanApprovals {
    /// Create an empty store.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
        }
    }

    /// Register a new pending plan approval.
    ///
    /// Returns a `oneshot::Receiver` that the graph node awaits.
    /// The receiver resolves when `complete()` is called with a matching `request_id`.
    pub fn register(
        &self,
        request_id: Uuid,
        session_id: Uuid,
        goal: String,
        timeout: Duration,
    ) -> oneshot::Receiver<PlanApprovalDecision> {
        let (tx, rx) = oneshot::channel();
        self.entries.insert(
            request_id,
            PendingApproval {
                request_id,
                session_id,
                goal,
                created_at: Instant::now(),
                timeout,
                resume_tx: Some(tx),
            },
        );
        rx
    }

    /// Deliver a user's decision to a pending approval.
    ///
    /// Removes the entry and sends the decision through the oneshot channel.
    /// Returns `Ok(())` on success, or an error string if the request was
    /// not found or the receiver was already dropped.
    pub fn complete(
        &self,
        request_id: &Uuid,
        decision: PlanApprovalDecision,
    ) -> Result<(), String> {
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

    /// Number of currently pending approvals.
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
    use std::time::Duration;

    #[tokio::test]
    async fn test_register_and_approve() {
        let store = PendingPlanApprovals::new();
        let req_id = Uuid::new_v4();
        let rx = store.register(
            req_id,
            Uuid::new_v4(),
            "test goal".into(),
            Duration::from_secs(300),
        );
        assert_eq!(store.pending_count(), 1);

        store
            .complete(&req_id, PlanApprovalDecision::Approve)
            .unwrap();
        let decision = rx.await.unwrap();
        assert!(matches!(decision, PlanApprovalDecision::Approve));
        assert_eq!(store.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_register_and_reject() {
        let store = PendingPlanApprovals::new();
        let req_id = Uuid::new_v4();
        let rx = store.register(
            req_id,
            Uuid::new_v4(),
            "test goal".into(),
            Duration::from_secs(300),
        );

        store
            .complete(
                &req_id,
                PlanApprovalDecision::Reject {
                    reason: Some("not needed".into()),
                },
            )
            .unwrap();
        let decision = rx.await.unwrap();
        match decision {
            PlanApprovalDecision::Reject { reason } => {
                assert_eq!(reason, Some("not needed".into()));
            }
            _ => panic!("Expected Reject"),
        }
    }

    #[tokio::test]
    async fn test_approve_with_edits() {
        let store = PendingPlanApprovals::new();
        let req_id = Uuid::new_v4();
        let rx = store.register(
            req_id,
            Uuid::new_v4(),
            "test goal".into(),
            Duration::from_secs(300),
        );

        let edited = vec![PlanStep {
            id: 1,
            action: "edited action".into(),
            tool_category: ToolCategory::Llm,
            dependency: super::super::planner::StepDependency::None,
            expected_output: None,
            executor_agent: None,
            executor_model: None,
            requires_visual_verification: None,
            prd_content: None,
            executor_type: None,
        }];
        store
            .complete(
                &req_id,
                PlanApprovalDecision::ApproveWithEdits {
                    edited_steps: edited,
                },
            )
            .unwrap();

        let decision = rx.await.unwrap();
        match decision {
            PlanApprovalDecision::ApproveWithEdits { edited_steps } => {
                assert_eq!(edited_steps.len(), 1);
                assert_eq!(edited_steps[0].action, "edited action");
            }
            _ => panic!("Expected ApproveWithEdits"),
        }
    }

    #[tokio::test]
    async fn test_revise() {
        let store = PendingPlanApprovals::new();
        let req_id = Uuid::new_v4();
        let rx = store.register(
            req_id,
            Uuid::new_v4(),
            "test goal".into(),
            Duration::from_secs(300),
        );

        store
            .complete(
                &req_id,
                PlanApprovalDecision::Revise {
                    feedback: "use browser instead".into(),
                },
            )
            .unwrap();

        let decision = rx.await.unwrap();
        match decision {
            PlanApprovalDecision::Revise { feedback } => {
                assert_eq!(feedback, "use browser instead");
            }
            _ => panic!("Expected Revise"),
        }
    }

    #[tokio::test]
    async fn test_complete_not_found() {
        let store = PendingPlanApprovals::new();
        let result = store.complete(&Uuid::new_v4(), PlanApprovalDecision::Approve);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_evict_expired() {
        let store = PendingPlanApprovals::new();
        // Register with 0ms timeout so it's immediately expired (with 2x factor)
        let req_id = Uuid::new_v4();
        let _rx = store.register(
            req_id,
            Uuid::new_v4(),
            "test".into(),
            Duration::from_millis(0),
        );
        assert_eq!(store.pending_count(), 1);

        // Small delay to ensure expiry
        tokio::time::sleep(Duration::from_millis(5)).await;
        let evicted = store.evict_expired();
        assert_eq!(evicted, 1);
        assert_eq!(store.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_timeout_receiver_error() {
        let store = PendingPlanApprovals::new();
        let req_id = Uuid::new_v4();
        let rx = store.register(
            req_id,
            Uuid::new_v4(),
            "test".into(),
            Duration::from_millis(50),
        );

        // Simulate timeout: drop does not complete, receiver awaits with timeout
        let result = tokio::time::timeout(Duration::from_millis(100), rx).await;
        // Receiver should error because sender is never called (it's still in the map)
        assert!(result.is_err()); // timeout
    }

    #[tokio::test]
    async fn test_double_complete() {
        let store = PendingPlanApprovals::new();
        let req_id = Uuid::new_v4();
        let _rx = store.register(
            req_id,
            Uuid::new_v4(),
            "test".into(),
            Duration::from_secs(300),
        );

        // First complete succeeds
        store
            .complete(&req_id, PlanApprovalDecision::Approve)
            .unwrap();
        // Second complete fails (entry removed)
        let result = store.complete(&req_id, PlanApprovalDecision::Approve);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_risk_classification() {
        assert_eq!(classify_risk(&ToolCategory::Shell), "high");
        assert_eq!(classify_risk(&ToolCategory::Browser), "high");
        assert_eq!(classify_risk(&ToolCategory::File), "medium");
        assert_eq!(classify_risk(&ToolCategory::Code), "medium");
        assert_eq!(classify_risk(&ToolCategory::Llm), "low");
        assert_eq!(classify_risk(&ToolCategory::Search), "low");
    }

    #[test]
    fn test_max_risk_level() {
        use super::super::planner::StepDependency;

        let steps = vec![
            PlanStep {
                id: 1,
                action: "search".into(),
                tool_category: ToolCategory::Search,
                dependency: StepDependency::None,
                expected_output: None,
                executor_agent: None,
                executor_model: None,
                requires_visual_verification: None,
                prd_content: None,
                executor_type: None,
            },
            PlanStep {
                id: 2,
                action: "llm".into(),
                tool_category: ToolCategory::Llm,
                dependency: StepDependency::Sequential,
                expected_output: None,
                executor_agent: None,
                executor_model: None,
                requires_visual_verification: None,
                prd_content: None,
                executor_type: None,
            },
        ];
        assert_eq!(max_risk_level(&steps), "low");

        let steps_with_file = vec![
            PlanStep {
                id: 1,
                action: "search".into(),
                tool_category: ToolCategory::Search,
                dependency: StepDependency::None,
                expected_output: None,
                executor_agent: None,
                executor_model: None,
                requires_visual_verification: None,
                prd_content: None,
                executor_type: None,
            },
            PlanStep {
                id: 2,
                action: "write".into(),
                tool_category: ToolCategory::File,
                dependency: StepDependency::Sequential,
                expected_output: None,
                executor_agent: None,
                executor_model: None,
                requires_visual_verification: None,
                prd_content: None,
                executor_type: None,
            },
        ];
        assert_eq!(max_risk_level(&steps_with_file), "medium");

        let steps_with_shell = vec![
            PlanStep {
                id: 1,
                action: "search".into(),
                tool_category: ToolCategory::Search,
                dependency: StepDependency::None,
                expected_output: None,
                executor_agent: None,
                executor_model: None,
                requires_visual_verification: None,
                prd_content: None,
                executor_type: None,
            },
            PlanStep {
                id: 2,
                action: "run".into(),
                tool_category: ToolCategory::Shell,
                dependency: StepDependency::Sequential,
                expected_output: None,
                executor_agent: None,
                executor_model: None,
                requires_visual_verification: None,
                prd_content: None,
                executor_type: None,
            },
        ];
        assert_eq!(max_risk_level(&steps_with_shell), "high");
    }

    #[test]
    fn test_plan_step_review_from_plan_step() {
        use super::super::planner::StepDependency;

        let step = PlanStep {
            id: 1,
            action: "Navigate to page".into(),
            tool_category: ToolCategory::Browser,
            dependency: StepDependency::Sequential,
            expected_output: Some("Page loaded".into()),
            executor_agent: None,
            executor_model: None,
            requires_visual_verification: None,
            prd_content: None,
            executor_type: None,
        };
        let review = PlanStepReview::from_plan_step(&step, Some("qwen-vl".into()));
        assert_eq!(review.id, 1);
        assert_eq!(review.risk_level, "high");
        assert_eq!(review.estimated_model, Some("qwen-vl".into()));
        assert_eq!(review.tool_category, "browser");
    }

    #[test]
    fn test_decision_serde() {
        let approve = PlanApprovalDecision::Approve;
        let json = serde_json::to_string(&approve).unwrap();
        assert!(json.contains("Approve"));

        let revise = PlanApprovalDecision::Revise {
            feedback: "change step 2".into(),
        };
        let json = serde_json::to_string(&revise).unwrap();
        let decoded: PlanApprovalDecision = serde_json::from_str(&json).unwrap();
        match decoded {
            PlanApprovalDecision::Revise { feedback } => assert_eq!(feedback, "change step 2"),
            _ => panic!("Wrong variant"),
        }
    }
}
