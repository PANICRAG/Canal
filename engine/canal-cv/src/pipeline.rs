//! Unified Computer Use Pipeline — act/extract/observe.
//!
//! Central entry point for all computer use operations:
//! - `act()` — parse instruction, detect target, execute action, verify
//! - `extract()` — capture screen, send to LLM, get structured data
//! - `observe()` — capture screen, return screenshot + context
//!
//! Uses LOCAL pHash baselines for verification (not shared ScreenChangeDetector).

use std::sync::Arc;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::llm_client::{CvChatRequest, CvContent, CvLlmClient, CvMessage};
use crate::observation_narrator::ObservationNarrator;
use crate::phash::{compute_phash, hash_similarity};
use crate::screen_controller::ScreenController;
use crate::vision_detector::DetectionInput;
use crate::vision_pipeline::VisionPipeline;

/// Configuration for the computer use pipeline.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Maximum retry attempts for action verification.
    pub max_retries: u32,
    /// Wait time (ms) after action for screen to settle.
    pub settle_delay_ms: u64,
    /// Whether to verify actions via pHash comparison.
    pub verify_actions: bool,
    /// pHash similarity threshold below which screen is "changed".
    pub change_threshold: f32,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            settle_delay_ms: 500,
            verify_actions: true,
            change_threshold: 0.85,
        }
    }
}

/// Type of user interaction to perform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionType {
    Click,
    Type,
    Scroll,
    KeyPress,
    Select,
}

/// Direction for scroll actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScrollDir {
    Up,
    Down,
    Left,
    Right,
}

/// Parsed instruction from natural language.
///
/// No LLM needed — regex-based extraction of action type, target, and parameters.
#[derive(Debug, Clone)]
pub struct ParsedInstruction {
    /// What kind of action to perform.
    pub action: ActionType,
    /// Target element description (e.g., "Submit button").
    pub target: String,
    /// Text to type (for Type actions).
    pub text: Option<String>,
    /// Key name (for KeyPress actions).
    pub key: Option<String>,
    /// Scroll direction.
    pub direction: Option<ScrollDir>,
}

impl ParsedInstruction {
    /// Parse a natural language instruction into structured form.
    ///
    /// # Examples
    /// - "Click the Submit button" -> ActionType::Click, target "Submit button"
    /// - "Type hello in the search box" -> ActionType::Type, target "search box", text "hello"
    /// - "Press Enter" -> ActionType::KeyPress, key "Enter"
    /// - "Scroll down" -> ActionType::Scroll, direction Down
    pub fn parse(instruction: &str) -> Result<Self, ParseError> {
        let lower = instruction.to_lowercase();
        let trimmed = instruction.trim();

        // Click: "Click [on|the] <target>"
        let click_re = Regex::new(r"(?i)^click\s+(?:on\s+)?(?:the\s+)?(.+)$").unwrap();
        if let Some(caps) = click_re.captures(trimmed) {
            return Ok(Self {
                action: ActionType::Click,
                target: caps[1].trim().to_string(),
                text: None,
                key: None,
                direction: None,
            });
        }

        // Type: "Type '<text>' [in|into] [the] <target>"
        let type_re =
            Regex::new(r#"(?i)^type\s+['"](.+?)['"]\s+(?:in(?:to)?)\s+(?:the\s+)?(.+)$"#).unwrap();
        if let Some(caps) = type_re.captures(trimmed) {
            return Ok(Self {
                action: ActionType::Type,
                target: caps[2].trim().to_string(),
                text: Some(caps[1].to_string()),
                key: None,
                direction: None,
            });
        }

        // Type without quotes: "Type <text> in <target>"
        let type_re2 = Regex::new(r"(?i)^type\s+(.+?)\s+(?:in(?:to)?)\s+(?:the\s+)?(.+)$").unwrap();
        if let Some(caps) = type_re2.captures(trimmed) {
            return Ok(Self {
                action: ActionType::Type,
                target: caps[2].trim().to_string(),
                text: Some(caps[1].to_string()),
                key: None,
                direction: None,
            });
        }

        // KeyPress: "Press <key>"
        let key_re = Regex::new(r"(?i)^press\s+(.+)$").unwrap();
        if let Some(caps) = key_re.captures(trimmed) {
            return Ok(Self {
                action: ActionType::KeyPress,
                target: String::new(),
                text: None,
                key: Some(caps[1].trim().to_string()),
                direction: None,
            });
        }

        // Scroll: "Scroll <direction>"
        if lower.contains("scroll") {
            let direction = if lower.contains("down") {
                ScrollDir::Down
            } else if lower.contains("up") {
                ScrollDir::Up
            } else if lower.contains("left") {
                ScrollDir::Left
            } else if lower.contains("right") {
                ScrollDir::Right
            } else {
                ScrollDir::Down // default
            };
            return Ok(Self {
                action: ActionType::Scroll,
                target: String::new(),
                text: None,
                key: None,
                direction: Some(direction),
            });
        }

        // Select: "Select <option>"
        let select_re = Regex::new(r"(?i)^select\s+(.+)$").unwrap();
        if let Some(caps) = select_re.captures(trimmed) {
            return Ok(Self {
                action: ActionType::Select,
                target: caps[1].trim().to_string(),
                text: None,
                key: None,
                direction: None,
            });
        }

        Err(ParseError::UnrecognizedInstruction(instruction.to_string()))
    }

    /// Get the target text for vision pipeline detection.
    pub fn target_text(&self) -> &str {
        &self.target
    }
}

/// Error type for instruction parsing.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unrecognized instruction: {0}")]
    UnrecognizedInstruction(String),
}

