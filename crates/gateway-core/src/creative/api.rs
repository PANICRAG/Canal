//! Unified Creative Tool API
//!
//! Provides a high-level API that abstracts over different creative applications.

use super::adapter::{AdapterError, CreativeAdapter};
use super::davinci::DaVinciAdapter;
use super::finalcut::FinalCutAdapter;
use super::premiere::PremiereAdapter;
use super::types::*;
use super::{Application, OperationResult};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Creative tool manager - coordinates adapters and provides unified access
pub struct CreativeToolManager {
    adapters: HashMap<Application, Arc<RwLock<Box<dyn CreativeAdapter>>>>,
    active_app: Option<Application>,
}

impl CreativeToolManager {
    /// Create a new manager with all available adapters
    pub fn new() -> Self {
        let mut adapters: HashMap<Application, Arc<RwLock<Box<dyn CreativeAdapter>>>> =
            HashMap::new();

        // Register available adapters
        adapters.insert(
            Application::DaVinciResolve,
            Arc::new(RwLock::new(Box::new(DaVinciAdapter::new()))),
        );
        adapters.insert(
            Application::AdobePremiere,
            Arc::new(RwLock::new(Box::new(PremiereAdapter::new()))),
        );
        adapters.insert(
            Application::FinalCutPro,
            Arc::new(RwLock::new(Box::new(FinalCutAdapter::new()))),
        );

        Self {
            adapters,
            active_app: None,
        }
    }

    /// Detect which creative application is currently running
    pub async fn detect_active_application(&mut self) -> Result<Application, AdapterError> {
        for (app, adapter) in &self.adapters {
            let adapter = adapter.read().await;
            if adapter.is_running().await {
                self.active_app = Some(*app);
                return Ok(*app);
            }
        }
        Err(AdapterError::NotRunning(
            "No supported creative application is running".to_string(),
        ))
    }

    /// Get the active application
    pub fn active_application(&self) -> Option<Application> {
        self.active_app
    }

    /// Set the active application manually
    pub fn set_active_application(&mut self, app: Application) {
        self.active_app = Some(app);
    }

    /// Get adapter for a specific application
    pub fn get_adapter(
        &self,
        app: Application,
    ) -> Option<Arc<RwLock<Box<dyn CreativeAdapter>>>> {
        self.adapters.get(&app).cloned()
    }

    /// Get adapter for the active application
    pub fn get_active_adapter(&self) -> Option<Arc<RwLock<Box<dyn CreativeAdapter>>>> {
        self.active_app.and_then(|app| self.adapters.get(&app).cloned())
    }

    /// Connect to the active application
    pub async fn connect(&self) -> Result<(), AdapterError> {
        let adapter = self
            .get_active_adapter()
            .ok_or_else(|| AdapterError::NotRunning("No active application".to_string()))?;
        let mut adapter = adapter.write().await;
        adapter.connect().await
    }

    /// Get a unified API interface for the active application
    pub async fn api(&self) -> Result<UnifiedApi, AdapterError> {
        let adapter = self
            .get_active_adapter()
            .ok_or_else(|| AdapterError::NotRunning("No active application".to_string()))?;
        Ok(UnifiedApi::new(adapter))
    }
}

impl Default for CreativeToolManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Unified API for creative operations
pub struct UnifiedApi {
    adapter: Arc<RwLock<Box<dyn CreativeAdapter>>>,
}

impl UnifiedApi {
    pub fn new(adapter: Arc<RwLock<Box<dyn CreativeAdapter>>>) -> Self {
        Self { adapter }
    }

    /// Get timeline operations API
    pub fn timeline(&self) -> TimelineApi {
        TimelineApi {
            adapter: self.adapter.clone(),
        }
    }

    /// Get color operations API
    pub fn color(&self) -> ColorApi {
        ColorApi {
            adapter: self.adapter.clone(),
        }
    }

    /// Get audio operations API
    pub fn audio(&self) -> AudioApi {
        AudioApi {
            adapter: self.adapter.clone(),
        }
    }

    /// Get export operations API
    pub fn export(&self) -> ExportApi {
        ExportApi {
            adapter: self.adapter.clone(),
        }
    }

    /// Get project info
    pub async fn get_project_info(&self) -> Result<ProjectInfo, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.get_project_info().await
    }

    /// Execute a named operation
    pub async fn execute(
        &self,
        operation: &str,
        params: serde_json::Value,
    ) -> Result<OperationResult, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.execute_operation(operation, params).await
    }
}

/// Timeline operations API
pub struct TimelineApi {
    adapter: Arc<RwLock<Box<dyn CreativeAdapter>>>,
}

