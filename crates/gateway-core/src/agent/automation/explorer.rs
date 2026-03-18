//! CV Explorer - Layer 2 of the Five-Layer Automation Architecture
//!
//! Uses Computer Vision (screenshots) to understand page structure and
//! generates a PageSchema that can be used for script generation.
//!
//! Token cost: Fixed ~3000-5000 tokens regardless of data volume.

use super::types::{
    ActionParameter, ActionSchema, BoundingBox, Coordinates, ElementSchema, ElementType,
    PageSchema, ParameterType, Viewport,
};
use crate::llm::LlmRouter;
use canal_cv::ScreenController;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

// ============================================================================
// Error Types
// ============================================================================

#[derive(Error, Debug)]
pub enum ExplorerError {
    #[error("Screenshot failed: {0}")]
    ScreenshotFailed(String),

    #[error("LLM analysis failed: {0}")]
    AnalysisFailed(String),

    #[error("Browser not connected")]
    BrowserNotConnected,

    #[error("Schema parsing failed: {0}")]
    ParseError(String),

    #[error("Timeout")]
    Timeout,
}

// ============================================================================
// Exploration Options
// ============================================================================

/// Options for CV exploration
#[derive(Debug, Clone)]
pub struct ExplorationOptions {
    /// Maximum number of screenshots to take
    pub max_screenshots: u32,
    /// Whether to scroll and explore beyond viewport
    pub explore_beyond_viewport: bool,
    /// Whether to detect interactive elements
    pub detect_interactive: bool,
    /// Whether to include element coordinates
    pub include_coordinates: bool,
    /// Whether to generate CSS selectors
    pub generate_selectors: bool,
    /// Target viewport size
    pub viewport: Option<Viewport>,
    /// Focus area (if known)
    pub focus_selector: Option<String>,
    /// Timeout in milliseconds
    pub timeout_ms: u64,
}

impl Default for ExplorationOptions {
    fn default() -> Self {
        Self {
            max_screenshots: 3,
            explore_beyond_viewport: true,
            detect_interactive: true,
            include_coordinates: true,
            generate_selectors: true,
            viewport: None,
            focus_selector: None,
            timeout_ms: 30000,
        }
    }
}

// ============================================================================
// Exploration Result
// ============================================================================

/// Result of CV exploration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorationResult {
    /// Generated page schema
    pub schema: PageSchema,
    /// Screenshots taken (base64)
    pub screenshots: Vec<ScreenshotInfo>,
    /// Token usage
    pub tokens_used: u64,
    /// Exploration duration in milliseconds
    pub duration_ms: u64,
    /// Warnings encountered
    pub warnings: Vec<String>,
}

/// Information about a screenshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotInfo {
    /// Screenshot index
    pub index: u32,
    /// Viewport offset (for scrolled captures)
    pub scroll_offset: u32,
    /// Base64 encoded image data
    pub data: String,
    /// Image dimensions
    pub width: u32,
    pub height: u32,
}

// ============================================================================
// CV Explorer
// ============================================================================

/// CV Explorer - Analyzes pages via screenshots
#[allow(dead_code)]
pub struct CvExplorer {
    /// Screen controller for taking screenshots
    screen_controller: Arc<dyn ScreenController>,
    /// LLM router for analyzing screenshots
    llm_router: Arc<LlmRouter>,
    /// Configuration
    config: CvExplorerConfig,
}

/// Configuration for the CV explorer
#[derive(Debug, Clone)]
pub struct CvExplorerConfig {
    /// Model to use for analysis
    pub model: String,
    /// Maximum tokens for analysis
    pub max_tokens: u32,
    /// System prompt for the LLM
    pub system_prompt: String,
}

impl Default for CvExplorerConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 4096,
            system_prompt: Self::default_system_prompt(),
        }
    }
}

impl CvExplorerConfig {
    fn default_system_prompt() -> String {
        r#"You are a UI analysis expert. Analyze the screenshot and identify:

1. **Interactive Elements**: Buttons, inputs, links, cells, menus
2. **Element Locations**: Provide approximate coordinates (x, y) for clicking
3. **Selectors**: CSS selectors when visible in the DOM
4. **Actions**: What actions can be performed on each element

Output a JSON PageSchema with:
- elements: Array of ElementSchema
- actions: Array of ActionSchema

For each element include:
- id: Unique identifier
- element_type: button/input/cell/link/etc
- coordinates: {x, y} for clicking
- description: What this element does
- selector: CSS selector if identifiable

For canvas-based apps (Google Sheets, Figma), focus on coordinate-based identification.
For DOM-based apps, include both selectors and coordinates.

Be concise and focus on actionable elements only."#
            .to_string()
    }
}

impl CvExplorer {
    /// Create a new CV explorer
    pub fn new(screen_controller: Arc<dyn ScreenController>, llm_router: Arc<LlmRouter>) -> Self {
        Self {
            screen_controller,
            llm_router,
            config: CvExplorerConfig::default(),
        }
    }

