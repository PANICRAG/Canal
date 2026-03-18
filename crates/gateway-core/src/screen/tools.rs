//! Screen-based Computer Use tools.
//!
//! Thin wrappers over `ScreenController` + `ComputerUsePipeline` that register
//! as agent tools. These replace the legacy `browser::computer_use` tools while
//! keeping identical tool names so LLM behavior is unchanged.

use async_trait::async_trait;
use gateway_tool_types::{AgentTool, ToolError, ToolResult};
use canal_cv::{Modifier, MouseButton, ScreenController};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::info;

use super::CdpScreenController;
use crate::agent::tools::registry::ToolRegistry;

// ---------------------------------------------------------------------------
// computer_screenshot
// ---------------------------------------------------------------------------

/// Tool to capture a screenshot of the current screen.
pub struct ScreenshotTool {
    controller: Arc<dyn ScreenController>,
}

#[derive(Debug, Deserialize)]
pub struct ScreenshotInput {}

#[derive(Debug, Serialize)]
pub struct ScreenshotOutput {
    /// Base64 JPEG image data.
    pub image_data: String,
    /// Display width in CSS pixels.
    pub display_width: u32,
    /// Display height in CSS pixels.
    pub display_height: u32,
}

#[async_trait]
impl AgentTool for ScreenshotTool {
    type Input = ScreenshotInput;
    type Output = ScreenshotOutput;

    fn name(&self) -> &str {
        "computer_screenshot"
    }
    fn description(&self) -> &str {
        "Capture a screenshot of the current browser page. Returns a base64 JPEG image."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }
    fn requires_permission(&self) -> bool {
        false
    }
    fn namespace(&self) -> &str {
        "screen"
    }

    async fn execute(
        &self,
        _input: Self::Input,
        _context: &gateway_tool_types::ToolContext,
    ) -> ToolResult<Self::Output> {
        let capture = self
            .controller
            .capture()
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        Ok(ScreenshotOutput {
            image_data: capture.base64,
            display_width: capture.display_width,
            display_height: capture.display_height,
        })
    }
}

// ---------------------------------------------------------------------------
// computer_click
// ---------------------------------------------------------------------------

/// Tool to click at coordinates on the screen.
pub struct ClickTool {
    controller: Arc<dyn ScreenController>,
}

#[derive(Debug, Deserialize)]
pub struct ClickInput {
    /// X coordinate in display pixels.
    pub x: u32,
    /// Y coordinate in display pixels.
    pub y: u32,
    /// Mouse button: "left", "right", or "middle".
    #[serde(default = "default_button")]
    pub button: String,
}

fn default_button() -> String {
    "left".to_string()
}

#[derive(Debug, Serialize)]
pub struct ClickOutput {
    pub clicked_at: (u32, u32),
}

#[async_trait]
impl AgentTool for ClickTool {
    type Input = ClickInput;
    type Output = ClickOutput;

    fn name(&self) -> &str {
        "computer_click"
    }
    fn description(&self) -> &str {
        "Click at display pixel coordinates on the browser page. Use computer_screenshot first to identify coordinates."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "x": { "type": "integer", "description": "X coordinate in display pixels" },
                "y": { "type": "integer", "description": "Y coordinate in display pixels" },
                "button": {
                    "type": "string",
                    "enum": ["left", "right", "middle"],
                    "default": "left",
                    "description": "Mouse button"
                }
            },
            "required": ["x", "y"]
        })
    }
    fn is_mutating(&self) -> bool {
        true
    }
    fn namespace(&self) -> &str {
        "screen"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &gateway_tool_types::ToolContext,
    ) -> ToolResult<Self::Output> {
        let button = match input.button.as_str() {
            "right" => MouseButton::Right,
            "middle" => MouseButton::Middle,
            _ => MouseButton::Left,
        };

        self.controller
            .click(input.x, input.y, button)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        Ok(ClickOutput {
            clicked_at: (input.x, input.y),
        })
    }
}

// ---------------------------------------------------------------------------
// computer_type
// ---------------------------------------------------------------------------

/// Tool to type text on the screen.
pub struct TypeTool {
    controller: Arc<dyn ScreenController>,
}

