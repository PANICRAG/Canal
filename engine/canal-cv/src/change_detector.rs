//! ScreenChangeDetector — detects screen changes using perceptual hashing.
//!
//! Shared instance used ONLY by CV5 ScreenMonitor for background polling.
//! Pipeline and ActionChainExecutor use LOCAL pHash baselines (see CV4 v5.0).

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::phash::{compute_phash, hash_similarity};
use crate::screen_controller::ScreenController;
use crate::types::ComputerUseError;

/// Configuration for screen change detection.
pub struct ChangeDetectionConfig {
    /// Similarity threshold below which a change is detected.
    /// Default: 0.85 (15% or more difference = changed)
    pub change_threshold: f32,
}

impl Default for ChangeDetectionConfig {
    fn default() -> Self {
        Self {
            change_threshold: 0.85,
        }
    }
}

/// Detects screen changes by comparing perceptual hashes of consecutive captures.
///
/// **Ownership rule**: Only `ScreenMonitor` (CV5) should hold a shared instance.
/// Other components (pipeline, chain executor) use `compute_phash()` + `hash_similarity()`
/// directly with local baselines.
pub struct ScreenChangeDetector {
    controller: Arc<dyn ScreenController>,
    last_hash: Arc<RwLock<Option<u64>>>,
    config: ChangeDetectionConfig,
}

impl ScreenChangeDetector {
    /// Create a new detector bound to a screen controller.
    pub fn new(controller: Arc<dyn ScreenController>, config: ChangeDetectionConfig) -> Self {
        Self {
            controller,
            last_hash: Arc::new(RwLock::new(None)),
            config,
        }
    }

    /// Capture current screen, compute pHash, compare with last known hash.
    ///
    /// Updates internal state. Returns true if screen has changed.
    pub async fn has_changed(&self) -> Result<bool, ComputerUseError> {
        let capture = self.controller.capture().await?;
        let current_hash = compute_phash(&capture.base64);

        let mut last = self.last_hash.write().await;
        let changed = match *last {
            Some(prev_hash) => {
                let similarity = hash_similarity(prev_hash, current_hash);
                similarity < self.config.change_threshold
            }
            None => false, // First capture — no previous state to compare
        };
        *last = Some(current_hash);
        Ok(changed)
    }

    /// Get the current hash without capturing a new screenshot.
    pub async fn current_hash(&self) -> Option<u64> {
        *self.last_hash.read().await
    }

    /// Reset internal state (e.g., after navigation).
    pub async fn reset(&self) {
        *self.last_hash.write().await = None;
    }

    /// Wait until screen stabilizes (no change for `settle_ms`).
    ///
    /// Max wait: 5 seconds. Polls every `settle_ms`.
    pub async fn wait_for_stable(&self, settle_ms: u64) -> Result<(), ComputerUseError> {
        let max_wait = std::time::Duration::from_secs(5);
        let settle = std::time::Duration::from_millis(settle_ms);
        let start = std::time::Instant::now();

        loop {
            tokio::time::sleep(settle).await;
            if !self.has_changed().await? {
                return Ok(());
            }
            if start.elapsed() > max_wait {
                return Ok(()); // Timeout — proceed anyway
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_controller::NoopScreenController;

    #[tokio::test]
    async fn test_first_capture_returns_error_with_noop() {
        let controller = Arc::new(NoopScreenController::new());
        let detector = ScreenChangeDetector::new(controller, ChangeDetectionConfig::default());
        // NoopScreenController returns NotConnected error
        assert!(detector.has_changed().await.is_err());
    }

    #[tokio::test]
    async fn test_reset_clears_state() {
        let detector = ScreenChangeDetector::new(
            Arc::new(NoopScreenController::new()),
            ChangeDetectionConfig::default(),
        );
        detector.reset().await;
        assert!(detector.current_hash().await.is_none());
    }

    #[test]
    fn test_change_detection_config_default() {
        let config = ChangeDetectionConfig::default();
        assert!((config.change_threshold - 0.85).abs() < f32::EPSILON);
    }
}
