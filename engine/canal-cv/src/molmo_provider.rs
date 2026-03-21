//! Molmo 2 vision provider — calls allenai/molmo-2-8b via CvLlmClient.
//!
//! Wraps any `CvLlmClient` implementation for LLM calls,
//! uses `MolmoParser` for response parsing.
//!
//! Key differences from UI-TARS:
//! - Output format: XML `<points coords="...">` instead of `Action: click(...)`
//! - Supports multi-point detection in a single call
//! - Different prompting strategy (describe-to-point vs action prediction)

use std::sync::Arc;

use crate::llm_client::{CvChatRequest, CvContent, CvLlmClient, CvMessage};
use crate::molmo_parser::{MolmoParseError, MolmoParser};

/// Configuration for Molmo provider.
#[derive(Debug, Clone)]
pub struct MolmoProviderConfig {
    /// Molmo model ID (e.g., on OpenRouter)
    pub model_id: String,
    /// Maximum tokens for response
    pub max_tokens: u32,
    /// Request timeout in seconds
    pub timeout_seconds: u32,
    /// Enable debug logging
    pub debug: bool,
}

impl Default for MolmoProviderConfig {
    fn default() -> Self {
        Self {
            model_id: "allenai/molmo-7b-d-0924".to_string(),
            max_tokens: 1024,
            timeout_seconds: 30,
            debug: false,
        }
    }
}

/// Result of a single-point Molmo detection.
#[derive(Debug, Clone)]
pub struct MolmoClickResult {
    /// X coordinate in display pixels
    pub x: u32,
    /// Y coordinate in display pixels
    pub y: u32,
    /// Original normalized X (0-1000)
    pub normalized_x: u32,
    /// Original normalized Y (0-1000)
    pub normalized_y: u32,
    /// Description from Molmo
    pub description: Option<String>,
    /// Reasoning from Molmo
    pub reasoning: Option<String>,
    /// Raw model response
    pub raw_response: String,
}

/// Result of a multi-point Molmo detection.
#[derive(Debug, Clone)]
pub struct MolmoMultiPointResult {
    /// All detected points in display pixels
    pub points: Vec<MolmoDetectedPoint>,
    /// Description from Molmo
    pub description: Option<String>,
    /// Reasoning from Molmo
    pub reasoning: Option<String>,
    /// Raw model response
    pub raw_response: String,
}

/// A single detected point in display pixel coordinates.
#[derive(Debug, Clone)]
pub struct MolmoDetectedPoint {
    /// Point ID from Molmo
    pub point_id: u32,
    /// X coordinate in display pixels
    pub x: u32,
    /// Y coordinate in display pixels
    pub y: u32,
    /// Original normalized X (0-1000)
    pub normalized_x: u32,
    /// Original normalized Y (0-1000)
    pub normalized_y: u32,
}

/// Molmo 2 vision provider for element detection via pointing.
///
/// Sends screenshots to Molmo via any `CvLlmClient` and parses the `<points>`
/// XML response to extract click coordinates.
///
/// # Example
/// ```ignore
/// let provider = MolmoProvider::new(llm_client);
/// let result = provider.get_click_coordinates(
///     &screenshot_base64,
///     "Point to the submit button",
///     1920, 1080,
/// ).await?;
/// println!("Click at ({}, {})", result.x, result.y);
/// ```
pub struct MolmoProvider {
    llm: Arc<dyn CvLlmClient>,
    config: MolmoProviderConfig,
}

impl MolmoProvider {
    /// Create with a CvLlmClient and default configuration.
    pub fn new(llm: Arc<dyn CvLlmClient>) -> Self {
        Self::with_config(llm, MolmoProviderConfig::default())
    }

    /// Create with a CvLlmClient and custom configuration.
    pub fn with_config(llm: Arc<dyn CvLlmClient>, config: MolmoProviderConfig) -> Self {
        Self { llm, config }
    }

