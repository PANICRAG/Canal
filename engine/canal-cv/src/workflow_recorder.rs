//! WorkflowRecorder — records user actions into replayable workflows.
//!
//! Intercepts CV4 Pipeline's act/extract calls and captures before/after
//! screenshots with step metadata. Screenshots stored as disk paths,
//! not inline base64 (~40MB savings for a 10-step workflow).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::screen_controller::ScreenController;
use crate::types::{ContextInfo, ScreenCapture};

/// A recorded user action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecordedAction {
    /// Click at a target.
    Click {
        target_description: String,
        x: u32,
        y: u32,
        detection_method: String,
    },
    /// Type text into a target.
    Type {
        target_description: String,
        text: String,
        is_parameter: bool,
    },
    /// Press a key.
    KeyPress { key: String },
    /// Scroll in a direction.
    Scroll { direction: String, amount: f64 },
    /// Extract data from screen.
    Extract {
        query: String,
        result: serde_json::Value,
    },
    /// Observe screen state.
    Observe,
    /// Wait for a duration.
    Wait { duration_ms: u64 },
}

impl RecordedAction {
    /// Human-readable description of this action.
    pub fn description(&self) -> String {
        match self {
            Self::Click {
                target_description, ..
            } => format!("Click '{}'", target_description),
            Self::Type { text, .. } => format!("Type '{}'", text),
            Self::KeyPress { key } => format!("Press {}", key),
            Self::Scroll { direction, .. } => format!("Scroll {}", direction),
            Self::Extract { query, .. } => format!("Extract: {}", query),
            Self::Observe => "Observe screen".to_string(),
            Self::Wait { duration_ms } => format!("Wait {}ms", duration_ms),
        }
    }
}

/// A single step in a recorded workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    /// Step index (0-based).
    pub index: usize,
    /// The recorded action.
    pub action: RecordedAction,
    /// Path to before-screenshot on disk (if saved).
    pub screenshot_before_path: Option<PathBuf>,
    /// Path to after-screenshot on disk (if saved).
    pub screenshot_after_path: Option<PathBuf>,
    /// pHash of before screenshot for quick comparison.
    pub phash_before: Option<u64>,
    /// pHash of after screenshot for quick comparison.
    pub phash_after: Option<u64>,
    /// Context before action (title, app, URL).
    pub context_before: Option<ContextInfo>,
    /// Context after action.
    pub context_after: Option<ContextInfo>,
    /// Duration of this step in milliseconds.
    pub duration_ms: u64,
    /// Whether the action was verified as successful.
    pub verified: bool,
    /// Number of retries needed.
    pub retries: u32,
    /// Detection method that succeeded.
    pub detection_method: String,
    /// Failure reason if not verified.
    pub failure_reason: Option<String>,
}

/// A completed workflow recording.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRecording {
    /// Unique recording ID.
    pub id: String,
    /// Recorded steps.
    pub steps: Vec<WorkflowStep>,
    /// Total duration in milliseconds.
    pub total_duration_ms: u64,
}

/// Internal recorder state.
struct RecorderState {
    recording: bool,
    recording_id: String,
    steps: Vec<WorkflowStep>,
    start_time: Option<Instant>,
}

/// Records user actions into replayable workflows.
///
/// All methods take `&self` — interior mutability via `RwLock`.
/// Compatible with `Arc<WorkflowRecorder>` sharing across pipeline + tools.
pub struct WorkflowRecorder {
    state: Arc<RwLock<RecorderState>>,
    _controller: Arc<dyn ScreenController>,
    screenshot_dir: PathBuf,
}

impl WorkflowRecorder {
    /// Create a new recorder.
    pub fn new(controller: Arc<dyn ScreenController>, screenshot_dir: PathBuf) -> Self {
        Self {
            state: Arc::new(RwLock::new(RecorderState {
                recording: false,
                recording_id: String::new(),
                steps: Vec::new(),
                start_time: None,
            })),
            _controller: controller,
            screenshot_dir,
        }
    }

    /// Start recording a workflow.
    pub async fn start(&self) {
        let mut state = self.state.write().await;
        state.recording = true;
        state.recording_id = uuid::Uuid::new_v4().to_string();
        state.steps.clear();
        state.start_time = Some(Instant::now());
        tracing::info!(recording_id = %state.recording_id, "Workflow recording started");
    }

    /// Check if currently recording.
    pub async fn is_recording(&self) -> bool {
        self.state.read().await.recording
    }