    /// Create a builder
    pub fn builder() -> CvExplorerBuilder {
        CvExplorerBuilder::default()
    }

    /// Explore a page and generate schema
    pub async fn explore(
        &self,
        url: &str,
        _options: ExplorationOptions,
    ) -> Result<ExplorationResult, ExplorerError> {
        let start = std::time::Instant::now();
        let warnings = Vec::new();
        let mut screenshots = Vec::new();

        // 1. Navigate to the URL if needed
        self.navigate_if_needed(url).await?;

        // 2. Take initial screenshot
        let screenshot = self.take_screenshot(0, 0).await?;
        screenshots.push(screenshot.clone());

        // 3. Analyze with LLM (placeholder - returns empty schema)
        let schema = self.analyze_screenshot_placeholder(&screenshot, url)?;

        // Token count estimate for exploration
        let tokens_used = 3500; // Fixed exploration cost

        Ok(ExplorationResult {
            schema,
            screenshots,
            tokens_used,
            duration_ms: start.elapsed().as_millis() as u64,
            warnings,
        })
    }

    /// Navigate to URL if not already there
    async fn navigate_if_needed(&self, _url: &str) -> Result<(), ExplorerError> {
        // Navigation is now handled externally via CdpScreenController::navigate()
        // The explorer focuses on screenshot analysis only.
        // If we have a CdpScreenController, the caller should navigate before calling explore().
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        Ok(())
    }

    /// Take a screenshot via ScreenController
    async fn take_screenshot(
        &self,
        index: u32,
        scroll_offset: u32,
    ) -> Result<ScreenshotInfo, ExplorerError> {
        let capture = self
            .screen_controller
            .capture()
            .await
            .map_err(|e| ExplorerError::ScreenshotFailed(e.to_string()))?;

        Ok(ScreenshotInfo {
            index,
            scroll_offset,
            data: capture.base64,
            width: capture.display_width,
            height: capture.display_height,
        })
    }

    /// Placeholder for LLM analysis - returns a basic schema
    /// TODO: Implement actual LLM vision analysis
    fn analyze_screenshot_placeholder(
        &self,
        _screenshot: &ScreenshotInfo,
        url: &str,
    ) -> Result<PageSchema, ExplorerError> {
        // Return a minimal schema for now
        // In production, this would use the LLM to analyze the screenshot
        let schema = PageSchema::new(url, "Analyzed Page");
        Ok(schema)
    }

    /// Build the analysis prompt
    #[allow(dead_code)]
    fn build_analysis_prompt(&self, url: &str, options: &ExplorationOptions) -> String {
        let mut prompt = format!("Analyze this screenshot of: {}\n\n", url);

        if options.detect_interactive {
            prompt.push_str("Focus on interactive elements that can be clicked or typed into.\n");
        }

        if options.include_coordinates {
            prompt.push_str("Include pixel coordinates (x, y) for each element.\n");
        }

        if options.generate_selectors {
            prompt.push_str("Generate CSS selectors where possible.\n");
        }

        prompt.push_str("\nOutput valid JSON matching the PageSchema format.");

        prompt
    }

    /// Extract JSON from LLM response (may be wrapped in markdown)
    #[allow(dead_code)]
    fn extract_json(&self, content: &str) -> String {
        // Try to find JSON block
        if let Some(start) = content.find("```json") {
            if let Some(end) = content[start..]
                .find("```\n")
                .or_else(|| content[start..].rfind("```"))
            {
                let json_start = start + 7;
                let json_end = start + end;
                if json_start < json_end && json_end <= content.len() {
                    return content[json_start..json_end].trim().to_string();
                }
            }
        }

        // Try to find raw JSON object
        if let Some(start) = content.find('{') {
            if let Some(end) = content.rfind('}') {
                return content[start..=end].to_string();
            }
        }

        content.to_string()
    }

    /// Parse an element from JSON
    #[allow(dead_code)]
    fn parse_element(&self, value: &serde_json::Value) -> Option<ElementSchema> {
        let id = value.get("id")?.as_str()?.to_string();
        let element_type = self.parse_element_type(
            value
                .get("element_type")
                .and_then(|v| v.as_str())
                .unwrap_or("other"),
        );
        let description = value
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut element = ElementSchema::new(id, element_type, description);

        // Parse coordinates
        if let Some(coords) = value.get("coordinates") {
            let x = coords.get("x").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let y = coords.get("y").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            element.coordinates = Some(Coordinates { x, y });
        }

        // Parse selector
        if let Some(selector) = value.get("selector").and_then(|v| v.as_str()) {
            element.selector = Some(selector.to_string());
        }

        // Parse text
        if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
            element.text = Some(text.to_string());
        }

        // Parse visibility
        if let Some(visible) = value.get("is_visible").and_then(|v| v.as_bool()) {
            element.is_visible = visible;
        }

