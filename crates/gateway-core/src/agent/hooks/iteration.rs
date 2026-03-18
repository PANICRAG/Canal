//! Iteration Learning Hook - Self-improvement through failure analysis
//!
//! Implements the learning loop: Execute → Verify → Diagnose → Record → Retry → Report

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::HookCallback;
use crate::agent::iteration::{ExecutionTracker, LearnedIssue, SkillUpdater, ToolExecution};
use crate::agent::types::{HookContext, HookEvent, HookResult, PostToolUseHookData};

/// Configuration for iteration learning
#[derive(Debug, Clone)]
pub struct IterationConfig {
    /// Maximum retries before giving up
    pub max_retries: u32,
    /// Auto-record issues on failure
    pub auto_record: bool,
    /// Skill directory path
    pub skill_dir: PathBuf,
    /// Enable deduplication check
    pub deduplicate: bool,
}

impl Default for IterationConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            auto_record: true,
            skill_dir: PathBuf::from(".agent/skills"),
            deduplicate: true,
        }
    }
}

/// Iteration learning hook
///
/// Tracks tool executions and learns from failures.
/// Learning loop: Execute → Verify → Diagnose → Record → Retry → Report
pub struct IterationHook {
    config: IterationConfig,
    tracker: Arc<RwLock<ExecutionTracker>>,
    updater: SkillUpdater,
    current_skill: Arc<RwLock<Option<String>>>,
}

impl IterationHook {
    /// Create new iteration hook
    pub fn new(config: IterationConfig) -> Self {
        let max_retries = config.max_retries;
        let skill_dir = config.skill_dir.clone();
        Self {
            config,
            tracker: Arc::new(RwLock::new(ExecutionTracker::new(max_retries))),
            updater: SkillUpdater::new(skill_dir),
            current_skill: Arc::new(RwLock::new(None)),
        }
    }

    /// Set current skill context
    pub async fn set_skill(&self, skill: Option<String>) {
        let mut current = self.current_skill.write().await;
        *current = skill;
    }

    /// Start tracking a session
    pub async fn start_session(&self, session_id: &str, skill: Option<&str>) {
        let mut tracker = self.tracker.write().await;
        tracker.start(session_id, skill);
        if let Some(s) = skill {
            let mut current = self.current_skill.write().await;
            *current = Some(s.to_string());
        }
    }

    /// Get execution log for session
    pub async fn get_log(&self, session_id: &str) -> Option<crate::agent::iteration::ExecutionLog> {
        let tracker = self.tracker.read().await;
        tracker.log(session_id)
    }

    /// Record a learned issue
    pub async fn record_issue(
        &self,
        symptom: &str,
        solution: &str,
    ) -> Result<bool, std::io::Error> {
        let skill = self.current_skill.read().await;
        if let Some(skill_name) = skill.as_ref() {
            let issue = LearnedIssue {
                symptom: symptom.to_string(),
                cause: None,
                solution: solution.to_string(),
                verify: None,
            };
            self.updater.add_issue(skill_name, &issue).await
        } else {
            Ok(false)
        }
    }

    /// Handle post-tool-use event
    async fn handle_post_tool_use(
        &self,
        data: &PostToolUseHookData,
        context: &HookContext,
    ) -> HookResult {
        let session_id = &context.session_id;
        let duration = data.duration_ms.unwrap_or(0);

        // Record execution
        let exec = ToolExecution {
            tool: data.tool_name.clone(),
            input: data.input.clone(),
            output: Some(data.result.clone()),
            error: if data.is_error {
                data.result.as_str().map(String::from)
            } else {
                None
            },
            success: !data.is_error,
            duration_ms: duration,
            retry: 0,
            ts: chrono::Utc::now(),
        };

        // R1-H3: Single write-lock acquisition for record + error handling.
        // Previously had two separate write-lock acquisitions.
        let should_retry = {
            let mut tracker = self.tracker.write().await;
            tracker.record(session_id, exec);

            if data.is_error {
                Some(tracker.retry(session_id))
            } else {
                tracker.mark_success(session_id);
                None
            }
        };

        // Handle error case (lock released)
        if let Some(can_retry) = should_retry {
            if can_retry {
                tracing::info!(
                    tool = %data.tool_name,
                    session = %session_id,
                    "Tool failed, will retry"
                );
                return HookResult::Retry {
                    modified_data: None,
                    delay_ms: Some(1000),
                };
            } else {
                tracing::warn!(
                    tool = %data.tool_name,
                    session = %session_id,
                    "Tool failed, max retries reached"
                );

                // Auto-record issue if enabled
                if self.config.auto_record {
                    let skill = self.current_skill.read().await;
                    if let Some(skill_name) = skill.as_ref() {
                        if let Some(error_msg) = data.result.as_str() {
                            let issue = LearnedIssue {
                                symptom: format!("{} failed: {}", data.tool_name, error_msg),
                                cause: None,
                                solution: "TODO: Investigate and document solution".to_string(),
                                verify: None,
                            };

                            // Fire and forget - don't block on recording
                            let skill_dir = self.config.skill_dir.clone();
                            let skill_name = skill_name.clone();
                            tokio::spawn(async move {
                                let updater = SkillUpdater::new(skill_dir);
                                if let Err(e) = updater.add_issue(&skill_name, &issue).await {
                                    tracing::error!(error = %e, "Failed to record issue");
                                }
                            });
                        }
                    }
                }
            }
        }

        HookResult::continue_()
    }
}

impl Clone for IterationHook {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            tracker: Arc::clone(&self.tracker),
            updater: self.updater.clone(),
            current_skill: Arc::clone(&self.current_skill),
        }
    }
}

#[async_trait]
impl HookCallback for IterationHook {
    async fn on_event(
        &self,
        event: HookEvent,
        data: serde_json::Value,
        context: &HookContext,
    ) -> HookResult {
        match event {
            HookEvent::PostToolUse => {
                if let Ok(post_data) = serde_json::from_value::<PostToolUseHookData>(data) {
                    return self.handle_post_tool_use(&post_data, context).await;
                }
            }
            HookEvent::Error => {
                // Log error for learning
                tracing::debug!(session = %context.session_id, "Error event received");
            }
            _ => {}
        }
        HookResult::continue_()
    }

    fn name(&self) -> &str {
        "iteration_learning"
    }

    fn handles_event(&self, event: HookEvent) -> bool {
        matches!(event, HookEvent::PostToolUse | HookEvent::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_iteration_hook_basic() {
        let dir = TempDir::new().unwrap();
        let config = IterationConfig {
            skill_dir: dir.path().to_path_buf(),
            ..Default::default()
        };

        let hook = IterationHook::new(config);

        // Start session
        hook.start_session("test-session", Some("test-skill")).await;

        // Check skill is set
        let skill = hook.current_skill.read().await;
        assert_eq!(skill.as_ref().map(|s| s.as_str()), Some("test-skill"));
    }

    #[tokio::test]
    async fn test_iteration_hook_handles_events() {
        let dir = TempDir::new().unwrap();
        let config = IterationConfig {
            skill_dir: dir.path().to_path_buf(),
            ..Default::default()
        };

        let hook = IterationHook::new(config);

        assert!(hook.handles_event(HookEvent::PostToolUse));
        assert!(hook.handles_event(HookEvent::Error));
        assert!(!hook.handles_event(HookEvent::PreToolUse));
    }
}
