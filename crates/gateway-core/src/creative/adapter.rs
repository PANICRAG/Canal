//! Creative tool adapter trait and error types

use super::types::*;
use super::{Application, OperationResult, ToolCategory};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use thiserror::Error;

/// Errors from creative tool adapters
#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("Application not running: {0}")]
    NotRunning(String),

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Operation not supported: {0}")]
    NotSupported(String),

    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Operation timeout: {0}")]
    Timeout(String),

    #[error("API error: {0}")]
    ApiError(String),

    #[error("Script error: {0}")]
    ScriptError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl AdapterError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            AdapterError::ConnectionFailed(_) | AdapterError::Timeout(_)
        )
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            AdapterError::NotRunning(_) => "NOT_RUNNING",
            AdapterError::ConnectionFailed(_) => "CONNECTION_FAILED",
            AdapterError::NotSupported(_) => "NOT_SUPPORTED",
            AdapterError::InvalidParameters(_) => "INVALID_PARAMS",
            AdapterError::NotFound(_) => "NOT_FOUND",
            AdapterError::PermissionDenied(_) => "PERMISSION_DENIED",
            AdapterError::Timeout(_) => "TIMEOUT",
            AdapterError::ApiError(_) => "API_ERROR",
            AdapterError::ScriptError(_) => "SCRIPT_ERROR",
            AdapterError::Internal(_) => "INTERNAL",
        }
    }
}

/// Adapter capabilities description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterCapabilities {
    pub application: Application,
    pub version: Option<String>,

    /// Supported operations
    pub timeline_operations: HashSet<String>,
    pub color_operations: HashSet<String>,
    pub audio_operations: HashSet<String>,
    pub export_operations: HashSet<String>,

    /// Feature flags
    pub supports_scripting: bool,
    pub supports_remote_api: bool,
    pub supports_batch_operations: bool,
    pub supports_undo: bool,

    /// Parallel execution capabilities
    pub parallel_safe_operations: HashSet<String>,
}

impl AdapterCapabilities {
    pub fn supports_operation(&self, operation: &str) -> bool {
        self.timeline_operations.contains(operation)
            || self.color_operations.contains(operation)
            || self.audio_operations.contains(operation)
            || self.export_operations.contains(operation)
    }

    pub fn can_parallel(&self, operation: &str) -> bool {
        self.parallel_safe_operations.contains(operation)
    }
}

/// Trait for creative tool adapters
#[async_trait]
pub trait CreativeAdapter: Send + Sync {
    /// Get the application this adapter supports
    fn application(&self) -> Application;

    /// Get adapter capabilities
    fn capabilities(&self) -> &AdapterCapabilities;

    /// Check if the application is running
    async fn is_running(&self) -> bool;

    /// Connect to the application
    async fn connect(&mut self) -> Result<(), AdapterError>;

    /// Disconnect from the application
    async fn disconnect(&mut self) -> Result<(), AdapterError>;

    // === Project Operations ===

    /// Get current project info
    async fn get_project_info(&self) -> Result<ProjectInfo, AdapterError>;

    /// Get all timelines in the project
    async fn get_timelines(&self) -> Result<Vec<TimelineInfo>, AdapterError>;

    /// Get the active timeline
    async fn get_active_timeline(&self) -> Result<TimelineInfo, AdapterError>;

    // === Timeline Operations ===

    /// Get clips in a timeline
    async fn get_clips(
        &self,
        timeline_id: &TimelineId,
        filter: Option<ClipFilter>,
    ) -> Result<Vec<ClipInfo>, AdapterError>;

    /// Get a specific clip by ID
    async fn get_clip(&self, clip_id: &ClipId) -> Result<ClipInfo, AdapterError>;

    /// Add a clip to the timeline
    async fn add_clip(
        &self,
        timeline_id: &TimelineId,
        source_path: &str,
        position: Timecode,
        track: u32,
    ) -> Result<ClipId, AdapterError>;

    /// Delete a clip
    async fn delete_clip(&self, clip_id: &ClipId) -> Result<(), AdapterError>;

