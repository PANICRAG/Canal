//! Execution Tracker - Records tool executions and retry history

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Single tool execution record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecution {
    pub tool: String,
    pub input: serde_json::Value,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub success: bool,
    pub duration_ms: u64,
    pub retry: u32,
    pub ts: DateTime<Utc>,
}

/// Execution context for a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub session_id: String,
    pub skill: Option<String>,
    pub executions: Vec<ToolExecution>,
    pub retries: u32,
    pub success: bool,
}

/// Execution log for reporting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionLog {
    pub session_id: String,
    pub skill: Option<String>,
    pub total_executions: usize,
    pub total_retries: u32,
    pub success: bool,
    pub executions: Vec<ToolExecution>,
    pub summary: String,
}

/// Tracks execution across sessions
pub struct ExecutionTracker {
    contexts: HashMap<String, ExecutionContext>,
    max_retries: u32,
}

impl ExecutionTracker {
    pub fn new(max_retries: u32) -> Self {
        Self {
            contexts: HashMap::new(),
            max_retries,
        }
    }

    /// Start tracking a new session
    pub fn start(&mut self, session_id: &str, skill: Option<&str>) {
        self.contexts.insert(
            session_id.to_string(),
            ExecutionContext {
                session_id: session_id.to_string(),
                skill: skill.map(|s| s.to_string()),
                executions: Vec::new(),
                retries: 0,
                success: false,
            },
        );
    }

    /// Record a tool execution
    pub fn record(&mut self, session_id: &str, exec: ToolExecution) {
        if let Some(ctx) = self.contexts.get_mut(session_id) {
            ctx.executions.push(exec);
        }
    }

    /// Increment retry count, returns true if can retry
    pub fn retry(&mut self, session_id: &str) -> bool {
        if let Some(ctx) = self.contexts.get_mut(session_id) {
            ctx.retries += 1;
            return ctx.retries <= self.max_retries;
        }
        false
    }

    /// Mark session as successful
    pub fn mark_success(&mut self, session_id: &str) {
        if let Some(ctx) = self.contexts.get_mut(session_id) {
            ctx.success = true;
        }
    }

    /// Get context
    pub fn get(&self, session_id: &str) -> Option<&ExecutionContext> {
        self.contexts.get(session_id)
    }

    /// Generate execution log
    pub fn log(&self, session_id: &str) -> Option<ExecutionLog> {
        self.contexts.get(session_id).map(|ctx| {
            let summary = if ctx.success {
                if ctx.retries > 0 {
                    format!("OK after {} retries", ctx.retries)
                } else {
                    "OK".to_string()
                }
            } else {
                format!("FAILED after {} attempts", ctx.retries + 1)
            };

            ExecutionLog {
                session_id: ctx.session_id.clone(),
                skill: ctx.skill.clone(),
                total_executions: ctx.executions.len(),
                total_retries: ctx.retries,
                success: ctx.success,
                executions: ctx.executions.clone(),
                summary,
            }
        })
    }

    /// Clear session
    pub fn clear(&mut self, session_id: &str) {
        self.contexts.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracker_basic() {
        let mut tracker = ExecutionTracker::new(3);
        tracker.start("s1", Some("gmail"));

        let exec = ToolExecution {
            tool: "browser_click".to_string(),
            input: serde_json::json!({"selector": "btn"}),
            output: None,
            error: Some("timeout".to_string()),
            success: false,
            duration_ms: 5000,
            retry: 0,
            ts: Utc::now(),
        };

        tracker.record("s1", exec);
        assert!(tracker.retry("s1"));
        assert!(tracker.retry("s1"));
        assert!(tracker.retry("s1"));
        assert!(!tracker.retry("s1")); // max reached

        let log = tracker.log("s1").unwrap();
        assert_eq!(log.total_retries, 4);
        assert!(!log.success);
    }
}
