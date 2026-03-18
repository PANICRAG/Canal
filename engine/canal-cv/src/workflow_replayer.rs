//! WorkflowReplayer — replays workflow templates with parameter substitution.
//!
//! During replay, Click and Type targets are re-detected via VisionPipeline
//! (not raw coordinates) to handle UI layout changes. KeyPress, Scroll, Wait
//! replay directly without re-detection.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::pipeline::ComputerUsePipeline;
use crate::workflow_template::{TemplateAction, WorkflowTemplate};

/// Result of replaying a workflow template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResult {
    /// Template ID that was replayed.
    pub template_id: String,
    /// Number of steps completed.
    pub steps_completed: usize,
    /// Total number of steps in template.
    pub total_steps: usize,
    /// Whether all steps succeeded.
    pub success: bool,
    /// Total replay duration in milliseconds.
    pub duration_ms: u64,
    /// Per-step results.
    pub step_results: Vec<StepReplayResult>,
}

/// Result of replaying a single step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepReplayResult {
    /// Step index.
    pub index: usize,
    /// Whether this step succeeded.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
    /// Duration of this step in milliseconds.
    pub duration_ms: u64,
}

/// Replays workflow templates using the CV4 pipeline.
///
/// Targets are re-detected via VisionPipeline during replay, not hardcoded
/// coordinates. This handles UI layout changes between recording and replay.
pub struct WorkflowReplayer {
    pipeline: Arc<ComputerUsePipeline>,
}

impl WorkflowReplayer {
    /// Create a new replayer with a pipeline.
    pub fn new(pipeline: Arc<ComputerUsePipeline>) -> Self {
        Self { pipeline }
    }

    /// Replay a template with parameter substitution.
    ///
    /// Parameters replace detected values in Type steps. Click targets
    /// are re-detected via VisionPipeline by description.
    pub async fn replay(
        &self,
        template: &WorkflowTemplate,
        params: &HashMap<String, String>,
    ) -> anyhow::Result<ReplayResult> {
        let start = Instant::now();
        let mut step_results = Vec::new();
        let mut all_success = true;

        for step in &template.steps {
            let step_start = Instant::now();

            let instruction = match &step.action {
                TemplateAction::Click { target_description } => {
                    format!("Click '{}'", target_description)
                }
                TemplateAction::Type {
                    target_description,
                    text,
                    parameter_name,
                } => {
                    // Substitute parameter if available
                    let actual_text = parameter_name
                        .as_ref()
                        .and_then(|name| params.get(name))
                        .unwrap_or(text);
                    format!("Click '{}' and type '{}'", target_description, actual_text)
                }
                TemplateAction::KeyPress { key } => format!("Press {}", key),
                TemplateAction::Scroll { direction, amount } => {
                    format!("Scroll {} by {}", direction, amount)
                }
                TemplateAction::Wait { duration_ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(*duration_ms)).await;
                    step_results.push(StepReplayResult {
                        index: step.index,
                        success: true,
                        error: None,
                        duration_ms: step_start.elapsed().as_millis() as u64,
                    });
                    continue;
                }
            };

            let result = self.pipeline.act(&instruction).await;
            let step_duration = step_start.elapsed().as_millis() as u64;

            match result {
                Ok(_) => {
                    step_results.push(StepReplayResult {
                        index: step.index,
                        success: true,
                        error: None,
                        duration_ms: step_duration,
                    });
                }
                Err(e) => {
                    all_success = false;
                    step_results.push(StepReplayResult {
                        index: step.index,
                        success: false,
                        error: Some(e.to_string()),
                        duration_ms: step_duration,
                    });
                    // Stop on first failure
                    break;
                }
            }
        }

        Ok(ReplayResult {
            template_id: template.id.clone(),
            steps_completed: step_results.iter().filter(|s| s.success).count(),
            total_steps: template.steps.len(),
            success: all_success,
            duration_ms: start.elapsed().as_millis() as u64,
            step_results,
        })
    }

    /// Get reference to the underlying pipeline.
    pub fn pipeline(&self) -> &ComputerUsePipeline {
        &self.pipeline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replay_result_serialization() {
        let result = ReplayResult {
            template_id: "abc".into(),
            steps_completed: 2,
            total_steps: 3,
            success: false,
            duration_ms: 500,
            step_results: vec![
                StepReplayResult {
                    index: 0,
                    success: true,
                    error: None,
                    duration_ms: 100,
                },
                StepReplayResult {
                    index: 1,
                    success: true,
                    error: None,
                    duration_ms: 150,
                },
                StepReplayResult {
                    index: 2,
                    success: false,
                    error: Some("Target not found".into()),
                    duration_ms: 250,
                },
            ],
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ReplayResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.steps_completed, 2);
        assert!(!deserialized.success);
        assert_eq!(deserialized.step_results.len(), 3);
    }

    #[test]
    fn test_step_replay_result_success() {
        let step = StepReplayResult {
            index: 0,
            success: true,
            error: None,
            duration_ms: 50,
        };
        assert!(step.success);
        assert!(step.error.is_none());
    }

    #[test]
    fn test_step_replay_result_failure() {
        let step = StepReplayResult {
            index: 1,
            success: false,
            error: Some("Not found".into()),
            duration_ms: 200,
        };
        assert!(!step.success);
        assert_eq!(step.error.as_deref(), Some("Not found"));
    }
}