#[derive(Debug, Deserialize)]
pub struct TypeInput {
    /// Text to type.
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct TypeOutput {
    pub typed: String,
}

#[async_trait]
impl AgentTool for TypeTool {
    type Input = TypeInput;
    type Output = TypeOutput;

    fn name(&self) -> &str {
        "computer_type"
    }
    fn description(&self) -> &str {
        "Type text on the browser page. Click on the target input field first using computer_click."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Text to type" }
            },
            "required": ["text"]
        })
    }
    fn is_mutating(&self) -> bool {
        true
    }
    fn namespace(&self) -> &str {
        "screen"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &gateway_tool_types::ToolContext,
    ) -> ToolResult<Self::Output> {
        self.controller
            .type_text(&input.text)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        Ok(TypeOutput { typed: input.text })
    }
}

// ---------------------------------------------------------------------------
// computer_key
// ---------------------------------------------------------------------------

/// Tool to press a key with optional modifiers.
pub struct KeyTool {
    controller: Arc<dyn ScreenController>,
}

#[derive(Debug, Deserialize)]
pub struct KeyInput {
    /// Key to press (e.g., "Enter", "Tab", "Escape").
    pub key: String,
    /// Modifier keys: "shift", "control", "alt", "meta".
    #[serde(default)]
    pub modifiers: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct KeyOutput {
    pub pressed: String,
}

#[async_trait]
impl AgentTool for KeyTool {
    type Input = KeyInput;
    type Output = KeyOutput;

    fn name(&self) -> &str {
        "computer_key"
    }
    fn description(&self) -> &str {
        "Press a key with optional modifiers (e.g., Enter, Tab, Ctrl+C)."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "key": { "type": "string", "description": "Key to press" },
                "modifiers": {
                    "type": "array",
                    "items": {
                        "type": "string",
                        "enum": ["shift", "control", "alt", "meta"]
                    },
                    "description": "Optional modifier keys"
                }
            },
            "required": ["key"]
        })
    }
    fn is_mutating(&self) -> bool {
        true
    }
    fn namespace(&self) -> &str {
        "screen"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &gateway_tool_types::ToolContext,
    ) -> ToolResult<Self::Output> {
        let modifiers: Vec<Modifier> = input
            .modifiers
            .iter()
            .filter_map(|m| match m.to_lowercase().as_str() {
                "shift" => Some(Modifier::Shift),
                "control" | "ctrl" => Some(Modifier::Control),
                "alt" => Some(Modifier::Alt),
                "meta" | "cmd" | "command" => Some(Modifier::Meta),
                _ => None,
            })
            .collect();

        self.controller
            .key_press(&input.key, &modifiers)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        let desc = if modifiers.is_empty() {
            input.key.clone()
        } else {
            format!("{}+{}", input.modifiers.join("+"), input.key)
        };

        Ok(KeyOutput { pressed: desc })
    }
}

// ---------------------------------------------------------------------------
// computer_scroll
// ---------------------------------------------------------------------------

/// Tool to scroll the page.
pub struct ScrollTool {
    controller: Arc<dyn ScreenController>,
}

#[derive(Debug, Deserialize)]
pub struct ScrollInput {
    /// Horizontal scroll delta.
    #[serde(default)]
    pub delta_x: f64,
    /// Vertical scroll delta (positive = down).
    #[serde(default = "default_scroll_delta")]
    pub delta_y: f64,
}

fn default_scroll_delta() -> f64 {
    300.0
}

#[derive(Debug, Serialize)]
pub struct ScrollOutput {
    pub scrolled: (f64, f64),
}

#[async_trait]
impl AgentTool for ScrollTool {
    type Input = ScrollInput;
    type Output = ScrollOutput;

    fn name(&self) -> &str {
        "computer_scroll"
    }
    fn description(&self) -> &str {
        "Scroll the browser page. Positive delta_y scrolls down, negative scrolls up."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "delta_x": { "type": "number", "description": "Horizontal scroll delta", "default": 0 },
                "delta_y": { "type": "number", "description": "Vertical scroll delta (positive=down)", "default": 300 }
            },
            "required": []
        })
    }
    fn is_mutating(&self) -> bool {
        true
    }
    fn namespace(&self) -> &str {
        "screen"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &gateway_tool_types::ToolContext,
    ) -> ToolResult<Self::Output> {
        self.controller
            .scroll(input.delta_x, input.delta_y)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        Ok(ScrollOutput {
            scrolled: (input.delta_x, input.delta_y),
        })
    }
}