impl TimelineApi {
    /// Get all timelines
    pub async fn list(&self) -> Result<Vec<TimelineInfo>, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.get_timelines().await
    }

    /// Get active timeline
    pub async fn active(&self) -> Result<TimelineInfo, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.get_active_timeline().await
    }

    /// Get clips in the active timeline
    pub async fn get_clips(&self, filter: Option<ClipFilter>) -> Result<Vec<ClipInfo>, AdapterError> {
        let adapter = self.adapter.read().await;
        let timeline = adapter.get_active_timeline().await?;
        adapter.get_clips(&timeline.id, filter).await
    }

    /// Get clips by tag
    pub async fn get_clips_by_tag(&self, tag: &str) -> Result<Vec<ClipInfo>, AdapterError> {
        let filter = ClipFilter::default().with_tag(tag);
        self.get_clips(Some(filter)).await
    }

    /// Get a specific clip
    pub async fn get_clip(&self, clip_id: &ClipId) -> Result<ClipInfo, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.get_clip(clip_id).await
    }

    /// Add a clip to the timeline
    pub async fn add_clip(
        &self,
        source_path: &str,
        position: Timecode,
        track: u32,
    ) -> Result<ClipId, AdapterError> {
        let adapter = self.adapter.read().await;
        let timeline = adapter.get_active_timeline().await?;
        adapter.add_clip(&timeline.id, source_path, position, track).await
    }

    /// Delete a clip
    pub async fn delete_clip(&self, clip_id: &ClipId) -> Result<(), AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.delete_clip(clip_id).await
    }

    /// Trim a clip
    pub async fn trim_clip(
        &self,
        clip_id: &ClipId,
        new_start: Timecode,
        new_end: Timecode,
    ) -> Result<(), AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.trim_clip(clip_id, new_start, new_end).await
    }

    /// Add marker to a clip
    pub async fn add_marker(&self, clip_id: &ClipId, marker: &Marker) -> Result<String, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.add_marker(clip_id, marker).await
    }

    /// Get markers for a clip
    pub async fn get_markers(&self, clip_id: &ClipId) -> Result<Vec<Marker>, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.get_markers(clip_id).await
    }
}

/// Color grading operations API
pub struct ColorApi {
    adapter: Arc<RwLock<Box<dyn CreativeAdapter>>>,
}

impl ColorApi {
    /// Apply LUT to a clip
    pub async fn apply_lut(&self, clip_id: &ClipId, lut_path: &str) -> Result<(), AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.apply_lut(clip_id, lut_path).await
    }

    /// Add a color node
    pub async fn add_node(&self, clip_id: &ClipId, node_type: NodeType) -> Result<NodeId, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.add_color_node(clip_id, node_type).await
    }

    /// Apply color wheel adjustments
    pub async fn apply_wheels(
        &self,
        clip_id: &ClipId,
        node_id: &NodeId,
        adjustments: &ColorWheelAdjustments,
    ) -> Result<(), AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.apply_color_wheels(clip_id, node_id, adjustments).await
    }

    /// Apply primary corrections
    pub async fn apply_primaries(
        &self,
        clip_id: &ClipId,
        node_id: &NodeId,
        corrections: &PrimaryCorrections,
    ) -> Result<(), AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.apply_primaries(clip_id, node_id, corrections).await
    }

    /// Apply RGB curves
    pub async fn apply_curves(
        &self,
        clip_id: &ClipId,
        node_id: &NodeId,
        curves: &RgbCurves,
    ) -> Result<(), AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.apply_curves(clip_id, node_id, curves).await
    }

    /// Apply S-curve for contrast
    pub async fn apply_s_curve(
        &self,
        clip_id: &ClipId,
        intensity: f32,
    ) -> Result<NodeId, AdapterError> {
        let adapter = self.adapter.read().await;
        let node_id = adapter.add_color_node(clip_id, NodeType::Curves).await?;
        let curves = RgbCurves::s_curve(intensity);
        adapter.apply_curves(clip_id, &node_id, &curves).await?;
        Ok(node_id)
    }

    /// Copy grade from one clip to others
    pub async fn copy_grade(
        &self,
        source: &ClipId,
        targets: &[ClipId],
    ) -> Result<u32, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.copy_grade(source, targets).await
    }

    /// Get color nodes for a clip
    pub async fn get_nodes(&self, clip_id: &ClipId) -> Result<Vec<NodeId>, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.get_color_nodes(clip_id).await
    }
}

/// Audio operations API
pub struct AudioApi {
    adapter: Arc<RwLock<Box<dyn CreativeAdapter>>>,
}

impl AudioApi {
    /// Analyze audio levels
    pub async fn analyze(&self, clip_id: &ClipId) -> Result<AudioAnalysis, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.analyze_audio(clip_id).await
    }

    /// Normalize audio to target LUFS
    pub async fn normalize(
        &self,
        clip_id: &ClipId,
        target_lufs: f32,
    ) -> Result<OperationResult, AdapterError> {
        let settings = AudioNormalization {
            target_lufs,
            ..Default::default()
        };
        self.normalize_with_settings(clip_id, &settings).await
    }

    /// Normalize audio with full settings
    pub async fn normalize_with_settings(
        &self,
        clip_id: &ClipId,
        settings: &AudioNormalization,
    ) -> Result<OperationResult, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.normalize_audio(clip_id, settings).await
    }

    /// Apply gain adjustment
    pub async fn apply_gain(&self, clip_id: &ClipId, gain_db: f32) -> Result<(), AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.apply_audio_gain(clip_id, gain_db).await
    }

    /// Batch normalize multiple clips
    pub async fn batch_normalize(
        &self,
        clips: &[ClipId],
        target_lufs: f32,
    ) -> Result<Vec<OperationResult>, AdapterError> {
        let mut results = Vec::with_capacity(clips.len());
        for clip_id in clips {
            let result = self.normalize(clip_id, target_lufs).await?;
            results.push(result);
        }
        Ok(results)
    }
}

/// Export operations API
pub struct ExportApi {
    adapter: Arc<RwLock<Box<dyn CreativeAdapter>>>,
}

impl ExportApi {
    /// Export the active timeline
    pub async fn render(&self, settings: &ExportSettings) -> Result<String, AdapterError> {
        let adapter = self.adapter.read().await;
        let timeline = adapter.get_active_timeline().await?;
        adapter.export(&timeline.id, settings).await
    }

    /// Get current export progress
    pub async fn progress(&self) -> Result<f32, AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.get_export_progress().await
    }

    /// Cancel current export
    pub async fn cancel(&self) -> Result<(), AdapterError> {
        let adapter = self.adapter.read().await;
        adapter.cancel_export().await
    }
}