/// Result of a pipeline `act()` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActResult {
    /// Whether the action succeeded (screen changed).
    pub success: bool,
    /// Description of the action taken.
    pub action_taken: String,
    /// Detection method used (e.g., "omniparser(exact)", "molmo(raw)").
    pub detection_method: String,
    /// Detection confidence (0.0 - 1.0).
    pub confidence: f32,
    /// Target X coordinate in display pixels.
    pub target_x: u32,
    /// Target Y coordinate in display pixels.
    pub target_y: u32,
    /// Whether the screen visibly changed after the action.
    pub screen_changed: bool,
    /// Before/after pHash similarity.
    pub similarity: f32,
    /// Narration of what happened.
    pub narration: Option<String>,
    /// Number of attempts (1 = first try succeeded).
    pub attempts: u32,
}

/// Result of a pipeline `observe()` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObserveResult {
    /// Screenshot as base64 JPEG.
    pub screenshot: String,
    /// Display width in pixels.
    pub display_width: u32,
    /// Display height in pixels.
    pub display_height: u32,
    /// Screen context (URL, title, app, elements).
    pub context: Option<serde_json::Value>,
    /// Number of interactive elements detected.
    pub element_count: usize,
}

/// The unified computer use pipeline.
///
/// Orchestrates screen capture, vision detection, action execution,
/// and result verification for computer use operations.
pub struct ComputerUsePipeline {
    controller: Arc<dyn ScreenController>,
    vision: Arc<VisionPipeline>,
    extract_llm: Option<Arc<dyn CvLlmClient>>,
    narrator: ObservationNarrator,
    config: PipelineConfig,
}

impl ComputerUsePipeline {
    /// Create a new pipeline.
    pub fn new(
        controller: Arc<dyn ScreenController>,
        vision: Arc<VisionPipeline>,
        extract_llm: Option<Arc<dyn CvLlmClient>>,
        narrator: ObservationNarrator,
        config: PipelineConfig,
    ) -> Self {
        Self {
            controller,
            vision,
            extract_llm,
            narrator,
            config,
        }
    }

