//! UI-TARS provider wrapper
//!
//! Combines the OpenRouter provider with UI-TARS parser to provide
//! high-precision click coordinate extraction from screenshots.

use crate::error::{Error, Result};
use crate::llm::providers::openrouter::{OpenRouterConfig, OpenRouterProvider};
use crate::llm::router::{ChatRequest, ContentBlock, LlmProvider, Message};

use super::uitars_parser::{UiTarsAction, UiTarsParser};

/// Configuration for UI-TARS provider
#[derive(Debug, Clone)]
pub struct UiTarsProviderConfig {
    /// OpenRouter configuration
    pub openrouter: OpenRouterConfig,
    /// UI-TARS model ID (e.g., "bytedance/ui-tars-1.5-7b")
    pub model_id: String,
    /// Maximum tokens for response
    pub max_tokens: u32,
    /// Request timeout in seconds
    pub timeout_seconds: u32,
    /// Enable debug logging
    pub debug: bool,
}

impl Default for UiTarsProviderConfig {
    fn default() -> Self {
        Self {
            openrouter: OpenRouterConfig::default(),
            model_id: "bytedance/ui-tars-1.5-7b".to_string(),
            max_tokens: 2048,
            timeout_seconds: 30,
            debug: false,
        }
    }
}

impl UiTarsProviderConfig {
    /// Create config for UI-TARS 7B model
    pub fn ui_tars_7b() -> Self {
        Self {
            model_id: "bytedance/ui-tars-1.5-7b".to_string(),
            max_tokens: 2048,
            ..Default::default()
        }
    }

    /// Create config for UI-TARS 72B model (free tier)
    pub fn ui_tars_72b_free() -> Self {
        Self {
            model_id: "bytedance-research/ui-tars-72b:free".to_string(),
            max_tokens: 4096,
            ..Default::default()
        }
    }
}

/// Result of UI-TARS click detection
#[derive(Debug, Clone)]
pub struct UiTarsClickResult {
    /// X coordinate in CSS pixels
    pub x: u32,
    /// Y coordinate in CSS pixels
    pub y: u32,
    /// Original normalized X (0-1000)
    pub normalized_x: u32,
    /// Original normalized Y (0-1000)
    pub normalized_y: u32,
    /// The reasoning from the model (if available)
    pub thought: Option<String>,
    /// The full parsed action
    pub action: UiTarsAction,
    /// Raw model response
    pub raw_response: String,
}

/// UI-TARS provider for high-precision GUI click detection
///
/// This provider sends screenshots to UI-TARS via OpenRouter and parses
/// the response to extract click coordinates.
///
/// # Example
/// ```ignore
/// let provider = UiTarsProvider::new();
/// let result = provider.get_click_coordinates(
///     &screenshot_base64,
///     "Click the submit button",
///     1920,
///     1080,
/// ).await?;
/// println!("Click at ({}, {})", result.x, result.y);
/// ```
pub struct UiTarsProvider {
    llm: OpenRouterProvider,
    config: UiTarsProviderConfig,
}

impl UiTarsProvider {
    /// Create a new UI-TARS provider with default configuration
    pub fn new() -> Self {
        Self::with_config(UiTarsProviderConfig::default())
    }

    /// Create a new UI-TARS provider with custom configuration
    pub fn with_config(config: UiTarsProviderConfig) -> Self {
        let llm = OpenRouterProvider::with_config(config.openrouter.clone());
        Self { llm, config }
    }