    /// Trim a clip
    async fn trim_clip(
        &self,
        clip_id: &ClipId,
        new_start: Timecode,
        new_end: Timecode,
    ) -> Result<(), AdapterError>;

    // === Color Operations ===

    /// Get color nodes for a clip
    async fn get_color_nodes(&self, clip_id: &ClipId) -> Result<Vec<NodeId>, AdapterError>;

    /// Add a color node
    async fn add_color_node(
        &self,
        clip_id: &ClipId,
        node_type: NodeType,
    ) -> Result<NodeId, AdapterError>;

    /// Apply LUT to a clip
    async fn apply_lut(&self, clip_id: &ClipId, lut_path: &str) -> Result<(), AdapterError>;

    /// Apply color wheel adjustments
    async fn apply_color_wheels(
        &self,
        clip_id: &ClipId,
        node_id: &NodeId,
        adjustments: &ColorWheelAdjustments,
    ) -> Result<(), AdapterError>;

    /// Apply primary corrections
    async fn apply_primaries(
        &self,
        clip_id: &ClipId,
        node_id: &NodeId,
        corrections: &PrimaryCorrections,
    ) -> Result<(), AdapterError>;

    /// Apply RGB curves
    async fn apply_curves(
        &self,
        clip_id: &ClipId,
        node_id: &NodeId,
        curves: &RgbCurves,
    ) -> Result<(), AdapterError>;

    /// Copy color grade from one clip to another
    async fn copy_grade(
        &self,
        source_clip: &ClipId,
        target_clips: &[ClipId],
    ) -> Result<u32, AdapterError>;

    // === Audio Operations ===

    /// Analyze audio levels
    async fn analyze_audio(&self, clip_id: &ClipId) -> Result<AudioAnalysis, AdapterError>;

    /// Normalize audio
    async fn normalize_audio(
        &self,
        clip_id: &ClipId,
        settings: &AudioNormalization,
    ) -> Result<OperationResult, AdapterError>;

    /// Apply audio gain
    async fn apply_audio_gain(&self, clip_id: &ClipId, gain_db: f32) -> Result<(), AdapterError>;

    // === Export Operations ===

    /// Export timeline
    async fn export(
        &self,
        timeline_id: &TimelineId,
        settings: &ExportSettings,
    ) -> Result<String, AdapterError>;

    /// Get export progress
    async fn get_export_progress(&self) -> Result<f32, AdapterError>;

    /// Cancel export
    async fn cancel_export(&self) -> Result<(), AdapterError>;

    // === Marker Operations ===

    /// Add marker to clip
    async fn add_marker(
        &self,
        clip_id: &ClipId,
        marker: &Marker,
    ) -> Result<String, AdapterError>;

    /// Get markers for clip
    async fn get_markers(&self, clip_id: &ClipId) -> Result<Vec<Marker>, AdapterError>;

    /// Delete marker
    async fn delete_marker(&self, marker_id: &str) -> Result<(), AdapterError>;

    // === Utility ===

    /// Get the tool category for an operation
    fn get_tool_category(&self, operation: &str) -> ToolCategory {
        match operation {
            // Read-only operations
            "get_project_info" | "get_timelines" | "get_clips" | "get_clip"
            | "get_color_nodes" | "get_markers" | "analyze_audio" | "get_export_progress" => {
                ToolCategory::ReadOnly
            }

            // Reversible operations
            "add_clip" | "trim_clip" | "add_color_node" | "apply_lut"
            | "apply_color_wheels" | "apply_primaries" | "apply_curves" | "copy_grade"
            | "normalize_audio" | "apply_audio_gain" | "add_marker" => {
                ToolCategory::Reversible
            }

            // Sensitive operations
            "delete_clip" | "delete_marker" | "export" | "cancel_export" => {
                ToolCategory::Sensitive
            }

            // Default to reversible
            _ => ToolCategory::Reversible,
        }
    }

    /// Execute a named operation with JSON parameters
    async fn execute_operation(
        &self,
        operation: &str,
        params: serde_json::Value,
    ) -> Result<OperationResult, AdapterError>;
}
