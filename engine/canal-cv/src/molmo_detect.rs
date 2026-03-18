//! MolmoDetector — adapts `MolmoProvider` to the `VisionDetector` trait.
//!
//! Pipeline-internal detector. Not exposed as a standalone agent tool.
//! Used by `VisionPipeline` (CV3) as a pluggable pointing model.

use std::sync::Arc;

use async_trait::async_trait;

use crate::molmo_provider::MolmoProvider;
use crate::vision_detector::{DetectionInput, DetectionResult, VisionDetector};

/// Wraps `MolmoProvider` as a `VisionDetector` implementation.
///
/// Molmo supports multi-point detection (multiple elements in one call).
/// Baseline confidence for Molmo raw results is 0.50 (always returns coordinates,
/// even for non-existent elements — validation deferred to CV3 pipeline).
pub struct MolmoDetector {
    provider: Arc<MolmoProvider>,
}

impl MolmoDetector {
    /// Create from an existing provider instance.
    pub fn new(provider: Arc<MolmoProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl VisionDetector for MolmoDetector {
    fn name(&self) -> &str {
        "molmo"
    }

    fn supports_multi_point(&self) -> bool {
        true
    }

    async fn detect(
        &self,
        input: &DetectionInput,
        task: &str,
    ) -> anyhow::Result<Vec<DetectionResult>> {
        let result = self
            .provider
            .get_multiple_points(
                &input.base64,
                task,
                input.display_width,
                input.display_height,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        if result.points.is_empty() {
            return Ok(vec![DetectionResult::not_found(task)]);
        }

        Ok(result
            .points
            .into_iter()
            .map(|p| DetectionResult {
                x: p.x,
                y: p.y,
                confidence: 0.50, // Molmo baseline — always returns coords, needs validation
                label: result.description.clone(),
                provider: "molmo".into(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::{CvChatRequest, CvChatResponse, CvLlmClient, CvLlmError};
    use async_trait::async_trait;

    struct MockLlm;

    #[async_trait]
    impl CvLlmClient for MockLlm {
        async fn chat(&self, _req: CvChatRequest) -> Result<CvChatResponse, CvLlmError> {
            Ok(CvChatResponse {
                text: "mock".into(),
            })
        }
        async fn is_available(&self) -> bool {
            true
        }
    }

    #[test]
    fn test_molmo_detector_name() {
        let llm: Arc<dyn CvLlmClient> = Arc::new(MockLlm);
        let provider = Arc::new(MolmoProvider::new(llm));
        let detector = MolmoDetector::new(provider);
        assert_eq!(detector.name(), "molmo");
    }

    #[test]
    fn test_molmo_supports_multi_point() {
        let llm: Arc<dyn CvLlmClient> = Arc::new(MockLlm);
        let provider = Arc::new(MolmoProvider::new(llm));
        let detector = MolmoDetector::new(provider);
        assert!(detector.supports_multi_point());
    }
}
