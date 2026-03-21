//! UiTarsDetector — adapts existing `browser/uitars_provider.rs` to VisionDetector trait.
//!
//! This is a thin wrapper, not a reimplementation. The existing UiTarsProvider
//! handles all OpenRouter communication and response parsing.

use std::sync::Arc;

use async_trait::async_trait;

use super::uitars_provider::UiTarsProvider;
use canal_cv::vision_detector::{DetectionInput, DetectionResult, VisionDetector};

/// Wraps `UiTarsProvider` as a `VisionDetector` implementation.
///
/// UI-TARS returns normalized 0-1000 coordinates, which this adapter
/// converts to display pixel space.
pub struct UiTarsDetector {
    provider: Arc<UiTarsProvider>,
}

impl UiTarsDetector {
    /// Create from an existing provider instance.
    pub fn new(provider: Arc<UiTarsProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl VisionDetector for UiTarsDetector {
    fn name(&self) -> &str {
        "uitars"
    }

    fn supports_multi_point(&self) -> bool {
        false
    }

    async fn detect(
        &self,
        input: &DetectionInput,
        task: &str,
    ) -> anyhow::Result<Vec<DetectionResult>> {
        let result = self
            .provider
            .get_click_coordinates(
                &input.base64,
                task,
                input.display_width,
                input.display_height,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        // UI-TARS returns CSS pixel coordinates directly from get_click_coordinates
        Ok(vec![DetectionResult {
            x: result.x,
            y: result.y,
            confidence: 0.7, // UI-TARS doesn't expose confidence
            label: result.thought.clone(),
            provider: "uitars".into(),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uitars_detector_name() {
        let provider = Arc::new(UiTarsProvider::new());
        let detector = UiTarsDetector::new(provider);
        assert_eq!(detector.name(), "uitars");
    }

    #[test]
    fn test_uitars_no_multi_point() {
        let provider = Arc::new(UiTarsProvider::new());
        let detector = UiTarsDetector::new(provider);
        assert!(!detector.supports_multi_point());
    }
}