    /// Get click coordinates for a given task on a screenshot
    ///
    /// # Arguments
    /// * `screenshot_base64` - Base64-encoded screenshot image (JPEG or PNG)
    /// * `task` - Description of what to click (e.g., "Click the submit button")
    /// * `image_width` - Width of the screenshot in CSS pixels
    /// * `image_height` - Height of the screenshot in CSS pixels
    ///
    /// # Returns
    /// * `Ok(UiTarsClickResult)` - The detected click coordinates and metadata
    /// * `Err(Error)` - If the request fails or response cannot be parsed
    pub async fn get_click_coordinates(
        &self,
        screenshot_base64: &str,
        task: &str,
        image_width: u32,
        image_height: u32,
    ) -> Result<UiTarsClickResult> {
        // Detect media type from base64 prefix or assume JPEG
        let media_type = if screenshot_base64.starts_with("/9j/") {
            "image/jpeg"
        } else if screenshot_base64.starts_with("iVBOR") {
            "image/png"
        } else {
            "image/jpeg"
        };

        // Build the UI-TARS prompt
        let system_prompt = self.build_system_prompt();
        let user_prompt = self.build_user_prompt(task);

        // Create messages with screenshot
        let messages = vec![
            Message::text("system", system_prompt),
            Message::with_blocks(
                "user",
                vec![
                    ContentBlock::Text {
                        text: user_prompt.clone(),
                    },
                    ContentBlock::Image {
                        source_type: "base64".to_string(),
                        media_type: media_type.to_string(),
                        data: screenshot_base64.to_string(),
                    },
                ],
            ),
        ];

        // Build request
        let request = ChatRequest {
            messages,
            model: Some(self.config.model_id.clone()),
            max_tokens: Some(self.config.max_tokens),
            temperature: Some(0.1), // Low temperature for deterministic output
            tools: vec![],
            task_type: Some("gui_click".to_string()),
            ..Default::default()
        };

        if self.config.debug {
            tracing::debug!(
                model = %self.config.model_id,
                task = %task,
                image_width = image_width,
                image_height = image_height,
                "Sending UI-TARS request"
            );
        }

        // Send request to UI-TARS via OpenRouter
        let response = self.llm.chat(request).await?;

        // Extract text response
        let raw_response = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        if self.config.debug {
            tracing::debug!(
                response = %raw_response,
                "UI-TARS raw response"
            );
        }

        // Parse the response
        let (thought, action) = UiTarsParser::parse_full(&raw_response)?;

        // Extract coordinates from action (still in normalized 0-1000 range)
        let (normalized_x, normalized_y) = match &action {
            UiTarsAction::Click { x, y } => (*x, *y),
            UiTarsAction::DoubleClick { x, y } => (*x, *y),
            UiTarsAction::RightClick { x, y } => (*x, *y),
            UiTarsAction::Scroll { x, y, .. } => (*x, *y),
            _ => {
                return Err(Error::InvalidInput(format!(
                    "UI-TARS returned non-clickable action: {:?}",
                    action
                )));
            }
        };

        // Convert to CSS pixels
        let (css_x, css_y) =
            UiTarsParser::to_css_pixels(normalized_x, normalized_y, image_width, image_height);

        // Convert action to CSS pixels as well
        let css_action =
            UiTarsParser::action_to_css_pixels(action.clone(), image_width, image_height);

        if self.config.debug {
            tracing::info!(
                normalized_x = normalized_x,
                normalized_y = normalized_y,
                css_x = css_x,
                css_y = css_y,
                thought = ?thought,
                action = ?css_action,
                "UI-TARS click detection result"
            );
        }

        Ok(UiTarsClickResult {
            x: css_x,
            y: css_y,
            normalized_x,
            normalized_y,
            thought,
            action: css_action,
            raw_response,
        })
    }

