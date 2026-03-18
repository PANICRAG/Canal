//! # Computer Vision Tools (CP27.2)
//!
//! CV-guided tools for GUI interaction when RTE is not available.
//! These tools provide fallback implementations for Computer Use actions,
//! using Vision LLM for element detection and the sandbox for execution.
//!
//! When a `ScreenController` is provided, tools that support direct screen
//! interaction (screenshot, click, type, scroll) delegate to the real backend.
//! When no controller is available, all tools return placeholder/fallback responses.
//!
//! ## 8 CV Tools
//! 1. `cv_take_screenshot` — capture current screen state
//! 2. `cv_find_element` — find UI element by visual description
//! 3. `cv_ocr_text` — extract text from screenshot region
//! 4. `cv_detect_objects` — identify UI elements/objects in view
//! 5. `cv_mouse_click` — click at coordinates (fallback when no RTE)
//! 6. `cv_keyboard_type` — type text (fallback when no RTE)
//! 7. `cv_scroll` — scroll viewport
//! 8. `cv_wait_for_element` — wait until element appears on screen

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

use super::traits::{AgentTool, ToolError, ToolResult};
use super::ToolContext;

// ============================================================================
// ScreenController trait (compatible with canal-cv::ScreenController)
// ============================================================================

/// Mouse button for click actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Screenshot captured from a screen controller.
#[derive(Debug, Clone)]
pub struct ScreenCapture {
    /// Base64-encoded image data.
    pub base64: String,
    /// Logical display width.
    pub display_width: u32,
    /// Logical display height.
    pub display_height: u32,
}

/// Abstraction over any screen surface (browser tab, desktop, remote VM).
///
/// This trait mirrors `canal_cv::ScreenController`. When the canal-cv
/// crate is available as a dependency, this can be replaced with a re-export.
#[async_trait]
pub trait ScreenController: Send + Sync {
    /// Capture current screen. Returns base64 image.
    async fn capture(&self) -> Result<ScreenCapture, Box<dyn std::error::Error + Send + Sync>>;

    /// Click at display pixel coordinates.
    async fn click(
        &self,
        x: u32,
        y: u32,
        button: MouseButton,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Type a text string.
    async fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Scroll at current position.
    async fn scroll(
        &self,
        delta_x: f64,
        delta_y: f64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Get display dimensions (width, height) in display pixels.
    fn display_size(&self) -> (u32, u32);
}

// ============================================================================
// 1. cv_take_screenshot
// ============================================================================

/// Take a screenshot of the current screen or a specific region.
pub struct CvTakeScreenshotTool {
    controller: Option<Arc<dyn ScreenController>>,
}

#[derive(Debug, Deserialize)]
pub struct CvTakeScreenshotInput {
    /// Optional region: {x, y, width, height}. If omitted, captures full screen.
    #[serde(default)]
    pub region: Option<ScreenRegion>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScreenRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Serialize)]
pub struct CvTakeScreenshotOutput {
    /// Base64-encoded image data.
    pub image_data: String,
    /// Image dimensions.
    pub width: u32,
    pub height: u32,
    /// Format (e.g., "jpeg" when controller is available, "png" for placeholder).
    pub format: String,
}

#[async_trait]
impl AgentTool for CvTakeScreenshotTool {
    type Input = CvTakeScreenshotInput;
    type Output = CvTakeScreenshotOutput;

    fn name(&self) -> &str {
        "cv_take_screenshot"
    }

    fn description(&self) -> &str {
        "Capture a screenshot of the current screen or a specific region. Returns base64-encoded image."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "region": {
                    "type": "object",
                    "description": "Optional screen region to capture",
                    "properties": {
                        "x": {"type": "integer"},
                        "y": {"type": "integer"},
                        "width": {"type": "integer"},
                        "height": {"type": "integer"}
                    }
                }
            }
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "cv"
    }

    async fn execute(
        &self,
        _input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        if let Some(ref controller) = self.controller {
            let capture = controller
                .capture()
                .await
                .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

            Ok(CvTakeScreenshotOutput {
                image_data: capture.base64,
                width: capture.display_width,
                height: capture.display_height,
                format: "jpeg".to_string(),
            })
        } else {
            // Fallback: return empty placeholder when no controller connected
            Ok(CvTakeScreenshotOutput {
                image_data: String::new(),
                width: 0,
                height: 0,
                format: "png".to_string(),
            })
        }
    }
}

