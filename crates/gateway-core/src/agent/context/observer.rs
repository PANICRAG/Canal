//! Agent Observer for conversation tracing.
//!
//! Provides an observation trait for tracking prompt construction,
//! LLM requests/responses, tool calls, and validation checks.

use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::inspector::PromptInspection;

/// Trait for observing agent execution events.
///
/// Implement this trait to track prompt construction, LLM calls,
/// tool invocations, and validation results.
#[async_trait]
pub trait AgentObserver: Send + Sync {
    /// Called when a prompt is constructed from the context hierarchy.
    async fn on_prompt_constructed(&self, inspection: &PromptInspection);

    /// Called before sending a request to the LLM.
    async fn on_llm_request(&self, model: &str, messages_count: usize, tokens: usize);

    /// Called after receiving a response from the LLM.
    async fn on_llm_response(&self, model: &str, duration_ms: u64, output_tokens: usize);

    /// Called after pre-flight validation.
    async fn on_preflight_check(&self, passed: bool, issues: &[String]);

    /// Called when a tool is invoked.
    async fn on_tool_call(&self, tool_name: &str, duration_ms: u64, success: bool);

    /// Called after post-flight validation.
    async fn on_postflight_check(&self, passed: bool, repair_triggered: bool);

    /// Called when a turn completes.
    async fn on_turn_complete(&self, turn: u32, total_tokens: usize);
}

/// JSONL trace event written to trace files.
#[derive(Debug, Serialize)]
struct TraceEvent {
    timestamp: String,
    event_type: String,
    #[serde(flatten)]
    data: serde_json::Value,
}

/// Agent observer that writes JSONL trace files.
///
/// Each session gets its own trace file at
/// `{trace_dir}/{session_id}.jsonl`.
pub struct JsonlAgentObserver {
    trace_dir: PathBuf,
    session_id: String,
    writer: Mutex<Option<tokio::io::BufWriter<tokio::fs::File>>>,
}

impl JsonlAgentObserver {
    /// Create a new JSONL observer.
    ///
    /// The trace file is created lazily on the first event.
    pub fn new(trace_dir: impl Into<PathBuf>, session_id: impl Into<String>) -> Self {
        Self {
            trace_dir: trace_dir.into(),
            session_id: session_id.into(),
            writer: Mutex::new(None),
        }
    }

    /// Write a trace event to the JSONL file.
    async fn write_event(&self, event_type: &str, data: serde_json::Value) {
        let event = TraceEvent {
            timestamp: Utc::now().to_rfc3339(),
            event_type: event_type.to_string(),
            data,
        };

        let line = match serde_json::to_string(&event) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("Failed to serialize trace event: {}", e);
                return;
            }
        };

        let mut guard = self.writer.lock().await;

        // Lazily initialize the writer
        if guard.is_none() {
            match self.init_writer().await {
                Ok(w) => *guard = Some(w),
                Err(e) => {
                    tracing::warn!("Failed to init trace writer: {}", e);
                    return;
                }
            }
        }

        if let Some(ref mut writer) = *guard {
            use tokio::io::AsyncWriteExt;
            if let Err(e) = writer.write_all(line.as_bytes()).await {
                tracing::warn!("Failed to write trace event: {}", e);
            }
            if let Err(e) = writer.write_all(b"\n").await {
                tracing::warn!("Failed to write newline: {}", e);
            }
            let _ = writer.flush().await;
        }
    }

    /// Initialize the trace file writer.
    async fn init_writer(&self) -> Result<tokio::io::BufWriter<tokio::fs::File>, std::io::Error> {
        tokio::fs::create_dir_all(&self.trace_dir).await?;
        let path = self.trace_dir.join(format!("{}.jsonl", self.session_id));
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        Ok(tokio::io::BufWriter::new(file))
    }
}

#[async_trait]
impl AgentObserver for JsonlAgentObserver {
    async fn on_prompt_constructed(&self, inspection: &PromptInspection) {
        self.write_event(
            "prompt_constructed",
            serde_json::json!({
                "total_tokens": inspection.total_tokens,
                "total_budget": inspection.total_budget,
                "utilization": inspection.utilization,
                "section_count": inspection.sections.len(),
                "sections": inspection.sections.iter().map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "tokens": s.tokens,
                        "truncated": s.truncated,
                    })
                }).collect::<Vec<_>>(),
            }),
        )
        .await;
    }

    async fn on_llm_request(&self, model: &str, messages_count: usize, tokens: usize) {
        self.write_event(
            "llm_request",
            serde_json::json!({
                "model": model,
                "messages_count": messages_count,
                "input_tokens": tokens,
            }),
        )
        .await;
    }

    async fn on_llm_response(&self, model: &str, duration_ms: u64, output_tokens: usize) {
        self.write_event(
            "llm_response",
            serde_json::json!({
                "model": model,
                "duration_ms": duration_ms,
                "output_tokens": output_tokens,
            }),
        )
        .await;
    }

    async fn on_preflight_check(&self, passed: bool, issues: &[String]) {
        self.write_event(
            "preflight_check",
            serde_json::json!({
                "passed": passed,
                "issues": issues,
            }),
        )
        .await;
    }

    async fn on_tool_call(&self, tool_name: &str, duration_ms: u64, success: bool) {
        self.write_event(
            "tool_call",
            serde_json::json!({
                "tool_name": tool_name,
                "duration_ms": duration_ms,
                "success": success,
            }),
        )
        .await;
    }

    async fn on_postflight_check(&self, passed: bool, repair_triggered: bool) {
        self.write_event(
            "postflight_check",
            serde_json::json!({
                "passed": passed,
                "repair_triggered": repair_triggered,
            }),
        )
        .await;
    }

    async fn on_turn_complete(&self, turn: u32, total_tokens: usize) {
        self.write_event(
            "turn_complete",
            serde_json::json!({
                "turn": turn,
                "total_tokens": total_tokens,
            }),
        )
        .await;
    }
}