        // Parse bounds
        if let Some(bounds) = value.get("bounds") {
            element.bounds = Some(BoundingBox {
                x: bounds.get("x").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                y: bounds.get("y").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                width: bounds.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                height: bounds.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            });
        }

        Some(element)
    }

    /// Parse element type from string
    #[allow(dead_code)]
    fn parse_element_type(&self, s: &str) -> ElementType {
        match s.to_lowercase().as_str() {
            "button" | "btn" => ElementType::Button,
            "input" | "textfield" | "text_field" => ElementType::Input,
            "textarea" | "text_area" => ElementType::TextArea,
            "link" | "a" => ElementType::Link,
            "cell" => ElementType::Cell,
            "row" => ElementType::Row,
            "column" | "col" => ElementType::Column,
            "menu" => ElementType::Menu,
            "menuitem" | "menu_item" => ElementType::MenuItem,
            "dropdown" | "select" => ElementType::Dropdown,
            "checkbox" => ElementType::Checkbox,
            "radio" => ElementType::Radio,
            "tab" => ElementType::Tab,
            "modal" | "dialog" => ElementType::Modal,
            "image" | "img" => ElementType::Image,
            "icon" => ElementType::Icon,
            "container" | "div" => ElementType::Container,
            _ => ElementType::Other,
        }
    }

    /// Parse an action from JSON
    #[allow(dead_code)]
    fn parse_action(&self, value: &serde_json::Value) -> Option<ActionSchema> {
        let name = value.get("name")?.as_str()?.to_string();
        let target = value
            .get("target_element_id")
            .or_else(|| value.get("target"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = value
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut action = ActionSchema::new(name, target, description);

        // Parse parameters
        if let Some(params) = value.get("parameters").and_then(|v| v.as_array()) {
            for param in params {
                if let Some(action_param) = self.parse_action_parameter(param) {
                    action.parameters.push(action_param);
                }
            }
        }

        // Parse expected outcome
        if let Some(outcome) = value.get("expected_outcome").and_then(|v| v.as_str()) {
            action.expected_outcome = Some(outcome.to_string());
        }

        Some(action)
    }

    /// Parse an action parameter from JSON
    #[allow(dead_code)]
    fn parse_action_parameter(&self, value: &serde_json::Value) -> Option<ActionParameter> {
        let name = value.get("name")?.as_str()?.to_string();
        let param_type = self.parse_param_type(
            value
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("string"),
        );
        let description = value
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let required = value
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Some(ActionParameter {
            name,
            param_type,
            description,
            required,
            default_value: value.get("default").cloned(),
        })
    }

    /// Parse parameter type from string
    #[allow(dead_code)]
    fn parse_param_type(&self, s: &str) -> ParameterType {
        match s.to_lowercase().as_str() {
            "string" | "text" => ParameterType::String,
            "number" | "int" | "integer" | "float" => ParameterType::Number,
            "boolean" | "bool" => ParameterType::Boolean,
            "coordinates" | "point" => ParameterType::Coordinates,
            "array" | "list" => ParameterType::Array,
            "object" | "dict" => ParameterType::Object,
            _ => ParameterType::String,
        }
    }
}

// ============================================================================
// Builder
// ============================================================================

/// Builder for CvExplorer
#[derive(Default)]
pub struct CvExplorerBuilder {
    screen_controller: Option<Arc<dyn ScreenController>>,
    llm_router: Option<Arc<LlmRouter>>,
    config: CvExplorerConfig,
}

impl CvExplorerBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set screen controller
    pub fn screen_controller(mut self, controller: Arc<dyn ScreenController>) -> Self {
        self.screen_controller = Some(controller);
        self
    }

    /// Set LLM router
    pub fn llm_router(mut self, router: Arc<LlmRouter>) -> Self {
        self.llm_router = Some(router);
        self
    }

    /// Set model
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = model.into();
        self
    }

    /// Set max tokens
    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.config.max_tokens = tokens;
        self
    }

    /// Set system prompt
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.system_prompt = prompt.into();
        self
    }

    /// Build the explorer
    pub fn build(self) -> Result<CvExplorer, ExplorerError> {
        let screen_controller = self
            .screen_controller
            .ok_or(ExplorerError::BrowserNotConnected)?;
        let llm_router = self.llm_router.ok_or(ExplorerError::AnalysisFailed(
            "LLM router not provided".to_string(),
        ))?;

        Ok(CvExplorer {
            screen_controller,
            llm_router,
            config: self.config,
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exploration_options_default() {
        let options = ExplorationOptions::default();
        assert_eq!(options.max_screenshots, 3);
        assert!(options.explore_beyond_viewport);
        assert!(options.detect_interactive);
    }

    #[test]
    fn test_config_default() {
        let config = CvExplorerConfig::default();
        assert_eq!(config.model, "claude-sonnet-4-6");
        assert_eq!(config.max_tokens, 4096);
    }
}