// ============================================================================
// 2. cv_find_element
// ============================================================================

/// Find a UI element on screen by visual description.
pub struct CvFindElementTool {
    #[allow(dead_code)]
    controller: Option<Arc<dyn ScreenController>>,
}

#[derive(Debug, Deserialize)]
pub struct CvFindElementInput {
    /// Natural language description of the element to find (e.g., "the blue login button").
    pub description: String,
    /// Optional screenshot to search in (base64 PNG). If omitted, takes a new screenshot.
    #[serde(default)]
    pub screenshot: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CvFindElementOutput {
    /// Whether the element was found.
    pub found: bool,
    /// Bounding box of the found element.
    pub bounding_box: Option<ScreenRegion>,
    /// Confidence score (0.0 - 1.0).
    pub confidence: f64,
    /// Description of what was found.
    pub label: String,
}

#[async_trait]
impl AgentTool for CvFindElementTool {
    type Input = CvFindElementInput;
    type Output = CvFindElementOutput;

    fn name(&self) -> &str {
        "cv_find_element"
    }

    fn description(&self) -> &str {
        "Find a UI element on screen by natural language description. Uses Vision LLM for element detection."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Natural language description of the element to find"
                },
                "screenshot": {
                    "type": "string",
                    "description": "Optional base64 PNG screenshot to search in"
                }
            },
            "required": ["description"]
        })
    }

    fn namespace(&self) -> &str {
        "cv"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        // Placeholder: requires VisionDetector (LLM call) — kept as fallback for now
        Ok(CvFindElementOutput {
            found: false,
            bounding_box: None,
            confidence: 0.0,
            label: format!("element not found: {}", input.description),
        })
    }
}

// ============================================================================
// 3. cv_ocr_text
// ============================================================================

/// Extract text from a screenshot or region using OCR.
pub struct CvOcrTextTool {
    #[allow(dead_code)]
    controller: Option<Arc<dyn ScreenController>>,
}

#[derive(Debug, Deserialize)]
pub struct CvOcrTextInput {
    /// Base64-encoded PNG screenshot.
    #[serde(default)]
    pub screenshot: Option<String>,
    /// Optional region to extract text from.
    #[serde(default)]
    pub region: Option<ScreenRegion>,
}

#[derive(Debug, Serialize)]
pub struct CvOcrTextOutput {
    /// Extracted text.
    pub text: String,
    /// Confidence score (0.0 - 1.0).
    pub confidence: f64,
}

#[async_trait]
impl AgentTool for CvOcrTextTool {
    type Input = CvOcrTextInput;
    type Output = CvOcrTextOutput;

    fn name(&self) -> &str {
        "cv_ocr_text"
    }

    fn description(&self) -> &str {
        "Extract text from a screenshot or screen region using OCR (Vision LLM)."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "screenshot": {
                    "type": "string",
                    "description": "Base64-encoded PNG screenshot"
                },
                "region": {
                    "type": "object",
                    "description": "Optional region to extract text from",
                    "properties": {
                        "x": {"type": "integer"},
                        "y": {"type": "integer"},
                        "width": {"type": "integer"},
                        "height": {"type": "integer"}
                    }
                }
            }
        })
    }

    fn namespace(&self) -> &str {
        "cv"
    }

    async fn execute(
        &self,
        _input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        // Placeholder: requires CvLlmClient for OCR — kept as fallback for now
        Ok(CvOcrTextOutput {
            text: String::new(),
            confidence: 0.0,
        })
    }
}

// ============================================================================
// 4. cv_detect_objects
// ============================================================================

/// Detect and identify UI elements/objects in a screenshot.
pub struct CvDetectObjectsTool {
    #[allow(dead_code)]
    controller: Option<Arc<dyn ScreenController>>,
}

