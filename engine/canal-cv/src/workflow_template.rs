//! WorkflowTemplate — parameterized workflow templates + code-based generalization.
//!
//! Templates are generalized from recordings via rule-based parameter detection
//! (0 LLM tokens, no hallucination). Parameters are typed text inputs that vary
//! between workflow invocations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::workflow_recorder::{RecordedAction, WorkflowRecording, WorkflowStep};

/// A parameterized workflow template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplate {
    /// Unique template ID.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of what this workflow does.
    pub description: String,
    /// Parameterized steps to replay.
    pub steps: Vec<TemplateStep>,
    /// Detected parameters (typed text that varies).
    pub parameters: Vec<WorkflowParameter>,
    /// Context pattern this workflow applies to (regex on app/title).
    pub context_pattern: Option<String>,
    /// Number of times this template has been used.
    pub use_count: u32,
    /// Last time this template was used.
    pub last_used: DateTime<Utc>,
    /// Success rate (0.0-1.0).
    pub success_rate: f32,
    /// Source recording ID.
    pub source_recording_id: String,
}

/// Summary of a workflow template (for listing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplateSummary {
    /// Template ID.
    pub id: String,
    /// Template name.
    pub name: String,
    /// Number of steps.
    pub step_count: usize,
    /// Number of parameters.
    pub parameter_count: usize,
    /// Usage count.
    pub use_count: u32,
    /// Success rate.
    pub success_rate: f32,
}

/// A parameterized step in a template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateStep {
    /// Step index.
    pub index: usize,
    /// Action to replay (parameterized version of RecordedAction).
    pub action: TemplateAction,
    /// Detection hint from recording (which method worked).
    pub detection_hint: String,
}

/// A template action — parameterized version of RecordedAction.
///
/// Click and Type use target descriptions for re-detection during replay.
/// KeyPress, Scroll, Wait replay directly without re-detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TemplateAction {
    /// Click target by description (re-detected via VisionPipeline during replay).
    Click { target_description: String },
    /// Type text — may contain parameter placeholder.
    Type {
        target_description: String,
        text: String,
        parameter_name: Option<String>,
    },
    /// Fixed key press.
    KeyPress { key: String },
    /// Fixed scroll.
    Scroll { direction: String, amount: f64 },
    /// Fixed wait.
    Wait { duration_ms: u64 },
}

/// A detected parameter in a workflow template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowParameter {
    /// Parameter name (from target description).
    pub name: String,
    /// Default value (from recording).
    pub default_value: String,
    /// Step index where this parameter appears.
    pub step_index: usize,
}

/// Shortcuts and common commands excluded from parameter detection.
const COMMON_SHORTCUTS: &[&str] = &[
    "enter",
    "tab",
    "escape",
    "esc",
    "backspace",
    "delete",
    "space",
];

/// Code-based workflow generalization (0 LLM tokens).
pub struct WorkflowGeneralizer;

impl WorkflowGeneralizer {
    /// Generalize a recording into a parameterized template.
    pub fn generalize(name: &str, recording: &WorkflowRecording) -> WorkflowTemplate {
        let parameters = Self::detect_parameters(&recording.steps);
        let steps = Self::build_template_steps(&recording.steps, &parameters);

        WorkflowTemplate {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            description: format!("Recorded workflow with {} steps", recording.steps.len()),
            steps,
            parameters,
            context_pattern: Self::detect_context_pattern(&recording.steps),
            use_count: 0,
            last_used: Utc::now(),
            success_rate: 1.0,
            source_recording_id: recording.id.clone(),
        }
    }