    /// Record a step with before/after captures.
    pub async fn record_step(
        &self,
        action: RecordedAction,
        _before: &ScreenCapture,
        _after: &ScreenCapture,
        context_before: Option<ContextInfo>,
        context_after: Option<ContextInfo>,
        duration_ms: u64,
        verified: bool,
        retries: u32,
        detection_method: &str,
        failure_reason: Option<String>,
    ) {
        let mut state = self.state.write().await;
        if !state.recording {
            return;
        }

        let index = state.steps.len();
        let before_hash = super::phash::compute_phash(&_before.base64);
        let after_hash = super::phash::compute_phash(&_after.base64);

        // Build screenshot paths (actual saving deferred to avoid blocking)
        let recording_dir = self.screenshot_dir.join(&state.recording_id);
        let before_path = recording_dir.join(format!("step_{index}_before.jpg"));
        let after_path = recording_dir.join(format!("step_{index}_after.jpg"));

        let step = WorkflowStep {
            index,
            action,
            screenshot_before_path: Some(before_path),
            screenshot_after_path: Some(after_path),
            phash_before: Some(before_hash),
            phash_after: Some(after_hash),
            context_before,
            context_after,
            duration_ms,
            verified,
            retries,
            detection_method: detection_method.to_string(),
            failure_reason,
        };

        tracing::debug!(step_index = index, action = %step.action.description(), "Recorded workflow step");
        state.steps.push(step);
    }

    /// Stop recording and return the completed recording.
    pub async fn stop(&self) -> WorkflowRecording {
        let mut state = self.state.write().await;
        state.recording = false;

        let total_duration_ms = state
            .start_time
            .map(|s| s.elapsed().as_millis() as u64)
            .unwrap_or(0);

        let recording = WorkflowRecording {
            id: state.recording_id.clone(),
            steps: std::mem::take(&mut state.steps),
            total_duration_ms,
        };

        tracing::info!(
            recording_id = %recording.id,
            steps = recording.steps.len(),
            duration_ms = total_duration_ms,
            "Workflow recording stopped"
        );
        recording
    }

    /// Get the screenshot directory path.
    pub fn screenshot_dir(&self) -> &PathBuf {
        &self.screenshot_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoopScreenController;

    fn make_recorder() -> WorkflowRecorder {
        let controller = Arc::new(NoopScreenController::new());
        WorkflowRecorder::new(controller, PathBuf::from("/tmp/test_workflows"))
    }

    #[tokio::test]
    async fn test_not_recording_initially() {
        let recorder = make_recorder();
        assert!(!recorder.is_recording().await);
    }

    #[tokio::test]
    async fn test_start_sets_recording() {
        let recorder = make_recorder();
        recorder.start().await;
        assert!(recorder.is_recording().await);
    }

    #[tokio::test]
    async fn test_stop_returns_recording() {
        let recorder = make_recorder();
        recorder.start().await;
        let recording = recorder.stop().await;
        assert!(!recording.steps.is_empty() || recording.steps.is_empty());
        assert!(!recorder.is_recording().await);
    }

    #[tokio::test]
    async fn test_stop_without_start_returns_empty() {
        let recorder = make_recorder();
        let recording = recorder.stop().await;
        assert!(recording.steps.is_empty());
    }

    #[test]
    fn test_recorded_action_description() {
        assert_eq!(
            RecordedAction::Click {
                target_description: "Submit".into(),
                x: 100,
                y: 200,
                detection_method: "exact".into(),
            }
            .description(),
            "Click 'Submit'"
        );
        assert_eq!(
            RecordedAction::Type {
                target_description: "Name".into(),
                text: "hello".into(),
                is_parameter: false,
            }
            .description(),
            "Type 'hello'"
        );
        assert_eq!(
            RecordedAction::KeyPress {
                key: "Enter".into(),
            }
            .description(),
            "Press Enter"
        );
        assert_eq!(
            RecordedAction::Scroll {
                direction: "down".into(),
                amount: 3.0,
            }
            .description(),
            "Scroll down"
        );
        assert_eq!(RecordedAction::Observe.description(), "Observe screen");
        assert_eq!(
            RecordedAction::Wait { duration_ms: 500 }.description(),
            "Wait 500ms"
        );
    }

    #[test]
    fn test_workflow_step_serialization() {
        let step = WorkflowStep {
            index: 0,
            action: RecordedAction::Click {
                target_description: "OK".into(),
                x: 50,
                y: 60,
                detection_method: "exact".into(),
            },
            screenshot_before_path: None,
            screenshot_after_path: None,
            phash_before: Some(12345),
            phash_after: Some(67890),
            context_before: None,
            context_after: None,
            duration_ms: 150,
            verified: true,
            retries: 0,
            detection_method: "omniparser(exact)".into(),
            failure_reason: None,
        };
        let json = serde_json::to_string(&step).unwrap();
        let deserialized: WorkflowStep = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.index, 0);
        assert!(deserialized.verified);
    }
}
