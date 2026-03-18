//! Core types shared across all CV modules.
//!
//! These types form the foundation that CV1-CV7 build on.
//! All coordinates follow the two-space convention:
//! - Physical pixels: raw image dimensions (what detectors see)
//! - Display pixels: logical screen dimensions (where clicks happen)
//!
//! Conversion: `display = physical / pixel_ratio`

use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Screenshot captured from any screen controller.
///
/// Original resolution, no compression or downscaling.
/// JPEG quality 70 at full resolution (~1.5MB, acceptable for local/near-network).
#[derive(Debug, Clone)]
pub struct ScreenCapture {
    /// JPEG compressed base64 (quality 70), original resolution
    pub base64: String,
    /// Physical image width (raw capture output)
    pub physical_width: u32,
    /// Physical image height (raw capture output)
    pub physical_height: u32,
    /// Logical display width (where clicks happen)
    pub display_width: u32,
    /// Logical display height (where clicks happen)
    pub display_height: u32,
    /// Physical / display ratio (e.g., 2.0 for Retina)
    pub pixel_ratio: f32,
    /// When this capture was taken
    pub timestamp: Instant,
}

/// Context information about the current screen surface.
///
/// Optional enrichment — desktop provides rich context via accessibility APIs,
/// browser provides URL/title, NoopController provides None.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextInfo {
    /// URL (browser) or None (desktop app)
    pub url: Option<String>,
    /// Window/tab title
    pub title: Option<String>,
    /// Application name (e.g., "Safari", "Figma")
    pub app_name: Option<String>,
    /// Interactive elements on screen (from accessibility tree)
    pub interactive_elements: Option<Vec<InteractiveElement>>,
}

/// An interactive UI element detected via accessibility APIs.
///
/// Coordinates are in display pixels (not physical pixels).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractiveElement {
    /// Unique ID (e.g., "ax_0", "ref_e1")
    pub id: String,
    /// Element type: "button", "input", "link", "checkbox", etc.
    pub element_type: String,
    /// Visible label / accessible name
    pub label: String,
    /// Bounding box in display pixels
    pub bounds: ElementBounds,
}

/// Bounding box for an interactive element, in display pixels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl ElementBounds {
    /// Center X coordinate of this element.
    pub fn center_x(&self) -> f32 {
        self.x + self.width / 2.0
    }

    /// Center Y coordinate of this element.
    pub fn center_y(&self) -> f32 {
        self.y + self.height / 2.0
    }

    /// Check if a point (in display pixels) is inside this bounding box.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.width && py >= self.y && py <= self.y + self.height
    }
}

/// Mouse button for click actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Keyboard modifier keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Modifier {
    Shift,
    Control,
    Alt,
    Meta,
}

/// Error type for all computer use operations.
#[derive(Debug, thiserror::Error)]
pub enum ComputerUseError {
    #[error("screenshot capture failed: {0}")]
    CaptureFailed(String),
    #[error("input action failed: {0}")]
    InputFailed(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("not connected to any screen surface")]
    NotConnected,
    #[error("operation timeout after {0:?}")]
    Timeout(std::time::Duration),
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_element_bounds_center() {
        let bounds = ElementBounds {
            x: 100.0,
            y: 200.0,
            width: 50.0,
            height: 30.0,
        };
        assert!((bounds.center_x() - 125.0).abs() < f32::EPSILON);
        assert!((bounds.center_y() - 215.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_element_bounds_contains() {
        let bounds = ElementBounds {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        };
        assert!(bounds.contains(50.0, 40.0)); // inside
        assert!(!bounds.contains(5.0, 40.0)); // left of
        assert!(!bounds.contains(50.0, 80.0)); // below
    }

    #[test]
    fn test_element_bounds_contains_edge() {
        let bounds = ElementBounds {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        };
        // Edge cases: on the boundary should be inside
        assert!(bounds.contains(10.0, 20.0)); // top-left corner
        assert!(bounds.contains(110.0, 70.0)); // bottom-right corner
    }
}
