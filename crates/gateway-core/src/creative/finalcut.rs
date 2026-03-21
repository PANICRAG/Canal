//! Final Cut Pro adapter
//!
//! Connects to Final Cut Pro via AppleScript and FCPXML.
//! macOS only.

use super::adapter::{AdapterCapabilities, AdapterError, CreativeAdapter};
use super::types::*;
use super::{Application, OperationResult};
use async_trait::async_trait;
use std::collections::HashSet;

/// Final Cut Pro adapter using AppleScript
pub struct FinalCutAdapter {
    capabilities: AdapterCapabilities,
    connected: bool,
}

impl FinalCutAdapter {
    pub fn new() -> Self {
        Self {
            capabilities: Self::build_capabilities(),
            connected: false,
        }
    }

    fn build_capabilities() -> AdapterCapabilities {
        let mut timeline_ops = HashSet::new();
        timeline_ops.insert("get_clips".to_string());
        timeline_ops.insert("add_clip".to_string());
        timeline_ops.insert("delete_clip".to_string());
        timeline_ops.insert("add_marker".to_string());

        let mut color_ops = HashSet::new();
        color_ops.insert("color_board_adjust".to_string());
        color_ops.insert("apply_color_preset".to_string());

        let mut audio_ops = HashSet::new();
        audio_ops.insert("normalize_audio".to_string());

        let mut export_ops = HashSet::new();
        export_ops.insert("export".to_string());
        export_ops.insert("export_xml".to_string());

        let mut parallel_safe = HashSet::new();
        parallel_safe.insert("get_clips".to_string());

        AdapterCapabilities {
            application: Application::FinalCutPro,
            version: None,
            timeline_operations: timeline_ops,
            color_operations: color_ops,
            audio_operations: audio_ops,
            export_operations: export_ops,
            supports_scripting: true, // AppleScript
            supports_remote_api: false,
            supports_batch_operations: true, // Via FCPXML
            supports_undo: true,
            parallel_safe_operations: parallel_safe,
        }
    }

    /// Execute an AppleScript command
    #[cfg(target_os = "macos")]
    async fn execute_applescript(&self, script: &str) -> Result<String, AdapterError> {
        let output = tokio::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .await
            .map_err(|e| AdapterError::ScriptError(e.to_string()))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(AdapterError::ScriptError(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ))
        }
    }

    #[cfg(not(target_os = "macos"))]
    async fn execute_applescript(&self, _script: &str) -> Result<String, AdapterError> {
        Err(AdapterError::NotSupported(
            "AppleScript is only available on macOS".to_string(),
        ))
    }
}

impl Default for FinalCutAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CreativeAdapter for FinalCutAdapter {
    fn application(&self) -> Application {
        Application::FinalCutPro
    }

    fn capabilities(&self) -> &AdapterCapabilities {
        &self.capabilities
    }

    async fn is_running(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = tokio::process::Command::new("pgrep")
                .arg("-x")
                .arg("Final Cut Pro")
                .output()
                .await
            {
                return output.status.success();
            }
        }

        // FCP is macOS only
        false
    }

    async fn connect(&mut self) -> Result<(), AdapterError> {
        #[cfg(not(target_os = "macos"))]
        {
            return Err(AdapterError::NotSupported(
                "Final Cut Pro is only available on macOS".to_string(),
            ));
        }

        #[cfg(target_os = "macos")]
        {
            if !self.is_running().await {
                return Err(AdapterError::NotRunning(
                    "Final Cut Pro is not running".to_string(),
                ));
            }
            self.connected = true;
            Ok(())
        }
    }

    async fn disconnect(&mut self) -> Result<(), AdapterError> {
        self.connected = false;
        Ok(())
    }

    async fn get_project_info(&self) -> Result<ProjectInfo, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_project_info not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn get_timelines(&self) -> Result<Vec<TimelineInfo>, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_timelines not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn get_active_timeline(&self) -> Result<TimelineInfo, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_active_timeline not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn get_clips(
        &self,
        _timeline_id: &TimelineId,
        _filter: Option<ClipFilter>,
    ) -> Result<Vec<ClipInfo>, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_clips not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn get_clip(&self, _clip_id: &ClipId) -> Result<ClipInfo, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_clip not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn add_clip(
        &self,
        _timeline_id: &TimelineId,
        _source_path: &str,
        _position: Timecode,
        _track: u32,
    ) -> Result<ClipId, AdapterError> {
        Err(AdapterError::NotSupported(
            "add_clip not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn delete_clip(&self, _clip_id: &ClipId) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "delete_clip not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn trim_clip(
        &self,
        _clip_id: &ClipId,
        _new_start: Timecode,
        _new_end: Timecode,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "trim_clip not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn get_color_nodes(&self, _clip_id: &ClipId) -> Result<Vec<NodeId>, AdapterError> {
        // FCP doesn't have nodes
        Ok(vec![])
    }

    async fn add_color_node(
        &self,
        _clip_id: &ClipId,
        _node_type: NodeType,
    ) -> Result<NodeId, AdapterError> {
        Err(AdapterError::NotSupported(
            "Final Cut Pro uses Color Board instead of nodes".to_string(),
        ))
    }

    async fn apply_lut(&self, _clip_id: &ClipId, _lut_path: &str) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "apply_lut not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn apply_color_wheels(
        &self,
        _clip_id: &ClipId,
        _node_id: &NodeId,
        _adjustments: &ColorWheelAdjustments,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "apply_color_wheels not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn apply_primaries(
        &self,
        _clip_id: &ClipId,
        _node_id: &NodeId,
        _corrections: &PrimaryCorrections,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "apply_primaries not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn apply_curves(
        &self,
        _clip_id: &ClipId,
        _node_id: &NodeId,
        _curves: &RgbCurves,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "apply_curves not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn copy_grade(
        &self,
        _source_clip: &ClipId,
        _target_clips: &[ClipId],
    ) -> Result<u32, AdapterError> {
        Err(AdapterError::NotSupported(
            "copy_grade not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn analyze_audio(&self, _clip_id: &ClipId) -> Result<AudioAnalysis, AdapterError> {
        Err(AdapterError::NotSupported(
            "analyze_audio not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn normalize_audio(
        &self,
        _clip_id: &ClipId,
        _settings: &AudioNormalization,
    ) -> Result<OperationResult, AdapterError> {
        Err(AdapterError::NotSupported(
            "normalize_audio not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn apply_audio_gain(&self, _clip_id: &ClipId, _gain_db: f32) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "apply_audio_gain not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn export(
        &self,
        _timeline_id: &TimelineId,
        _settings: &ExportSettings,
    ) -> Result<String, AdapterError> {
        Err(AdapterError::NotSupported(
            "export not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn get_export_progress(&self) -> Result<f32, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_export_progress not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn cancel_export(&self) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "cancel_export not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn add_marker(&self, _clip_id: &ClipId, _marker: &Marker) -> Result<String, AdapterError> {
        Err(AdapterError::NotSupported(
            "add_marker not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn get_markers(&self, _clip_id: &ClipId) -> Result<Vec<Marker>, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_markers not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn delete_marker(&self, _marker_id: &str) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "delete_marker not yet implemented for Final Cut Pro".to_string(),
        ))
    }

    async fn execute_operation(
        &self,
        operation: &str,
        _params: serde_json::Value,
    ) -> Result<OperationResult, AdapterError> {
        Err(AdapterError::NotSupported(format!(
            "Operation '{}' not yet implemented for Final Cut Pro",
            operation
        )))
    }
}