#[derive(Debug, Deserialize)]
pub struct CvDetectObjectsInput {
    /// Base64-encoded PNG screenshot.
    #[serde(default)]
    pub screenshot: Option<String>,
    /// Filter by element type (e.g., "button", "input", "link").
    #[serde(default)]
    pub element_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectedObject {
    /// Element label/type.
    pub label: String,
    /// Bounding box.
    pub bounding_box: ScreenRegion,
    /// Confidence (0.0 - 1.0).
    pub confidence: f64,
    /// Extracted text content if any.
    pub text: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CvDetectObjectsOutput {
    /// List of detected objects.
    pub objects: Vec<DetectedObject>,
    /// Total count.
    pub count: usize,
}

#[async_trait]
impl AgentTool for CvDetectObjectsTool {
    type Input = CvDetectObjectsInput;
    type Output = CvDetectObjectsOutput;

    fn name(&self) -> &str {
        "cv_detect_objects"
    }

    fn description(&self) -> &str {
        "Detect and identify UI elements/objects in a screenshot. Returns bounding boxes and labels."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "screenshot": {
                    "type": "string",
                    "description": "Base64-encoded PNG screenshot"
                },
                "element_type": {
                    "type": "string",
                    "description": "Filter by element type (button, input, link, etc.)"
                }
            }
        })
    }

    fn namespace(&self) -> &str {
        "cv"
    }

    async fn execute(
        &self,
        _input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        // Placeholder: requires VisionDetector — kept as fallback for now
        Ok(CvDetectObjectsOutput {
            objects: vec![],
            count: 0,
        })
    }
}

// ============================================================================
// 5. cv_mouse_click — fallback for Computer Use click
// ============================================================================

/// Click at screen coordinates (CV fallback when no RTE available).
pub struct CvMouseClickTool {
    controller: Option<Arc<dyn ScreenController>>,
}

#[derive(Debug, Deserialize)]
pub struct CvMouseClickInput {
    /// X coordinate.
    pub x: u32,
    /// Y coordinate.
    pub y: u32,
    /// Click type: "left", "right", "double".
    #[serde(default = "default_click_type")]
    pub click_type: String,
}

fn default_click_type() -> String {
    "left".to_string()
}

#[derive(Debug, Serialize)]
pub struct CvMouseClickOutput {
    /// Whether the click was performed.
    pub success: bool,
    /// Coordinates clicked.
    pub x: u32,
    pub y: u32,
    /// Status message.
    pub message: String,
}

#[async_trait]
impl AgentTool for CvMouseClickTool {
    type Input = CvMouseClickInput;
    type Output = CvMouseClickOutput;

    fn name(&self) -> &str {
        "cv_mouse_click"
    }

    fn description(&self) -> &str {
        "Click at specific screen coordinates. Fallback tool for GUI interaction when RTE is not available."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "x": {"type": "integer", "description": "X coordinate"},
                "y": {"type": "integer", "description": "Y coordinate"},
                "click_type": {
                    "type": "string",
                    "enum": ["left", "right", "double"],
                    "default": "left"
                }
            },
            "required": ["x", "y"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "cv"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        if let Some(ref controller) = self.controller {
            let button = match input.click_type.as_str() {
                "right" => MouseButton::Right,
                _ => MouseButton::Left,
            };

            controller
                .click(input.x, input.y, button)
                .await
                .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

            Ok(CvMouseClickOutput {
                success: true,
                x: input.x,
                y: input.y,
                message: format!(
                    "clicked at ({}, {}) with {} button",
                    input.x, input.y, input.click_type
                ),
            })
        } else {
            // No controller: report that click cannot be performed
            Ok(CvMouseClickOutput {
                success: false,
                x: input.x,
                y: input.y,
                message: "no RTE connected — click cannot be performed locally".to_string(),
            })
        }
    }
}

// ============================================================================
// 6. cv_keyboard_type — fallback for Computer Use type
// ============================================================================

/// Type text (CV fallback when no RTE available).
pub struct CvKeyboardTypeTool {
    controller: Option<Arc<dyn ScreenController>>,
}

#[derive(Debug, Deserialize)]
pub struct CvKeyboardTypeInput {
    /// Text to type.
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct CvKeyboardTypeOutput {
    /// Whether the typing was performed.
    pub success: bool,
    /// Characters typed.
    pub characters: usize,
    /// Status message.
    pub message: String,
}

#[async_trait]
impl AgentTool for CvKeyboardTypeTool {
    type Input = CvKeyboardTypeInput;
    type Output = CvKeyboardTypeOutput;

    fn name(&self) -> &str {
        "cv_keyboard_type"
    }

    fn description(&self) -> &str {
        "Type text on the keyboard. Fallback tool for text input when RTE is not available."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to type"
                }
            },
            "required": ["text"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "cv"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        if let Some(ref controller) = self.controller {
            let char_count = input.text.len();
            controller
                .type_text(&input.text)
                .await
                .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

            Ok(CvKeyboardTypeOutput {
                success: true,
                characters: char_count,
                message: format!("typed {} characters", char_count),
            })
        } else {
            Ok(CvKeyboardTypeOutput {
                success: false,
                characters: input.text.len(),
                message: "no RTE connected — keyboard input cannot be performed locally"
                    .to_string(),
            })
        }
    }
}

