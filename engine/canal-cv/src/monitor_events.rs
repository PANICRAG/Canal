//! Monitor event types — screen change events broadcast by ScreenMonitor.
//!
//! Events are lightweight (no full screenshots) to avoid expensive clones
//! in tokio::broadcast channel. Screenshots are captured on-demand via
//! ScreenController when actually needed.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::types::ContextInfo;

/// State of the screen at a point in time.
///
/// Lightweight — stores pHash + metadata only, NOT full screenshots.
/// A 10-entry history costs ~1KB vs ~40MB with inline base64.
#[derive(Debug, Clone)]
pub struct MonitoredState {
    /// Perceptual hash of the screen capture.
    pub phash: u64,
    /// Display width (logical pixels).
    pub display_width: u32,
    /// Display height (logical pixels).
    pub display_height: u32,
    /// Context info (URL, title, app name) if available.
    pub context: Option<ContextInfo>,
    /// When this state was captured.
    pub timestamp: Instant,
}

/// Event broadcast when the screen changes.
///
/// Sent via `tokio::broadcast` — cloned on send, so must be lightweight.
/// No full MonitoredState or screenshots to avoid multi-MB copies.
#[derive(Debug, Clone)]
pub struct ScreenChangeEvent {
    /// What kind of change was detected.
    pub change_type: ChangeType,
    /// pHash before the change.
    pub before_phash: u64,
    /// pHash after the change.
    pub after_phash: u64,
    /// Context before.
    pub before_context: Option<ContextInfo>,
    /// Context after.
    pub after_context: Option<ContextInfo>,
    /// Similarity between before and after (0.0-1.0).
    pub similarity: f32,
    /// When the change was detected.
    pub detected_at: Instant,
}

/// Classification of screen changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChangeType {
    /// Context changed (URL navigation, app switch, window title change).
    ContextChange,
    /// Visual content changed (same context).
    ContentUpdate {
        /// Similarity score between before and after.
        similarity: f32,
    },
    /// Both context and visual changed.
    FullChange {
        /// Similarity score between before and after.
        similarity: f32,
    },
    /// Screen became stable after activity.
    Stabilized,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_change_type_context_change() {
        let ct = ChangeType::ContextChange;
        assert!(matches!(ct, ChangeType::ContextChange));
    }

    #[test]
    fn test_change_type_content_update() {
        let ct = ChangeType::ContentUpdate { similarity: 0.72 };
        if let ChangeType::ContentUpdate { similarity } = ct {
            assert!((similarity - 0.72).abs() < f32::EPSILON);
        } else {
            panic!("Expected ContentUpdate");
        }
    }

    #[test]
    fn test_change_type_full_change() {
        let ct = ChangeType::FullChange { similarity: 0.3 };
        assert!(matches!(ct, ChangeType::FullChange { .. }));
    }

    #[test]
    fn test_monitored_state_clone() {
        let state = MonitoredState {
            phash: 12345,
            display_width: 1920,
            display_height: 1080,
            context: Some(ContextInfo {
                url: Some("https://example.com".into()),
                title: Some("Example".into()),
                app_name: Some("Browser".into()),
                interactive_elements: None,
            }),
            timestamp: Instant::now(),
        };
        let cloned = state.clone();
        assert_eq!(cloned.phash, 12345);
        assert_eq!(cloned.display_width, 1920);
        assert!(cloned.context.is_some());
    }

    #[test]
    fn test_screen_change_event_clone() {
        let event = ScreenChangeEvent {
            change_type: ChangeType::ContentUpdate { similarity: 0.8 },
            before_phash: 100,
            after_phash: 200,
            before_context: None,
            after_context: None,
            similarity: 0.8,
            detected_at: Instant::now(),
        };
        let cloned = event.clone();
        assert_eq!(cloned.before_phash, 100);
        assert_eq!(cloned.after_phash, 200);
        assert!((cloned.similarity - 0.8).abs() < f32::EPSILON);
    }
}
