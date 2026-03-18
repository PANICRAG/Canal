//! Workflow Engine
//!
//! This module provides workflow execution capabilities for orchestrating
//! multi-step AI workflows with DAG-based parallel execution, pause/resume,
//! and checkpoint/recovery support.
//!
//! ## Workflow Recording
//!
//! The `recorder` submodule provides capabilities to record user actions
//! and generate reusable workflow templates:
//!
//! ```rust,ignore
//! use gateway_core::workflow::{WorkflowRecorder, WorkflowTemplate};
//!
//! let recorder = WorkflowRecorder::new();
//! let session_id = recorder.start_recording("My Workflow".to_string(), None).await;
//!
//! // Record actions as they happen...
//! recorder.record_action("tool_name".to_string(), params, result, true, None).await;
//!
//! // Stop and create template
//! let session = recorder.stop_recording().await.unwrap();
//! let template = recorder.analyze_and_create_template(&session).await;
//! ```

pub mod checkpoint;
pub mod dag;
pub mod engine;
pub mod executor;
pub mod recorder;
pub use checkpoint::{Checkpoint, CheckpointStore};
pub use dag::{DagExecutor, DagNode};
pub use engine::{
    ExecutionStatus, StepType, WorkflowDefinition, WorkflowEngine, WorkflowExecution, WorkflowStep,
};
pub use executor::{StepContext, StepResult, WorkflowExecutor};
pub use recorder::{
    ActionType,
    ConditionType,
    DetectedPattern,
    ExpectedOutcome,
    OutcomeType,
    ParameterType,
    // Pattern Engine
    PatternEngine,
    PatternType,
    RecordedAction,
    RecordingMetadata,
    RecordingSession,
    RecordingStatus,
    StepCondition,
    TemplateParameter,
    TemplateStep,
    // Recording
    WorkflowRecorder,
    // Templates
    WorkflowTemplate,
};