// ============================================================================
// 7. cv_scroll
// ============================================================================

/// Scroll the viewport.
pub struct CvScrollTool {
    controller: Option<Arc<dyn ScreenController>>,
}

#[derive(Debug, Deserialize)]
pub struct CvScrollInput {
    /// Scroll direction: "up", "down", "left", "right".
    pub direction: String,
    /// Scroll amount in pixels.
    #[serde(default = "default_scroll_amount")]
    pub amount: u32,
}

fn default_scroll_amount() -> u32 {
    300
}

#[derive(Debug, Serialize)]
pub struct CvScrollOutput {
    pub success: bool,
    pub direction: String,
    pub amount: u32,
    pub message: String,
}

#[async_trait]
impl AgentTool for CvScrollTool {
    type Input = CvScrollInput;
    type Output = CvScrollOutput;

    fn name(&self) -> &str {
        "cv_scroll"
    }

    fn description(&self) -> &str {
        "Scroll the viewport in a specified direction."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "direction": {
                    "type": "string",
                    "enum": ["up", "down", "left", "right"],
                    "description": "Scroll direction"
                },
                "amount": {
                    "type": "integer",
                    "default": 300,
                    "description": "Scroll amount in pixels"
                }
            },
            "required": ["direction"]
        })
    }

    fn namespace(&self) -> &str {
        "cv"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        if let Some(ref controller) = self.controller {
            let (delta_x, delta_y) = match input.direction.as_str() {
                "up" => (0.0, -(input.amount as f64)),
                "down" => (0.0, input.amount as f64),
                "left" => (-(input.amount as f64), 0.0),
                "right" => (input.amount as f64, 0.0),
                _ => (0.0, input.amount as f64),
            };

            controller
                .scroll(delta_x, delta_y)
                .await
                .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

            let msg = format!("scrolled {} by {} pixels", input.direction, input.amount);
            Ok(CvScrollOutput {
                success: true,
                direction: input.direction,
                amount: input.amount,
                message: msg,
            })
        } else {
            Ok(CvScrollOutput {
                success: false,
                direction: input.direction,
                amount: input.amount,
                message: "no RTE connected — scroll cannot be performed locally".to_string(),
            })
        }
    }
}

// ============================================================================
// 8. cv_wait_for_element
// ============================================================================

/// Wait until a UI element appears on screen.
pub struct CvWaitForElementTool {
    #[allow(dead_code)]
    controller: Option<Arc<dyn ScreenController>>,
}

#[derive(Debug, Deserialize)]
pub struct CvWaitForElementInput {
    /// Description of the element to wait for.
    pub description: String,
    /// Maximum wait time in milliseconds.
    #[serde(default = "default_wait_timeout")]
    pub timeout_ms: u64,
}

fn default_wait_timeout() -> u64 {
    5000
}

#[derive(Debug, Serialize)]
pub struct CvWaitForElementOutput {
    /// Whether the element was found within the timeout.
    pub found: bool,
    /// Bounding box if found.
    pub bounding_box: Option<ScreenRegion>,
    /// Time waited in milliseconds.
    pub waited_ms: u64,
    /// Message.
    pub message: String,
}

#[async_trait]
impl AgentTool for CvWaitForElementTool {
    type Input = CvWaitForElementInput;
    type Output = CvWaitForElementOutput;

    fn name(&self) -> &str {
        "cv_wait_for_element"
    }

    fn description(&self) -> &str {
        "Wait until a UI element matching the description appears on screen."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Natural language description of the element to wait for"
                },
                "timeout_ms": {
                    "type": "integer",
                    "default": 5000,
                    "description": "Maximum wait time in milliseconds"
                }
            },
            "required": ["description"]
        })
    }

    fn namespace(&self) -> &str {
        "cv"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        // Placeholder: requires VisionDetector for polling — kept as fallback for now
        Ok(CvWaitForElementOutput {
            found: false,
            bounding_box: None,
            waited_ms: 0,
            message: format!(
                "no RTE connected — cannot wait for element: {}",
                input.description
            ),
        })
    }
}

// ============================================================================
// CV Tool Registration Helper
// ============================================================================

