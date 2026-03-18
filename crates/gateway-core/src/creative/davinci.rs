//! DaVinci Resolve adapter
//!
//! Connects to DaVinci Resolve via its Python scripting API.
//! Requires DaVinci Resolve to be running with scripting enabled.

use super::adapter::{AdapterCapabilities, AdapterError, CreativeAdapter};
use super::types::*;
use super::{Application, OperationResult};
use async_trait::async_trait;
use std::collections::HashSet;

/// DaVinci Resolve adapter using Python API
pub struct DaVinciAdapter {
    capabilities: AdapterCapabilities,
    connected: bool,
    project_name: Option<String>,
}

impl DaVinciAdapter {
    pub fn new() -> Self {
        Self {
            capabilities: Self::build_capabilities(),
            connected: false,
            project_name: None,
        }
    }

    fn build_capabilities() -> AdapterCapabilities {
        let mut timeline_ops = HashSet::new();
        timeline_ops.insert("get_clips".to_string());
        timeline_ops.insert("add_clip".to_string());
        timeline_ops.insert("delete_clip".to_string());
        timeline_ops.insert("trim_clip".to_string());
        timeline_ops.insert("add_marker".to_string());
        timeline_ops.insert("get_markers".to_string());

        let mut color_ops = HashSet::new();
        color_ops.insert("add_node".to_string());
        color_ops.insert("apply_lut".to_string());
        color_ops.insert("apply_powergrade".to_string());
        color_ops.insert("apply_color_wheels".to_string());
        color_ops.insert("apply_primaries".to_string());
        color_ops.insert("apply_curves".to_string());
        color_ops.insert("copy_grade".to_string());

        let mut audio_ops = HashSet::new();
        audio_ops.insert("analyze_audio".to_string());
        audio_ops.insert("normalize_audio".to_string());
        audio_ops.insert("apply_audio_gain".to_string());

        let mut export_ops = HashSet::new();
        export_ops.insert("export".to_string());
        export_ops.insert("get_export_progress".to_string());
        export_ops.insert("cancel_export".to_string());

        let mut parallel_safe = HashSet::new();
        parallel_safe.insert("get_clips".to_string());
        parallel_safe.insert("get_clip".to_string());
        parallel_safe.insert("get_markers".to_string());
        parallel_safe.insert("analyze_audio".to_string());
        parallel_safe.insert("get_color_nodes".to_string());

        AdapterCapabilities {
            application: Application::DaVinciResolve,
            version: None,
            timeline_operations: timeline_ops,
            color_operations: color_ops,
            audio_operations: audio_ops,
            export_operations: export_ops,
            supports_scripting: true,
            supports_remote_api: true,
            supports_batch_operations: true,
            supports_undo: true,
            parallel_safe_operations: parallel_safe,
        }
    }

    /// Execute a Python script via DaVinci's scripting interface
    async fn execute_python(&self, _script: &str) -> Result<serde_json::Value, AdapterError> {
        // In production, this would:
        // 1. Connect to DaVinci's Python scripting API
        // 2. Execute the script
        // 3. Return the result
        //
        // For now, return a placeholder
        Err(AdapterError::NotSupported(
            "Python scripting integration not yet implemented".to_string(),
        ))
    }
}

impl Default for DaVinciAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CreativeAdapter for DaVinciAdapter {
    fn application(&self) -> Application {
        Application::DaVinciResolve
    }

    fn capabilities(&self) -> &AdapterCapabilities {
        &self.capabilities
    }

