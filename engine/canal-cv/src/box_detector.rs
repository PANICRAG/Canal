//! BoxDetector trait — generic interface for UI bounding box detection.
//!
//! Implementations call an external service (e.g., OmniParser) to detect
//! all UI elements on screen and return their bounding boxes.
//! Used by VisionPipeline (CV3) for code-first matching before VLM fallback.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::vision_detector::DetectionInput;

/// A detected UI element bounding box in display pixels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    /// Human-readable label of the element
    pub label: String,
    /// X coordinate of top-left corner in display pixels
    pub x: u32,
    /// Y coordinate of top-left corner in display pixels
    pub y: u32,
    /// Width in display pixels
    pub width: u32,
    /// Height in display pixels
    pub height: u32,
    /// Detection confidence (0.0 - 1.0)
    pub confidence: f32,
    /// Element type if detected (e.g., "button", "input", "link")
    pub element_type: Option<String>,
}

impl BoundingBox {
    /// Center X coordinate.
    pub fn center_x(&self) -> u32 {
        self.x + self.width / 2
    }

    /// Center Y coordinate.
    pub fn center_y(&self) -> u32 {
        self.y + self.height / 2
    }

    /// Check if a point (in display pixels) is inside this box.
    pub fn contains(&self, px: u32, py: u32) -> bool {
        px >= self.x && px <= self.x + self.width && py >= self.y && py <= self.y + self.height
    }

    /// Distance from box center to a given point.
    pub fn distance_to(&self, px: u32, py: u32) -> f64 {
        let dx = self.center_x() as f64 - px as f64;
        let dy = self.center_y() as f64 - py as f64;
        (dx * dx + dy * dy).sqrt()
    }
}

/// Trait for UI element bounding box detection backends.
///
/// Implementations call an external service to detect all visible UI elements
/// on a screenshot and return their bounding boxes with labels.
#[async_trait]
pub trait BoxDetector: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Detect all UI element bounding boxes on the screenshot.
    ///
    /// Returns boxes in display pixel coordinates, sorted by confidence (highest first).
    async fn detect_boxes(&self, input: &DetectionInput) -> anyhow::Result<Vec<BoundingBox>>;

    /// Check if the detection service is available.
    async fn is_available(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use crate::*;

    fn test_box() -> BoundingBox {
        BoundingBox {
            label: "Submit".into(),
            x: 100,
            y: 200,
            width: 80,
            height: 30,
            confidence: 0.95,
            element_type: Some("button".into()),
        }
    }

    #[test]
    fn test_bounding_box_center() {
        let b = test_box();
        assert_eq!(b.center_x(), 140);
        assert_eq!(b.center_y(), 215);
    }

    #[test]
    fn test_bounding_box_contains() {
        let b = test_box();
        assert!(b.contains(140, 215)); // center
        assert!(b.contains(100, 200)); // top-left
        assert!(b.contains(180, 230)); // bottom-right
        assert!(!b.contains(99, 215)); // left of
        assert!(!b.contains(181, 215)); // right of
    }

    #[test]
    fn test_bounding_box_distance() {
        let b = test_box();
        let d = b.distance_to(140, 215);
        assert!(d.abs() < 0.001); // center to center = 0
    }
}