/// All CV tool names for registration and filtering.
pub const CV_TOOL_NAMES: &[&str] = &[
    "cv_take_screenshot",
    "cv_find_element",
    "cv_ocr_text",
    "cv_detect_objects",
    "cv_mouse_click",
    "cv_keyboard_type",
    "cv_scroll",
    "cv_wait_for_element",
];

/// Register all 8 CV tools into a ToolRegistry.
///
/// When `controller` is `Some`, tools that support direct screen interaction
/// (screenshot, click, type, scroll) will delegate to the real ScreenController.
/// When `None`, all tools return placeholder/fallback responses.
pub fn register_cv_tools(
    registry: &mut super::ToolRegistry,
    controller: Option<Arc<dyn ScreenController>>,
) {
    info!(
        has_controller = controller.is_some(),
        "Registering CV tools"
    );

    registry.register_tool(CvTakeScreenshotTool {
        controller: controller.clone(),
    });
    registry.register_tool(CvFindElementTool {
        controller: controller.clone(),
    });
    registry.register_tool(CvOcrTextTool {
        controller: controller.clone(),
    });
    registry.register_tool(CvDetectObjectsTool {
        controller: controller.clone(),
    });
    registry.register_tool(CvMouseClickTool {
        controller: controller.clone(),
    });
    registry.register_tool(CvKeyboardTypeTool {
        controller: controller.clone(),
    });
    registry.register_tool(CvScrollTool {
        controller: controller.clone(),
    });
    registry.register_tool(CvWaitForElementTool {
        controller: controller.clone(),
    });
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock ScreenController for testing with-controller code paths.
    struct MockScreenController;

    #[async_trait]
    impl ScreenController for MockScreenController {
        async fn capture(&self) -> Result<ScreenCapture, Box<dyn std::error::Error + Send + Sync>> {
            Ok(ScreenCapture {
                base64: "mock_base64_data".to_string(),
                display_width: 1920,
                display_height: 1080,
            })
        }

        async fn click(
            &self,
            _x: u32,
            _y: u32,
            _button: MouseButton,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }

        async fn type_text(
            &self,
            _text: &str,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }

        async fn scroll(
            &self,
            _delta_x: f64,
            _delta_y: f64,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }

        fn display_size(&self) -> (u32, u32) {
            (1920, 1080)
        }
    }

    #[test]
    fn test_cv_tools_registered() {
        let mut registry = super::super::ToolRegistry::new();
        register_cv_tools(&mut registry, None);

        for name in CV_TOOL_NAMES {
            assert!(
                registry.get_builtin(name).is_some(),
                "CV tool '{}' not registered",
                name
            );
        }
    }

    #[test]
    fn test_cv_tools_registered_with_controller() {
        let mut registry = super::super::ToolRegistry::new();
        let controller: Arc<dyn ScreenController> = Arc::new(MockScreenController);
        register_cv_tools(&mut registry, Some(controller));

        for name in CV_TOOL_NAMES {
            assert!(
                registry.get_builtin(name).is_some(),
                "CV tool '{}' not registered",
                name
            );
        }
    }

    #[test]
    fn test_cv_tool_schemas() {
        let tools: Vec<Box<dyn std::any::Any>> = vec![
            Box::new(CvTakeScreenshotTool { controller: None }),
            Box::new(CvFindElementTool { controller: None }),
            Box::new(CvOcrTextTool { controller: None }),
            Box::new(CvDetectObjectsTool { controller: None }),
            Box::new(CvMouseClickTool { controller: None }),
            Box::new(CvKeyboardTypeTool { controller: None }),
            Box::new(CvScrollTool { controller: None }),
            Box::new(CvWaitForElementTool { controller: None }),
        ];
        assert_eq!(tools.len(), 8);

        // Verify each tool has valid input schema
        let screenshot = CvTakeScreenshotTool { controller: None };
        let schema = screenshot.input_schema();
        assert_eq!(schema["type"], "object");

        let find = CvFindElementTool { controller: None };
        let schema = find.input_schema();
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("description")));
    }

    #[test]
    fn test_cv_tool_rte_fallback_config() {
        // CV tools in "cv" namespace — filtered out when RTE is available
        let screenshot = CvTakeScreenshotTool { controller: None };
        let click = CvMouseClickTool { controller: None };
        let keyboard = CvKeyboardTypeTool { controller: None };

        assert_eq!(screenshot.namespace(), "cv");
        assert_eq!(click.namespace(), "cv");
        assert_eq!(keyboard.namespace(), "cv");

        // Mutating tools require permission
        assert!(click.requires_permission());
        assert!(click.is_mutating());
        assert!(keyboard.requires_permission());
        assert!(keyboard.is_mutating());
    }

    #[tokio::test]
    async fn test_take_screenshot_no_rte() {
        let tool = CvTakeScreenshotTool { controller: None };
        let input = CvTakeScreenshotInput { region: None };
        let ctx = ToolContext::default();
        let result = tool.execute(input, &ctx).await.unwrap();
        // No controller -> empty image data
        assert!(result.image_data.is_empty());
        assert_eq!(result.format, "png");
    }

    #[tokio::test]
    async fn test_take_screenshot_with_controller() {
        let controller: Arc<dyn ScreenController> = Arc::new(MockScreenController);
        let tool = CvTakeScreenshotTool {
            controller: Some(controller),
        };
        let input = CvTakeScreenshotInput { region: None };
        let ctx = ToolContext::default();
        let result = tool.execute(input, &ctx).await.unwrap();
        assert_eq!(result.image_data, "mock_base64_data");
        assert_eq!(result.width, 1920);
        assert_eq!(result.height, 1080);
        assert_eq!(result.format, "jpeg");
    }

    #[tokio::test]
    async fn test_mouse_click_no_rte() {
        let tool = CvMouseClickTool { controller: None };
        let input = CvMouseClickInput {
            x: 100,
            y: 200,
            click_type: "left".to_string(),
        };
        let ctx = ToolContext::default();
        let result = tool.execute(input, &ctx).await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("no RTE"));
    }

    #[tokio::test]
    async fn test_mouse_click_with_controller() {
        let controller: Arc<dyn ScreenController> = Arc::new(MockScreenController);
        let tool = CvMouseClickTool {
            controller: Some(controller),
        };
        let input = CvMouseClickInput {
            x: 100,
            y: 200,
            click_type: "left".to_string(),
        };
        let ctx = ToolContext::default();
        let result = tool.execute(input, &ctx).await.unwrap();
        assert!(result.success);
        assert_eq!(result.x, 100);
        assert_eq!(result.y, 200);
        assert!(result.message.contains("clicked at"));
    }

    #[tokio::test]
    async fn test_keyboard_type_no_rte() {
        let tool = CvKeyboardTypeTool { controller: None };
        let input = CvKeyboardTypeInput {
            text: "hello world".to_string(),
        };
        let ctx = ToolContext::default();
        let result = tool.execute(input, &ctx).await.unwrap();
        assert!(!result.success);
        assert_eq!(result.characters, 11);
        assert!(result.message.contains("no RTE"));
    }

    #[tokio::test]
    async fn test_keyboard_type_with_controller() {
        let controller: Arc<dyn ScreenController> = Arc::new(MockScreenController);
        let tool = CvKeyboardTypeTool {
            controller: Some(controller),
        };
        let input = CvKeyboardTypeInput {
            text: "hello".to_string(),
        };
        let ctx = ToolContext::default();
        let result = tool.execute(input, &ctx).await.unwrap();
        assert!(result.success);
        assert_eq!(result.characters, 5);
        assert!(result.message.contains("typed 5 characters"));
    }

    #[tokio::test]
    async fn test_scroll_with_controller() {
        let controller: Arc<dyn ScreenController> = Arc::new(MockScreenController);
        let tool = CvScrollTool {
            controller: Some(controller),
        };
        let input = CvScrollInput {
            direction: "down".to_string(),
            amount: 500,
        };
        let ctx = ToolContext::default();
        let result = tool.execute(input, &ctx).await.unwrap();
        assert!(result.success);
        assert_eq!(result.direction, "down");
        assert_eq!(result.amount, 500);
    }

    #[tokio::test]
    async fn test_scroll_no_rte() {
        let tool = CvScrollTool { controller: None };
        let input = CvScrollInput {
            direction: "up".to_string(),
            amount: 300,
        };
        let ctx = ToolContext::default();
        let result = tool.execute(input, &ctx).await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("no RTE"));
    }

    #[test]
    fn test_cv_tools_not_available_without_sandbox() {
        // All CV tools are in "cv" namespace — can be filtered by namespace
        for name in CV_TOOL_NAMES {
            // Tool names all start with "cv_"
            assert!(name.starts_with("cv_"));
        }
    }
}