    async fn is_running(&self) -> bool {
        // Check if DaVinci Resolve process is running
        #[cfg(target_os = "macos")]
        {
            // Use pgrep on macOS
            if let Ok(output) = tokio::process::Command::new("pgrep")
                .arg("-x")
                .arg("Resolve")
                .output()
                .await
            {
                return output.status.success();
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Use tasklist on Windows
            if let Ok(output) = tokio::process::Command::new("tasklist")
                .args(["/FI", "IMAGENAME eq Resolve.exe"])
                .output()
                .await
            {
                return String::from_utf8_lossy(&output.stdout).contains("Resolve.exe");
            }
        }

        false
    }

    async fn connect(&mut self) -> Result<(), AdapterError> {
        if !self.is_running().await {
            return Err(AdapterError::NotRunning(
                "DaVinci Resolve is not running".to_string(),
            ));
        }

        // In production, connect to DaVinci's scripting API
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), AdapterError> {
        self.connected = false;
        self.project_name = None;
        Ok(())
    }

    async fn get_project_info(&self) -> Result<ProjectInfo, AdapterError> {
        // Would execute: resolve.GetProjectManager().GetCurrentProject()
        Err(AdapterError::NotSupported(
            "get_project_info not yet implemented".to_string(),
        ))
    }

    async fn get_timelines(&self) -> Result<Vec<TimelineInfo>, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_timelines not yet implemented".to_string(),
        ))
    }

    async fn get_active_timeline(&self) -> Result<TimelineInfo, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_active_timeline not yet implemented".to_string(),
        ))
    }

    async fn get_clips(
        &self,
        _timeline_id: &TimelineId,
        _filter: Option<ClipFilter>,
    ) -> Result<Vec<ClipInfo>, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_clips not yet implemented".to_string(),
        ))
    }

    async fn get_clip(&self, _clip_id: &ClipId) -> Result<ClipInfo, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_clip not yet implemented".to_string(),
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
            "add_clip not yet implemented".to_string(),
        ))
    }

    async fn delete_clip(&self, _clip_id: &ClipId) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "delete_clip not yet implemented".to_string(),
        ))
    }

    async fn trim_clip(
        &self,
        _clip_id: &ClipId,
        _new_start: Timecode,
        _new_end: Timecode,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "trim_clip not yet implemented".to_string(),
        ))
    }

    async fn get_color_nodes(&self, _clip_id: &ClipId) -> Result<Vec<NodeId>, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_color_nodes not yet implemented".to_string(),
        ))
    }

    async fn add_color_node(
        &self,
        _clip_id: &ClipId,
        _node_type: NodeType,
    ) -> Result<NodeId, AdapterError> {
        Err(AdapterError::NotSupported(
            "add_color_node not yet implemented".to_string(),
        ))
    }

    async fn apply_lut(&self, _clip_id: &ClipId, _lut_path: &str) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "apply_lut not yet implemented".to_string(),
        ))
    }

    async fn apply_color_wheels(
        &self,
        _clip_id: &ClipId,
        _node_id: &NodeId,
        _adjustments: &ColorWheelAdjustments,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "apply_color_wheels not yet implemented".to_string(),
        ))
    }

    async fn apply_primaries(
        &self,
        _clip_id: &ClipId,
        _node_id: &NodeId,
        _corrections: &PrimaryCorrections,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "apply_primaries not yet implemented".to_string(),
        ))
    }

    async fn apply_curves(
        &self,
        _clip_id: &ClipId,
        _node_id: &NodeId,
        _curves: &RgbCurves,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "apply_curves not yet implemented".to_string(),
        ))
    }

    async fn copy_grade(
        &self,
        _source_clip: &ClipId,
        _target_clips: &[ClipId],
    ) -> Result<u32, AdapterError> {
        Err(AdapterError::NotSupported(
            "copy_grade not yet implemented".to_string(),
        ))
    }

    async fn analyze_audio(&self, _clip_id: &ClipId) -> Result<AudioAnalysis, AdapterError> {
        Err(AdapterError::NotSupported(
            "analyze_audio not yet implemented".to_string(),
        ))
    }

    async fn normalize_audio(
        &self,
        _clip_id: &ClipId,
        _settings: &AudioNormalization,
    ) -> Result<OperationResult, AdapterError> {
        Err(AdapterError::NotSupported(
            "normalize_audio not yet implemented".to_string(),
        ))
    }

    async fn apply_audio_gain(&self, _clip_id: &ClipId, _gain_db: f32) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "apply_audio_gain not yet implemented".to_string(),
        ))
    }

    async fn export(
        &self,
        _timeline_id: &TimelineId,
        _settings: &ExportSettings,
    ) -> Result<String, AdapterError> {
        Err(AdapterError::NotSupported(
            "export not yet implemented".to_string(),
        ))
    }

    async fn get_export_progress(&self) -> Result<f32, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_export_progress not yet implemented".to_string(),
        ))
    }

    async fn cancel_export(&self) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "cancel_export not yet implemented".to_string(),
        ))
    }

    async fn add_marker(&self, _clip_id: &ClipId, _marker: &Marker) -> Result<String, AdapterError> {
        Err(AdapterError::NotSupported(
            "add_marker not yet implemented".to_string(),
        ))
    }

    async fn get_markers(&self, _clip_id: &ClipId) -> Result<Vec<Marker>, AdapterError> {
        Err(AdapterError::NotSupported(
            "get_markers not yet implemented".to_string(),
        ))
    }

    async fn delete_marker(&self, _marker_id: &str) -> Result<(), AdapterError> {
        Err(AdapterError::NotSupported(
            "delete_marker not yet implemented".to_string(),
        ))
    }

    async fn execute_operation(
        &self,
        operation: &str,
        params: serde_json::Value,
    ) -> Result<OperationResult, AdapterError> {
        // Route to appropriate method based on operation name
        match operation {
            "get_project_info" => {
                let _info = self.get_project_info().await?;
                Ok(OperationResult::success(operation))
            }
            "get_clips" => {
                let timeline_id = params
                    .get("timeline_id")
                    .and_then(|v| v.as_str())
                    .map(|s| TimelineId(s.to_string()))
                    .unwrap_or(TimelineId("current".to_string()));
                let _clips = self.get_clips(&timeline_id, None).await?;
                Ok(OperationResult::success(operation))
            }
            _ => Err(AdapterError::NotSupported(format!(
                "Operation '{}' not supported",
                operation
            ))),
        }
    }
}
