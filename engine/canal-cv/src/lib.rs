//! canal-cv — Computer Vision engine for screen automation.
//!
//! This crate provides the core CV abstractions and implementations
//! for screen capture, vision detection, action pipelines, workflow
//! recording/replay, and screen monitoring.
//!
//! # Architecture
//!
//! ```text
//! ScreenController trait
//!   ├── NoopScreenController (this crate)
//!   ├── BrowserScreenController (gateway-core adapter)
//!   └── [future] DesktopScreenController
//!
//! VisionDetector trait
//!   ├── MolmoDetector (this crate)
//!   ├── UiTarsDetector (gateway-core adapter)
//!   └── FallbackDetector (this crate, composition)
//!
//! CvLlmClient trait
//!   └── GatewayCoreLlmClient (gateway-core adapter)
//! ```

// -- Core types and traits --
pub mod box_detector;
pub mod change_detector;
pub mod llm_client;
pub mod phash;
pub mod screen_controller;
pub mod types;
pub mod vision_detector;

// -- Vision detection --
pub mod molmo_detect;
pub mod molmo_parser;
pub mod molmo_provider;
pub mod omniparser_detector;
pub mod vision_pipeline;

// -- Narration & pipeline --
pub mod observation_narrator;
pub mod pipeline;

// -- Action execution --
pub mod action_chain;

// -- Screen monitoring --
pub mod monitor_events;
pub mod screen_monitor;

// -- Workflow recording/replay --
pub mod workflow_recorder;
pub mod workflow_replayer;
pub mod workflow_store;
pub mod workflow_template;

// -- Learning integration --
pub mod cv_experience_adapter;

// -- Re-exports --
pub use action_chain::{
    is_computer_use_tool, is_mutating_tool, ActionChainExecutor, ChainConfig, COMPUTER_USE_TOOLS,
};
pub use box_detector::{BoundingBox, BoxDetector};
pub use change_detector::{ChangeDetectionConfig, ScreenChangeDetector};
pub use cv_experience_adapter::{CvExperienceAdapter, NarrativeMemory, SubtaskExperience};
pub use llm_client::{
    CvChatRequest, CvChatResponse, CvContent, CvLlmClient, CvLlmError, CvMessage,
};
pub use molmo_detect::MolmoDetector;
pub use molmo_parser::{MolmoParseError, MolmoParseResult, MolmoParsedPoint, MolmoParser};
pub use molmo_provider::{
    MolmoClickResult, MolmoMultiPointResult, MolmoProvider, MolmoProviderConfig,
};
pub use monitor_events::{ChangeType, MonitoredState, ScreenChangeEvent};
pub use observation_narrator::{ActionObservation, NarrationConfig, ObservationNarrator};
pub use omniparser_detector::{OmniParserConfig, OmniParserDetector};
pub use phash::{compute_phash, hash_similarity};
pub use pipeline::{
    ActResult, ActionType, ComputerUsePipeline, ObserveResult, ParsedInstruction, PipelineConfig,
};
pub use screen_controller::{NoopScreenController, ScreenController};
pub use screen_monitor::{ScreenMonitor, ScreenMonitorConfig};
pub use types::*;
pub use vision_detector::{DetectionInput, DetectionResult, VisionDetector};
pub use vision_pipeline::{FallbackDetector, VisionPipeline, VisionPipelineConfig};
pub use workflow_recorder::{RecordedAction, WorkflowRecorder, WorkflowRecording, WorkflowStep};
pub use workflow_replayer::{ReplayResult, StepReplayResult, WorkflowReplayer};
pub use workflow_store::{InMemoryWorkflowStore, JsonWorkflowStore, WorkflowStore};
pub use workflow_template::{
    TemplateAction, TemplateStep, WorkflowGeneralizer, WorkflowParameter, WorkflowTemplate,
    WorkflowTemplateSummary,
};