    /// Execute a computer use action with detection, execution, and verification.
    ///
    /// Workflow:
    /// 1. Capture before screenshot + compute pHash
    /// 2. Parse instruction -> action type + target
    /// 3. Detect target via VisionPipeline (code-first cascade)
    /// 4. Execute action via ScreenController
    /// 5. Wait for settle, capture after screenshot
    /// 6. Verify via local pHash comparison
    /// 7. Narrate result
    /// 8. Retry on failure (up to max_retries)
    pub async fn act(&self, instruction: &str) -> anyhow::Result<ActResult> {
        let parsed = ParsedInstruction::parse(instruction).map_err(|e| anyhow::anyhow!("{}", e))?;

        let mut attempts = 0;

        loop {
            attempts += 1;

            // 1. Capture before
            let before = self
                .controller
                .capture()
                .await
                .map_err(|e| anyhow::anyhow!("capture failed: {}", e))?;
            let before_hash = compute_phash(&before.base64);
            let before_context = self.controller.context_info();

            // 2. Detect target
            let input = DetectionInput::from_capture(&before);
            let a11y_elements = before_context
                .as_ref()
                .and_then(|c| c.interactive_elements.as_ref());

            let detection = self
                .vision
                .detect(
                    &input,
                    parsed.target_text(),
                    a11y_elements.map(|v| v.as_slice()),
                )
                .await?;

            // 3. Execute action
            let action_desc = self
                .execute_action(&parsed, detection.x, detection.y)
                .await?;

            // 4. Wait for settle
            tokio::time::sleep(std::time::Duration::from_millis(
                self.config.settle_delay_ms,
            ))
            .await;

            // 5. Capture after + verify
            let after = self
                .controller
                .capture()
                .await
                .map_err(|e| anyhow::anyhow!("post-action capture failed: {}", e))?;
            let after_hash = compute_phash(&after.base64);
            let after_context = self.controller.context_info();

            let similarity = hash_similarity(before_hash, after_hash);
            let screen_changed = similarity < self.config.change_threshold;

            // 6. Narrate
            let observation = self
                .narrator
                .narrate(
                    before_hash,
                    after_hash,
                    &action_desc,
                    before_context.as_ref(),
                    after_context.as_ref(),
                    Some(&before.base64),
                    Some(&after.base64),
                )
                .await;

            let success = !self.config.verify_actions || screen_changed;

            if success || attempts > self.config.max_retries {
                return Ok(ActResult {
                    success,
                    action_taken: action_desc,
                    detection_method: detection.provider.clone(),
                    confidence: detection.confidence,
                    target_x: detection.x,
                    target_y: detection.y,
                    screen_changed,
                    similarity,
                    narration: observation.narration,
                    attempts,
                });
            }

            tracing::warn!(
                attempt = attempts,
                similarity = similarity,
                "Action verification failed (no screen change), retrying"
            );
        }
    }

    /// Extract structured data from the current screen using an LLM.
    ///
    /// Captures a screenshot, builds a multimodal message with the query,
    /// and sends it to the configured LLM for structured extraction.
    pub async fn extract(
        &self,
        query: &str,
        schema: Option<&serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        let llm = self
            .extract_llm
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No LLM configured for extract()"))?;

        let capture = self
            .controller
            .capture()
            .await
            .map_err(|e| anyhow::anyhow!("capture failed: {}", e))?;

        let mut prompt = format!("Look at this screenshot and answer: {query}");
        if let Some(s) = schema {
            prompt.push_str(&format!(
                "\n\nRespond with JSON matching this schema:\n```json\n{}\n```",
                serde_json::to_string_pretty(s)?
            ));
        }

        let message = CvMessage::new(
            "user",
            vec![
                CvContent::Image {
                    media_type: "image/jpeg".to_string(),
                    base64_data: capture.base64,
                },
                CvContent::Text { text: prompt },
            ],
        );

        let request = CvChatRequest {
            messages: vec![message],
            model: None,
            max_tokens: Some(1024),
            temperature: Some(0.1),
        };

        let response = llm
            .chat(request)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        // Try to parse as JSON, or wrap in a value
        serde_json::from_str(&response.text)
            .or_else(|_| Ok(serde_json::json!({ "text": response.text })))
    }

    /// Observe the current screen state without taking any action.
    ///
    /// Returns screenshot, display size, context, and element count.
    pub async fn observe(&self) -> anyhow::Result<ObserveResult> {
        let capture = self
            .controller
            .capture()
            .await
            .map_err(|e| anyhow::anyhow!("capture failed: {}", e))?;

        let context = self.controller.context_info();
        let element_count = context
            .as_ref()
            .and_then(|c| c.interactive_elements.as_ref())
            .map(|e| e.len())
            .unwrap_or(0);

        let context_json = context
            .as_ref()
            .map(|c| serde_json::to_value(c).unwrap_or_default());

        Ok(ObserveResult {
            screenshot: capture.base64,
            display_width: capture.display_width,
            display_height: capture.display_height,
            context: context_json,
            element_count,
        })
    }

