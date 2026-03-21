//! VisionDetector trait — generic interface for visual element detection.
//!
//! Implementations: MolmoDetector (CV2), UiTarsDetector (CV0 adapter), \[future\] custom models.
//! Used by VisionPipeline (CV3) as pluggable pointing model.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::ScreenCapture;

/// Unified input for all vision detectors.
///
/// Single canonical definition — used by ALL CV modules.
/// Two coordinate spaces:
/// - Physical (what detectors see in the image)
/// - Display (where clicks happen, pipeline output space)
#[derive(Debug, Clone)]
pub struct DetectionInput {
    /// Screenshot as base64 JPEG (original resolution, no downscale)
    pub base64: String,
    /// Physical image width (what detectors see)
    pub physical_width: u32,
    /// Physical image height (what detectors see)
    pub physical_height: u32,
    /// Logical display width (where clicks happen)
    pub display_width: u32,
    /// Logical display height (where clicks happen)
    pub display_height: u32,
    /// Physical / display ratio (for coordinate conversion)
    pub pixel_ratio: f32,
}

impl DetectionInput {
    /// Construct from a ScreenCapture — all fields map directly.
    pub fn from_capture(cap: &ScreenCapture) -> Self {
        Self {
            base64: cap.base64.clone(),
            physical_width: cap.physical_width,
            physical_height: cap.physical_height,
            display_width: cap.display_width,
            display_height: cap.display_height,
            pixel_ratio: cap.pixel_ratio,
        }
    }
}

/// Result of a vision detection operation.
///
/// Coordinates are always in **display pixels** — ready for clicking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionResult {
    /// X coordinate in display pixels
    pub x: u32,
    /// Y coordinate in display pixels
    pub y: u32,
    /// Detection confidence (0.0 - 1.0)
    pub confidence: f32,
    /// Human-readable label of detected element
    pub label: Option<String>,
    /// Provider name (e.g., "molmo", "uitars", "omniparser(exact)")
    pub provider: String,
}

impl DetectionResult {
    /// Create a "not found" sentinel result.
    pub fn not_found(task: &str) -> Self {
        Self {
            x: 0,
            y: 0,
            confidence: 0.0,
            label: Some(format!("not found: {task}")),
            provider: "none".into(),
        }
    }
}

/// Trait for visual element detection backends (pointing models).
///
/// Implementations handle the specifics of calling a VLM or detection model
/// and converting coordinates to display pixel space.
#[async_trait]
pub trait VisionDetector: Send + Sync {
    /// Human-readable name for logging/reporting.
    fn name(&self) -> &str;

    /// Whether this detector can return multiple points per call.
    fn supports_multi_point(&self) -> bool;

    /// Detect elements matching the task description.
    ///
    /// Returns results sorted by confidence (highest first).
    /// Empty vec means nothing was found.
    async fn detect(
        &self,
        input: &DetectionInput,
        task: &str,
    ) -> anyhow::Result<Vec<DetectionResult>>;
}

#[cfg(test)]
mod tests {
    use crate::*;
    use std::time::Instant;

    #[test]
    fn test_detection_input_from_capture() {
        let cap = ScreenCapture {
            base64: "abc123".into(),
            physical_width: 3840,
            physical_height: 2160,
            display_width: 1920,
            display_height: 1080,
            pixel_ratio: 2.0,
            timestamp: Instant::now(),
        };
        let input = DetectionInput::from_capture(&cap);
        assert_eq!(input.physical_width, 3840);
        assert_eq!(input.display_width, 1920);
        assert!((input.pixel_ratio - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_detection_result_not_found() {
        let r = DetectionResult::not_found("submit button");
        assert_eq!(r.x, 0);
        assert_eq!(r.y, 0);
        assert!(r.confidence.abs() < f32::EPSILON);
        assert_eq!(r.provider, "none");
        assert!(r.label.unwrap().contains("submit button"));
    }
}
