//! OmniParser detector — calls OmniParser API for bounding box detection.
//!
//! OmniParser is a UI element detection service that returns bounding boxes
//! for all visible UI elements on a screenshot. Used by VisionPipeline (CV3)
//! for code-first matching before VLM fallback.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::box_detector::{BoundingBox, BoxDetector};
use crate::vision_detector::DetectionInput;

/// Configuration for OmniParser API.
#[derive(Debug, Clone)]
pub struct OmniParserConfig {
    /// OmniParser API endpoint URL
    pub api_url: String,
    /// Optional API key for authentication
    pub api_key: Option<String>,
    /// Request timeout in seconds
    pub timeout_seconds: u32,
    /// Minimum confidence to include a box
    pub confidence_threshold: f32,
}

impl Default for OmniParserConfig {
    fn default() -> Self {
        Self {
            api_url: "http://localhost:8000/parse".to_string(),
            api_key: None,
            timeout_seconds: 10,
            confidence_threshold: 0.3,
        }
    }
}

/// Raw bounding box from OmniParser API response (physical pixels).
#[derive(Debug, Deserialize, Serialize)]
struct RawBoundingBox {
    label: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    confidence: f64,
    #[serde(default)]
    element_type: Option<String>,
}

/// OmniParser API response structure.
#[derive(Debug, Deserialize)]
struct OmniParserResponse {
    #[serde(default)]
    boxes: Vec<RawBoundingBox>,
}

/// OmniParser-based box detector.
///
/// Calls the OmniParser HTTP API to detect all UI elements on a screenshot.
/// Returns bounding boxes converted from physical pixels to display pixels.
pub struct OmniParserDetector {
    client: reqwest::Client,
    config: OmniParserConfig,
}

impl OmniParserDetector {
    /// Create with default configuration.
    pub fn new() -> Self {
        Self::with_config(OmniParserConfig::default())
    }

    /// Create with custom configuration.
    pub fn with_config(config: OmniParserConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(
                config.timeout_seconds as u64,
            ))
            .build()
            .unwrap_or_default();
        Self { client, config }
    }

    /// Call OmniParser API and get raw bounding boxes.
    async fn call_api(&self, base64: &str) -> anyhow::Result<Vec<RawBoundingBox>> {
        let body = serde_json::json!({
            "image": base64,
        });

        let mut request = self.client.post(&self.config.api_url).json(&body);

        if let Some(ref key) = self.config.api_key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }

        let response = request.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("OmniParser API error {status}: {text}");
        }

        let parsed: OmniParserResponse = response.json().await?;
        Ok(parsed.boxes)
    }
}

impl Default for OmniParserDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BoxDetector for OmniParserDetector {
    fn name(&self) -> &str {
        "omniparser"
    }

    async fn detect_boxes(&self, input: &DetectionInput) -> anyhow::Result<Vec<BoundingBox>> {
        let raw_boxes = self.call_api(&input.base64).await?;

        let ratio = input.pixel_ratio;
        let threshold = self.config.confidence_threshold;

        let mut boxes: Vec<BoundingBox> = raw_boxes
            .into_iter()
            .filter(|b| b.confidence as f32 >= threshold)
            .map(|b| {
                // Convert from physical pixels to display pixels
                BoundingBox {
                    label: b.label,
                    x: (b.x / ratio as f64) as u32,
                    y: (b.y / ratio as f64) as u32,
                    width: (b.width / ratio as f64) as u32,
                    height: (b.height / ratio as f64) as u32,
                    confidence: b.confidence as f32,
                    element_type: b.element_type,
                }
            })
            .collect();

        // Sort by confidence, highest first
        boxes.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(boxes)
    }

    async fn is_available(&self) -> bool {
        // Try a lightweight health check
        let health_url = self.config.api_url.trim_end_matches("/parse").to_string() + "/health";

        self.client
            .get(&health_url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_omniparser_config_defaults() {
        let config = OmniParserConfig::default();
        assert_eq!(config.api_url, "http://localhost:8000/parse");
        assert!(config.api_key.is_none());
        assert_eq!(config.timeout_seconds, 10);
        assert!((config.confidence_threshold - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn test_raw_bbox_deserialization() {
        let json = r#"{
            "label": "Submit",
            "x": 200.0,
            "y": 400.0,
            "width": 160.0,
            "height": 60.0,
            "confidence": 0.95,
            "element_type": "button"
        }"#;
        let raw: RawBoundingBox = serde_json::from_str(json).unwrap();
        assert_eq!(raw.label, "Submit");
        assert!((raw.x - 200.0).abs() < 0.001);
        assert_eq!(raw.element_type, Some("button".into()));
    }

    #[test]
    fn test_physical_to_display_conversion() {
        let raw = RawBoundingBox {
            label: "Button".into(),
            x: 400.0,
            y: 600.0,
            width: 160.0,
            height: 60.0,
            confidence: 0.9,
            element_type: None,
        };

        let ratio = 2.0f32;
        let bbox = BoundingBox {
            label: raw.label,
            x: (raw.x / ratio as f64) as u32,
            y: (raw.y / ratio as f64) as u32,
            width: (raw.width / ratio as f64) as u32,
            height: (raw.height / ratio as f64) as u32,
            confidence: raw.confidence as f32,
            element_type: raw.element_type,
        };

        assert_eq!(bbox.x, 200);
        assert_eq!(bbox.y, 300);
        assert_eq!(bbox.width, 80);
        assert_eq!(bbox.height, 30);
    }

    #[test]
    fn test_omniparser_response_deserialization() {
        let json = r#"{"boxes": [
            {"label": "OK", "x": 100, "y": 200, "width": 50, "height": 30, "confidence": 0.9}
        ]}"#;
        let resp: OmniParserResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.boxes.len(), 1);
        assert_eq!(resp.boxes[0].label, "OK");
    }
}