    /// Execute the action on the screen controller.
    async fn execute_action(
        &self,
        parsed: &ParsedInstruction,
        x: u32,
        y: u32,
    ) -> anyhow::Result<String> {
        use crate::types::MouseButton;

        match &parsed.action {
            ActionType::Click => {
                self.controller
                    .click(x, y, MouseButton::Left)
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                Ok(format!("Clicked at ({}, {})", x, y))
            }
            ActionType::Type => {
                let text = parsed.text.as_deref().unwrap_or("");
                // Click target first, then type
                self.controller
                    .click(x, y, MouseButton::Left)
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                self.controller
                    .type_text(text)
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                Ok(format!("Typed '{}' at ({}, {})", text, x, y))
            }
            ActionType::Scroll => {
                let (dx, dy) = match &parsed.direction {
                    Some(ScrollDir::Up) => (0.0, -300.0),
                    Some(ScrollDir::Down) => (0.0, 300.0),
                    Some(ScrollDir::Left) => (-300.0, 0.0),
                    Some(ScrollDir::Right) => (300.0, 0.0),
                    None => (0.0, 300.0),
                };
                self.controller
                    .scroll(dx, dy)
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                Ok(format!(
                    "Scrolled {}",
                    parsed
                        .direction
                        .as_ref()
                        .map(|d| format!("{:?}", d).to_lowercase())
                        .unwrap_or_else(|| "down".into())
                ))
            }
            ActionType::KeyPress => {
                let key = parsed.key.as_deref().unwrap_or("Enter");
                self.controller
                    .key_press(key, &[])
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                Ok(format!("Pressed {}", key))
            }
            ActionType::Select => {
                self.controller
                    .click(x, y, MouseButton::Left)
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
                Ok(format!("Selected at ({}, {})", x, y))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_click_instruction() {
        let p = ParsedInstruction::parse("Click the Submit button").unwrap();
        assert_eq!(p.action, ActionType::Click);
        assert_eq!(p.target, "Submit button");
    }

    #[test]
    fn test_parse_click_on() {
        let p = ParsedInstruction::parse("Click on the login link").unwrap();
        assert_eq!(p.action, ActionType::Click);
        assert_eq!(p.target, "login link");
    }

    #[test]
    fn test_parse_type_instruction() {
        let p = ParsedInstruction::parse("Type 'hello' in the search box").unwrap();
        assert_eq!(p.action, ActionType::Type);
        assert_eq!(p.target, "search box");
        assert_eq!(p.text, Some("hello".into()));
    }

    #[test]
    fn test_parse_type_double_quotes() {
        let p = ParsedInstruction::parse(r#"Type "world" into the input"#).unwrap();
        assert_eq!(p.action, ActionType::Type);
        assert_eq!(p.target, "input");
        assert_eq!(p.text, Some("world".into()));
    }

    #[test]
    fn test_parse_scroll_down() {
        let p = ParsedInstruction::parse("Scroll down").unwrap();
        assert_eq!(p.action, ActionType::Scroll);
        assert_eq!(p.direction, Some(ScrollDir::Down));
    }

    #[test]
    fn test_parse_scroll_up() {
        let p = ParsedInstruction::parse("Scroll up").unwrap();
        assert_eq!(p.action, ActionType::Scroll);
        assert_eq!(p.direction, Some(ScrollDir::Up));
    }

    #[test]
    fn test_parse_key_press() {
        let p = ParsedInstruction::parse("Press Enter").unwrap();
        assert_eq!(p.action, ActionType::KeyPress);
        assert_eq!(p.key, Some("Enter".into()));
    }

    #[test]
    fn test_parse_select() {
        let p = ParsedInstruction::parse("Select Option A").unwrap();
        assert_eq!(p.action, ActionType::Select);
        assert_eq!(p.target, "Option A");
    }

    #[test]
    fn test_parse_unrecognized() {
        let err = ParsedInstruction::parse("Do something weird").unwrap_err();
        assert!(err.to_string().contains("unrecognized"));
    }

    #[test]
    fn test_pipeline_config_defaults() {
        let config = PipelineConfig::default();
        assert_eq!(config.max_retries, 2);
        assert_eq!(config.settle_delay_ms, 500);
        assert!(config.verify_actions);
        assert!((config.change_threshold - 0.85).abs() < f32::EPSILON);
    }

    #[test]
    fn test_target_text() {
        let p = ParsedInstruction::parse("Click Submit").unwrap();
        assert_eq!(p.target_text(), "Submit");
    }
}