/// Composite observer that dispatches events to multiple observers.
pub struct CompositeAgentObserver {
    observers: Vec<Arc<dyn AgentObserver>>,
}

impl CompositeAgentObserver {
    /// Create a new composite observer.
    pub fn new() -> Self {
        Self {
            observers: Vec::new(),
        }
    }

    /// Add an observer.
    pub fn add(&mut self, observer: Arc<dyn AgentObserver>) {
        self.observers.push(observer);
    }
}

impl Default for CompositeAgentObserver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentObserver for CompositeAgentObserver {
    async fn on_prompt_constructed(&self, inspection: &PromptInspection) {
        for obs in &self.observers {
            obs.on_prompt_constructed(inspection).await;
        }
    }

    async fn on_llm_request(&self, model: &str, messages_count: usize, tokens: usize) {
        for obs in &self.observers {
            obs.on_llm_request(model, messages_count, tokens).await;
        }
    }

    async fn on_llm_response(&self, model: &str, duration_ms: u64, output_tokens: usize) {
        for obs in &self.observers {
            obs.on_llm_response(model, duration_ms, output_tokens).await;
        }
    }

    async fn on_preflight_check(&self, passed: bool, issues: &[String]) {
        for obs in &self.observers {
            obs.on_preflight_check(passed, issues).await;
        }
    }

    async fn on_tool_call(&self, tool_name: &str, duration_ms: u64, success: bool) {
        for obs in &self.observers {
            obs.on_tool_call(tool_name, duration_ms, success).await;
        }
    }

    async fn on_postflight_check(&self, passed: bool, repair_triggered: bool) {
        for obs in &self.observers {
            obs.on_postflight_check(passed, repair_triggered).await;
        }
    }

    async fn on_turn_complete(&self, turn: u32, total_tokens: usize) {
        for obs in &self.observers {
            obs.on_turn_complete(turn, total_tokens).await;
        }
    }
}

/// No-op observer for when tracing is disabled.
pub struct NoOpAgentObserver;

#[async_trait]
impl AgentObserver for NoOpAgentObserver {
    async fn on_prompt_constructed(&self, _: &PromptInspection) {}
    async fn on_llm_request(&self, _: &str, _: usize, _: usize) {}
    async fn on_llm_response(&self, _: &str, _: u64, _: usize) {}
    async fn on_preflight_check(&self, _: bool, _: &[String]) {}
    async fn on_tool_call(&self, _: &str, _: u64, _: bool) {}
    async fn on_postflight_check(&self, _: bool, _: bool) {}
    async fn on_turn_complete(&self, _: u32, _: usize) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context::inspector::PromptInspection;
    use crate::agent::context::resolver::ResolvedContext;

    #[tokio::test]
    async fn test_noop_observer() {
        let obs = NoOpAgentObserver;
        let ctx = ResolvedContext::default();
        let inspection = PromptInspection::from_resolved(&ctx, 0);

        obs.on_prompt_constructed(&inspection).await;
        obs.on_llm_request("test", 1, 100).await;
        obs.on_llm_response("test", 200, 50).await;
        obs.on_preflight_check(true, &[]).await;
        obs.on_tool_call("read", 10, true).await;
        obs.on_postflight_check(true, false).await;
        obs.on_turn_complete(1, 150).await;
    }

    #[tokio::test]
    async fn test_composite_observer() {
        let mut composite = CompositeAgentObserver::new();
        composite.add(Arc::new(NoOpAgentObserver));
        composite.add(Arc::new(NoOpAgentObserver));

        let ctx = ResolvedContext::default();
        let inspection = PromptInspection::from_resolved(&ctx, 0);

        composite.on_prompt_constructed(&inspection).await;
        composite.on_turn_complete(1, 100).await;
    }

    #[tokio::test]
    async fn test_jsonl_observer_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let obs = JsonlAgentObserver::new(tmp.path(), "test-session");

        obs.on_llm_request("test-model", 5, 1000).await;
        obs.on_turn_complete(1, 1000).await;

        // Check that file was created
        let trace_path = tmp.path().join("test-session.jsonl");
        assert!(trace_path.exists());

        let content = std::fs::read_to_string(&trace_path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        // First line should be llm_request
        let event: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(event["event_type"], "llm_request");
        assert_eq!(event["model"], "test-model");
    }
}