// ---------------------------------------------------------------------------
// computer_drag
// ---------------------------------------------------------------------------

/// Tool to drag from one point to another.
pub struct DragTool {
    controller: Arc<dyn ScreenController>,
}

#[derive(Debug, Deserialize)]
pub struct DragInput {
    pub from_x: u32,
    pub from_y: u32,
    pub to_x: u32,
    pub to_y: u32,
}

#[derive(Debug, Serialize)]
pub struct DragOutput {
    pub from: (u32, u32),
    pub to: (u32, u32),
}

#[async_trait]
impl AgentTool for DragTool {
    type Input = DragInput;
    type Output = DragOutput;

    fn name(&self) -> &str {
        "computer_drag"
    }
    fn description(&self) -> &str {
        "Drag from one point to another on the screen."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "from_x": { "type": "integer", "description": "Start X coordinate" },
                "from_y": { "type": "integer", "description": "Start Y coordinate" },
                "to_x": { "type": "integer", "description": "End X coordinate" },
                "to_y": { "type": "integer", "description": "End Y coordinate" }
            },
            "required": ["from_x", "from_y", "to_x", "to_y"]
        })
    }
    fn is_mutating(&self) -> bool {
        true
    }
    fn namespace(&self) -> &str {
        "screen"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &gateway_tool_types::ToolContext,
    ) -> ToolResult<Self::Output> {
        self.controller
            .drag(input.from_x, input.from_y, input.to_x, input.to_y)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        Ok(DragOutput {
            from: (input.from_x, input.from_y),
            to: (input.to_x, input.to_y),
        })
    }
}

// ---------------------------------------------------------------------------
// computer_navigate (CDP-specific, not in ScreenController trait)
// ---------------------------------------------------------------------------

/// Tool to navigate the browser to a URL.
pub struct NavigateTool {
    cdp: Arc<CdpScreenController>,
}

#[derive(Debug, Deserialize)]
pub struct NavigateInput {
    /// URL to navigate to.
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct NavigateOutput {
    pub navigated_to: String,
}

#[async_trait]
impl AgentTool for NavigateTool {
    type Input = NavigateInput;
    type Output = NavigateOutput;

    fn name(&self) -> &str {
        "computer_navigate"
    }
    fn description(&self) -> &str {
        "Navigate the browser to a URL."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "URL to navigate to" }
            },
            "required": ["url"]
        })
    }
    fn is_mutating(&self) -> bool {
        true
    }
    fn namespace(&self) -> &str {
        "screen"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &gateway_tool_types::ToolContext,
    ) -> ToolResult<Self::Output> {
        self.cdp
            .navigate(&input.url)
            .await
            .map_err(|e| ToolError::ExecutionError(e.to_string()))?;

        Ok(NavigateOutput {
            navigated_to: input.url,
        })
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// All screen tool names for filter context matching.
pub const SCREEN_TOOL_NAMES: &[&str] = &[
    "computer_screenshot",
    "computer_click",
    "computer_type",
    "computer_key",
    "computer_scroll",
    "computer_drag",
    "computer_navigate",
];

/// Register all screen tools backed by a ScreenController.
///
/// When `cdp` is provided, also registers `computer_navigate`.
pub fn register_screen_tools(
    registry: &mut ToolRegistry,
    controller: Arc<dyn ScreenController>,
    cdp: Option<Arc<CdpScreenController>>,
) {
    info!("Registering screen tools (ScreenController-backed)");

    registry.register_tool(ScreenshotTool {
        controller: controller.clone(),
    });
    registry.register_tool(ClickTool {
        controller: controller.clone(),
    });
    registry.register_tool(TypeTool {
        controller: controller.clone(),
    });
    registry.register_tool(KeyTool {
        controller: controller.clone(),
    });
    registry.register_tool(ScrollTool {
        controller: controller.clone(),
    });
    registry.register_tool(DragTool {
        controller: controller.clone(),
    });

    // Navigate (CDP-specific)
    if let Some(cdp) = cdp {
        registry.register_tool(NavigateTool { cdp });
    }
}
