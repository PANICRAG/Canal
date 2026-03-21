//! AgentObserver implementation that writes LLM-layer events to ExecutionStore.
//!
//! Bridges the `AgentObserver` trait (LLM requests, responses, tool calls)
//! to the `ExecutionStore` for unified debug visibility.

use std::sync::Arc;

use async_trait::async_trait;

use super::execution_store::{EventPayload, ExecutionStore};

/// An `AgentObserver` that records LLM-layer events to an `ExecutionStore`.
pub struct ExecutionStoreObserver {
    store: Arc<ExecutionStore>,
    execution_id: String,
}

impl ExecutionStoreObserver {
    /// Create a new observer for a specific execution.
    pub fn new(store: Arc<ExecutionStore>, execution_id: impl Into<String>) -> Self {
        Self {
            store,
            execution_id: execution_id.into(),
        }
    }

    /// Get the execution ID this observer is recording to.
    pub fn execution_id(&self) -> &str {
        &self.execution_id
    }
}

#[cfg(feature = "context-engineering")]
#[async_trait]
impl crate::agent::context::observer::AgentObserver for ExecutionStoreObserver {
    async fn on_prompt_constructed(
        &self,
        _inspection: &crate::agent::context::inspector::PromptInspection,
    ) {
        // No-op: prompt construction details are not recorded
    }

    async fn on_llm_request(&self, model: &str, _messages_count: usize, tokens: usize) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::LlmRequest {
                    model: model.to_string(),
                    input_tokens: tokens,
                },
            )
            .await;
    }

    async fn on_llm_response(&self, model: &str, duration_ms: u64, output_tokens: usize) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::LlmResponse {
                    model: model.to_string(),
                    duration_ms,
                    output_tokens,
                },
            )
            .await;
    }

    async fn on_preflight_check(&self, _passed: bool, _issues: &[String]) {
        // No-op: preflight details are not recorded
    }

    async fn on_tool_call(&self, tool_name: &str, duration_ms: u64, success: bool) {
        self.store
            .append_event(
                &self.execution_id,
                EventPayload::ToolCall {
                    tool_name: tool_name.to_string(),
                    duration_ms,
                    success,
                },
            )
            .await;
    }

    async fn on_postflight_check(&self, _passed: bool, _repair_triggered: bool) {
        // No-op: postflight details are not recorded
    }

    async fn on_turn_complete(&self, _turn: u32, _total_tokens: usize) {
        // No-op: turn completion is tracked via other events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::execution_store::ExecutionMode;

    #[tokio::test]
    async fn test_execution_store_observer_creation() {
        let store = Arc::new(ExecutionStore::new(10));
        let observer = ExecutionStoreObserver::new(store, "exec_123");
        assert_eq!(observer.execution_id(), "exec_123");
    }

    #[tokio::test]
    async fn test_manual_event_recording() {
        // Test that we can manually record LLM events via the store
        let store = Arc::new(ExecutionStore::new(10));
        store.start_execution("exec_1", ExecutionMode::Direct).await;

        store
            .append_event(
                "exec_1",
                EventPayload::LlmRequest {
                    model: "claude-sonnet-4-5-20250929".to_string(),
                    input_tokens: 500,
                },
            )
            .await;

        store
            .append_event(
                "exec_1",
                EventPayload::LlmResponse {
                    model: "claude-sonnet-4-5-20250929".to_string(),
                    duration_ms: 800,
                    output_tokens: 200,
                },
            )
            .await;

        store
            .append_event(
                "exec_1",
                EventPayload::ToolCall {
                    tool_name: "computer_screenshot".to_string(),
                    duration_ms: 50,
                    success: true,
                },
            )
            .await;

        let events = store.get_events("exec_1", 0, None);
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0].payload, EventPayload::LlmRequest { .. }));
        assert!(matches!(
            events[1].payload,
            EventPayload::LlmResponse { .. }
        ));
        assert!(matches!(events[2].payload, EventPayload::ToolCall { .. }));
    }
}