    /// Detect a single click target on the screenshot.
    ///
    /// Returns coordinates in display pixels.
    pub async fn get_click_coordinates(
        &self,
        screenshot_base64: &str,
        task: &str,
        display_width: u32,
        display_height: u32,
    ) -> anyhow::Result<MolmoClickResult> {
        let raw_response = self.call_molmo(screenshot_base64, task).await?;

        let parsed = MolmoParser::parse_single_point(&raw_response).map_err(|e| match e {
            MolmoParseError::NoPointsFound => {
                anyhow::anyhow!("Molmo found no points for task: {task}")
            }
            MolmoParseError::MultiplePoints(n) => {
                anyhow::anyhow!("Molmo returned {n} points, expected 1 for task: {task}")
            }
            MolmoParseError::InvalidCoordinate(s) => {
                anyhow::anyhow!("Molmo invalid coordinate: {s}")
            }
        })?;

        let (x, y) =
            MolmoParser::to_display_pixels(parsed.x, parsed.y, display_width, display_height);

        let full_result = MolmoParser::parse(&raw_response);

        Ok(MolmoClickResult {
            x,
            y,
            normalized_x: parsed.x,
            normalized_y: parsed.y,
            description: full_result.description,
            reasoning: full_result.reasoning,
            raw_response,
        })
    }

    /// Detect multiple points on the screenshot.
    ///
    /// Returns all detected points in display pixel coordinates.
    pub async fn get_multiple_points(
        &self,
        screenshot_base64: &str,
        task: &str,
        display_width: u32,
        display_height: u32,
    ) -> anyhow::Result<MolmoMultiPointResult> {
        let raw_response = self.call_molmo(screenshot_base64, task).await?;

        let full_result = MolmoParser::parse(&raw_response);

        let points = full_result
            .points
            .iter()
            .map(|p| {
                let (x, y) =
                    MolmoParser::to_display_pixels(p.x, p.y, display_width, display_height);
                MolmoDetectedPoint {
                    point_id: p.point_id,
                    x,
                    y,
                    normalized_x: p.x,
                    normalized_y: p.y,
                }
            })
            .collect();

        Ok(MolmoMultiPointResult {
            points,
            description: full_result.description,
            reasoning: full_result.reasoning,
            raw_response,
        })
    }

    /// Check if Molmo is available via the LLM client.
    pub async fn is_available(&self) -> bool {
        self.llm.is_available().await
    }

    /// Send request to Molmo via CvLlmClient.
    async fn call_molmo(&self, screenshot_base64: &str, task: &str) -> anyhow::Result<String> {
        let media_type = if screenshot_base64.starts_with("/9j/") {
            "image/jpeg"
        } else if screenshot_base64.starts_with("iVBOR") {
            "image/png"
        } else {
            "image/jpeg"
        };

        let message = CvMessage::new(
            "user",
            vec![
                CvContent::Image {
                    media_type: media_type.to_string(),
                    base64_data: screenshot_base64.to_string(),
                },
                CvContent::Text {
                    text: format!("Point to: {task}"),
                },
            ],
        );

        let request = CvChatRequest {
            messages: vec![message],
            model: Some(self.config.model_id.clone()),
            max_tokens: Some(self.config.max_tokens),
            temperature: Some(0.1),
        };

        if self.config.debug {
            tracing::debug!(
                model = %self.config.model_id,
                task = %task,
                "Sending Molmo request"
            );
        }

        let response = self
            .llm
            .chat(request)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        if self.config.debug {
            tracing::debug!(response = %response.text, "Molmo raw response");
        }

        Ok(response.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::{CvChatResponse, CvLlmError};
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
    fn test_default_config() {
        let config = MolmoProviderConfig::default();
        assert_eq!(config.model_id, "allenai/molmo-7b-d-0924");
        assert_eq!(config.max_tokens, 1024);
        assert_eq!(config.timeout_seconds, 30);
        assert!(!config.debug);
    }

    #[test]
    fn test_provider_creation() {
        let llm: Arc<dyn CvLlmClient> = Arc::new(MockLlm);
        let provider = MolmoProvider::new(llm);
        assert_eq!(provider.config.model_id, "allenai/molmo-7b-d-0924");
    }

    #[test]
    fn test_custom_config() {
        let config = MolmoProviderConfig {
            model_id: "allenai/molmo-2-8b".to_string(),
            max_tokens: 2048,
            timeout_seconds: 60,
            debug: true,
        };
        let llm: Arc<dyn CvLlmClient> = Arc::new(MockLlm);
        let provider = MolmoProvider::with_config(llm, config);
        assert_eq!(provider.config.model_id, "allenai/molmo-2-8b");
        assert_eq!(provider.config.max_tokens, 2048);
        assert!(provider.config.debug);
    }
}