    /// Get the next action from UI-TARS for a given task
    ///
    /// This is similar to `get_click_coordinates` but returns any action type,
    /// not just clickable ones.
    ///
    /// # Arguments
    /// * `screenshot_base64` - Base64-encoded screenshot image
    /// * `task` - Description of what to do
    /// * `image_width` - Width of the screenshot in CSS pixels
    /// * `image_height` - Height of the screenshot in CSS pixels
    ///
    /// # Returns
    /// * `Ok((Option<String>, UiTarsAction))` - Thought and action (coordinates in CSS pixels)
    /// * `Err(Error)` - If the request fails or response cannot be parsed
    pub async fn get_next_action(
        &self,
        screenshot_base64: &str,
        task: &str,
        image_width: u32,
        image_height: u32,
    ) -> Result<(Option<String>, UiTarsAction)> {
        // Detect media type
        let media_type = if screenshot_base64.starts_with("/9j/") {
            "image/jpeg"
        } else if screenshot_base64.starts_with("iVBOR") {
            "image/png"
        } else {
            "image/jpeg"
        };

        // Build the UI-TARS prompt
        let system_prompt = self.build_system_prompt();
        let user_prompt = self.build_user_prompt(task);

        // Create messages
        let messages = vec![
            Message::text("system", system_prompt),
            Message::with_blocks(
                "user",
                vec![
                    ContentBlock::Text { text: user_prompt },
                    ContentBlock::Image {
                        source_type: "base64".to_string(),
                        media_type: media_type.to_string(),
                        data: screenshot_base64.to_string(),
                    },
                ],
            ),
        ];

        // Build request
        let request = ChatRequest {
            messages,
            model: Some(self.config.model_id.clone()),
            max_tokens: Some(self.config.max_tokens),
            temperature: Some(0.1),
            tools: vec![],
            task_type: Some("gui_click".to_string()),
            ..Default::default()
        };

        // Send request
        let response = self.llm.chat(request).await?;

        // Extract and parse response
        let raw_response = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        let (thought, action) = UiTarsParser::parse_full(&raw_response)?;

        // Convert action coordinates to CSS pixels
        let css_action = UiTarsParser::action_to_css_pixels(action, image_width, image_height);

        Ok((thought, css_action))
    }

    /// Check if UI-TARS is available (has valid API key)
    pub async fn is_available(&self) -> bool {
        self.llm.is_available().await
    }

    /// Build the system prompt for UI-TARS
    fn build_system_prompt(&self) -> String {
        r#"You are UI-TARS, a vision-language model specialized in GUI automation.
Your task is to analyze screenshots and determine the precise location to interact with UI elements.

When given a task, you should:
1. Analyze the screenshot to understand the current UI state
2. Identify the target element that matches the task description
3. Output your reasoning and the action to perform

Output format:
Thought: <your reasoning about what you see and what action to take>
Action: <the action to perform>

Available actions:
- click(start_box='(x,y)') - Single left click at normalized coordinates
- left_double(start_box='(x,y)') - Double left click
- right_single(start_box='(x,y)') - Single right click
- drag(start_box='(x1,y1)', end_box='(x2,y2)') - Drag from start to end
- type(content='text') - Type text
- scroll(start_box='(x,y)', direction='up/down/left/right') - Scroll
- hotkey(key='ctrl+c') - Press hotkey combination
- wait(time='seconds') - Wait for specified time

IMPORTANT: Coordinates are normalized to 0-1000 range, where (0,0) is top-left and (1000,1000) is bottom-right."#.to_string()
    }

    /// Build the user prompt for UI-TARS
    fn build_user_prompt(&self, task: &str) -> String {
        format!(
            "Task: {}\n\nAnalyze the screenshot and determine the action to complete this task.",
            task
        )
    }
}

impl Default for UiTarsProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = UiTarsProviderConfig::default();
        assert_eq!(config.model_id, "bytedance/ui-tars-1.5-7b");
        assert_eq!(config.max_tokens, 2048);
        assert_eq!(config.timeout_seconds, 30);
    }

    #[test]
    fn test_ui_tars_7b_config() {
        let config = UiTarsProviderConfig::ui_tars_7b();
        assert_eq!(config.model_id, "bytedance/ui-tars-1.5-7b");
        assert_eq!(config.max_tokens, 2048);
    }

    #[test]
    fn test_ui_tars_72b_config() {
        let config = UiTarsProviderConfig::ui_tars_72b_free();
        assert_eq!(config.model_id, "bytedance-research/ui-tars-72b:free");
        assert_eq!(config.max_tokens, 4096);
    }

    #[test]
    fn test_system_prompt() {
        let provider = UiTarsProvider::new();
        let prompt = provider.build_system_prompt();
        assert!(prompt.contains("UI-TARS"));
        assert!(prompt.contains("click"));
        assert!(prompt.contains("0-1000"));
    }

    #[test]
    fn test_user_prompt() {
        let provider = UiTarsProvider::new();
        let prompt = provider.build_user_prompt("Click the submit button");
        assert!(prompt.contains("Click the submit button"));
        assert!(prompt.contains("Task:"));
    }
}
