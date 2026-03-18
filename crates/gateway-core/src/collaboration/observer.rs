//! Collaboration-level observer for Swarm and Expert events.
//!
//! Decoupled from GraphObserver to avoid polluting the generic graph layer
//! with collaboration-specific concerns.

use std::sync::Arc;

use async_trait::async_trait;

use crate::graph::execution_store::{EventPayload, ExecutionStore};

/// Observer for swarm and expert collaboration events.
///
/// All methods have default no-op implementations, so implementors
/// can selectively override only the events they care about.
#[async_trait]
pub trait CollaborationObserver: Send + Sync {
    // ── Swarm events ──

    /// Called when a swarm handoff is triggered.
    async fn on_handoff_triggered(
        &self,
        _exec_id: &str,
        _from: &str,
        _to: &str,
        _condition: &str,
        _count: u32,
    ) {
    }

    /// Called when a handoff condition is checked.
    async fn on_handoff_condition_checked(
        &self,
        _exec_id: &str,
        _agent: &str,
        _condition: &str,
        _matched: bool,
    ) {
    }

    /// Called when a cycle is detected in swarm execution.
    async fn on_cycle_detected(&self, _exec_id: &str, _agent: &str, _visit_count: u32) {}

    // ── Expert events ──

    /// Called when the supervisor selects a specialist.
    async fn on_supervisor_decision(
        &self,
        _exec_id: &str,
        _selected: Option<&str>,
        _available: &[String],
    ) {
    }

    /// Called when a specialist is dispatched.
    async fn on_specialist_dispatched(
        &self,
        _exec_id: &str,
        _specialist: &str,
        _dispatch_count: u32,
    ) {
    }

    /// Called when a quality gate produces a result.
    async fn on_quality_gate_result(
        &self,
        _exec_id: &str,
        _specialist: &str,
        _score: f32,
        _passed: bool,
        _feedback: &str,
    ) {
    }
}

/// Bridges CollaborationObserver events to an ExecutionStore.
pub struct CollaborationRecorder {
    store: Arc<ExecutionStore>,
    execution_id: String,
}

impl CollaborationRecorder {
    /// Create a new collaboration recorder.
    pub fn new(store: Arc<ExecutionStore>, execution_id: impl Into<String>) -> Self {
        Self {
            store,
            execution_id: execution_id.into(),
        }
    }
}

#[async_trait]
impl CollaborationObserver for CollaborationRecorder {
    async fn on_handoff_triggered(
        &self,
        _exec_id: &str,
        from: &str,
        to: &str,
        condition: &str,
        count: u32,
    ) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::HandoffTriggered {
                    from_agent: from.to_string(),
                    to_agent: to.to_string(),
                    condition: condition.to_string(),
                    handoff_count: count,
                },
            )
            .await;
    }

    async fn on_handoff_condition_checked(
        &self,
        _exec_id: &str,
        agent: &str,
        condition: &str,
        matched: bool,
    ) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::HandoffConditionChecked {
                    agent: agent.to_string(),
                    condition: condition.to_string(),
                    matched,
                },
            )
            .await;
    }

    async fn on_cycle_detected(&self, _exec_id: &str, agent: &str, visit_count: u32) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::CycleDetected {
                    agent: agent.to_string(),
                    visit_count,
                },
            )
            .await;
    }

    async fn on_supervisor_decision(
        &self,
        _exec_id: &str,
        selected: Option<&str>,
        available: &[String],
    ) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::SupervisorDecision {
                    selected: selected.map(|s| s.to_string()),
                    available: available.to_vec(),
                },
            )
            .await;
    }

    async fn on_specialist_dispatched(
        &self,
        _exec_id: &str,
        specialist: &str,
        dispatch_count: u32,
    ) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::SpecialistDispatched {
                    specialist: specialist.to_string(),
                    dispatch_count,
                },
            )
            .await;
    }

    async fn on_quality_gate_result(
        &self,
        _exec_id: &str,
        specialist: &str,
        score: f32,
        passed: bool,
        feedback: &str,
    ) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::QualityGateResult {
                    specialist: specialist.to_string(),
                    score,
                    passed,
                    feedback: feedback.to_string(),
                },
            )
            .await;
    }
}

/// A no-op collaboration observer.
pub struct NoOpCollaborationObserver;

#[async_trait]
impl CollaborationObserver for NoOpCollaborationObserver {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::execution_store::ExecutionMode;

    #[tokio::test]
    async fn test_collaboration_recorder_swarm_events() {
        let store = Arc::new(ExecutionStore::new(10));
        store.start_execution("exec_1", ExecutionMode::Swarm).await;

        let recorder = CollaborationRecorder::new(store.clone(), "exec_1");

        recorder
            .on_handoff_triggered("exec_1", "agent_a", "agent_b", "OnKeyword", 1)
            .await;
        recorder
            .on_handoff_condition_checked("exec_1", "agent_a", "OnKeyword", true)
            .await;
        recorder.on_cycle_detected("exec_1", "agent_a", 3).await;

        let events = store.get_events("exec_1", 0, None);
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[0].payload,
            EventPayload::HandoffTriggered { .. }
        ));
        assert!(matches!(
            events[1].payload,
            EventPayload::HandoffConditionChecked { .. }
        ));
        assert!(matches!(
            events[2].payload,
            EventPayload::CycleDetected { .. }
        ));
    }

    #[tokio::test]
    async fn test_collaboration_recorder_expert_events() {
        let store = Arc::new(ExecutionStore::new(10));
        store.start_execution("exec_1", ExecutionMode::Expert).await;

        let recorder = CollaborationRecorder::new(store.clone(), "exec_1");

        recorder
            .on_supervisor_decision(
                "exec_1",
                Some("analyst"),
                &["analyst".into(), "critic".into()],
            )
            .await;
        recorder
            .on_specialist_dispatched("exec_1", "analyst", 1)
            .await;
        recorder
            .on_quality_gate_result("exec_1", "analyst", 0.85, true, "Good analysis")
            .await;

        let events = store.get_events("exec_1", 0, None);
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[0].payload,
            EventPayload::SupervisorDecision { .. }
        ));
        assert!(matches!(
            events[1].payload,
            EventPayload::SpecialistDispatched { .. }
        ));
        assert!(matches!(
            events[2].payload,
            EventPayload::QualityGateResult { .. }
        ));
    }

    #[tokio::test]
    async fn test_noop_collaboration_observer() {
        let obs = NoOpCollaborationObserver;
        // Should not panic
        obs.on_handoff_triggered("x", "a", "b", "c", 1).await;
        obs.on_cycle_detected("x", "a", 1).await;
    }
}