    /// Rule-based parameter detection — no LLM needed.
    ///
    /// Detects typed text inputs that are likely to vary between workflow runs.
    /// Excludes single chars, keyboard shortcuts, URLs, and common commands.
    fn detect_parameters(steps: &[WorkflowStep]) -> Vec<WorkflowParameter> {
        steps
            .iter()
            .filter_map(|step| match &step.action {
                RecordedAction::Type {
                    text,
                    target_description,
                    ..
                } => {
                    // Exclude: single char, keyboard shortcuts, common commands
                    let is_shortcut =
                        text.len() <= 1 || COMMON_SHORTCUTS.contains(&text.to_lowercase().as_str());
                    if !is_shortcut && text.len() > 1 {
                        Some(WorkflowParameter {
                            name: target_description.clone(),
                            default_value: text.clone(),
                            step_index: step.index,
                        })
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect()
    }

    /// Build template steps from recorded steps.
    fn build_template_steps(
        steps: &[WorkflowStep],
        parameters: &[WorkflowParameter],
    ) -> Vec<TemplateStep> {
        steps
            .iter()
            .map(|step| {
                let action = match &step.action {
                    RecordedAction::Click {
                        target_description, ..
                    } => TemplateAction::Click {
                        target_description: target_description.clone(),
                    },
                    RecordedAction::Type {
                        target_description,
                        text,
                        ..
                    } => {
                        let param_name = parameters
                            .iter()
                            .find(|p| p.step_index == step.index)
                            .map(|p| p.name.clone());
                        TemplateAction::Type {
                            target_description: target_description.clone(),
                            text: text.clone(),
                            parameter_name: param_name,
                        }
                    }
                    RecordedAction::KeyPress { key } => {
                        TemplateAction::KeyPress { key: key.clone() }
                    }
                    RecordedAction::Scroll { direction, amount } => TemplateAction::Scroll {
                        direction: direction.clone(),
                        amount: *amount,
                    },
                    RecordedAction::Wait { duration_ms } => TemplateAction::Wait {
                        duration_ms: *duration_ms,
                    },
                    RecordedAction::Extract { .. } | RecordedAction::Observe => {
                        // Extract and Observe don't replay — skip
                        TemplateAction::Wait { duration_ms: 0 }
                    }
                };

                TemplateStep {
                    index: step.index,
                    action,
                    detection_hint: step.detection_method.clone(),
                }
            })
            .collect()
    }

    /// Detect context pattern from recording (first step's context).
    fn detect_context_pattern(steps: &[WorkflowStep]) -> Option<String> {
        steps.first().and_then(|s| {
            s.context_before
                .as_ref()
                .and_then(|ctx| ctx.app_name.clone())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_recording() -> WorkflowRecording {
        WorkflowRecording {
            id: "test-recording".into(),
            steps: vec![
                WorkflowStep {
                    index: 0,
                    action: RecordedAction::Click {
                        target_description: "Name field".into(),
                        x: 100,
                        y: 200,
                        detection_method: "exact".into(),
                    },
                    screenshot_before_path: None,
                    screenshot_after_path: None,
                    phash_before: None,
                    phash_after: None,
                    context_before: None,
                    context_after: None,
                    duration_ms: 50,
                    verified: true,
                    retries: 0,
                    detection_method: "omniparser(exact)".into(),
                    failure_reason: None,
                },
                WorkflowStep {
                    index: 1,
                    action: RecordedAction::Type {
                        target_description: "Name field".into(),
                        text: "John Doe".into(),
                        is_parameter: false,
                    },
                    screenshot_before_path: None,
                    screenshot_after_path: None,
                    phash_before: None,
                    phash_after: None,
                    context_before: None,
                    context_after: None,
                    duration_ms: 100,
                    verified: true,
                    retries: 0,
                    detection_method: "keyboard".into(),
                    failure_reason: None,
                },
                WorkflowStep {
                    index: 2,
                    action: RecordedAction::KeyPress { key: "Tab".into() },
                    screenshot_before_path: None,
                    screenshot_after_path: None,
                    phash_before: None,
                    phash_after: None,
                    context_before: None,
                    context_after: None,
                    duration_ms: 30,
                    verified: true,
                    retries: 0,
                    detection_method: "keyboard".into(),
                    failure_reason: None,
                },
            ],
            total_duration_ms: 180,
        }
    }

    #[test]
    fn test_generalize_creates_template() {
        let recording = make_recording();
        let template = WorkflowGeneralizer::generalize("Test Workflow", &recording);
        assert_eq!(template.name, "Test Workflow");
        assert_eq!(template.steps.len(), 3);
        assert_eq!(template.source_recording_id, "test-recording");
    }

    #[test]
    fn test_detect_parameters() {
        let recording = make_recording();
        let params = WorkflowGeneralizer::detect_parameters(&recording.steps);
        // "John Doe" should be detected as a parameter
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "Name field");
        assert_eq!(params[0].default_value, "John Doe");
        assert_eq!(params[0].step_index, 1);
    }

    #[test]
    fn test_single_char_not_parameter() {
        let steps = vec![WorkflowStep {
            index: 0,
            action: RecordedAction::Type {
                target_description: "Search".into(),
                text: "x".into(),
                is_parameter: false,
            },
            screenshot_before_path: None,
            screenshot_after_path: None,
            phash_before: None,
            phash_after: None,
            context_before: None,
            context_after: None,
            duration_ms: 10,
            verified: true,
            retries: 0,
            detection_method: "keyboard".into(),
            failure_reason: None,
        }];
        let params = WorkflowGeneralizer::detect_parameters(&steps);
        assert!(params.is_empty());
    }

    #[test]
    fn test_template_step_types() {
        let recording = make_recording();
        let template = WorkflowGeneralizer::generalize("Test", &recording);
        assert!(matches!(
            template.steps[0].action,
            TemplateAction::Click { .. }
        ));
        assert!(matches!(
            template.steps[1].action,
            TemplateAction::Type { .. }
        ));
        assert!(matches!(
            template.steps[2].action,
            TemplateAction::KeyPress { .. }
        ));
    }

    #[test]
    fn test_template_serialization() {
        let recording = make_recording();
        let template = WorkflowGeneralizer::generalize("Test", &recording);
        let json = serde_json::to_string(&template).unwrap();
        let deserialized: WorkflowTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "Test");
        assert_eq!(deserialized.steps.len(), 3);
    }

    #[test]
    fn test_template_summary() {
        let summary = WorkflowTemplateSummary {
            id: "abc".into(),
            name: "Test".into(),
            step_count: 5,
            parameter_count: 2,
            use_count: 10,
            success_rate: 0.95,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let deserialized: WorkflowTemplateSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.step_count, 5);
    }
}
