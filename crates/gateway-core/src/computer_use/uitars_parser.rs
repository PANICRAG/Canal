//! UI-TARS action parser
//!
//! Parses UI-TARS model outputs and converts normalized coordinates to CSS pixels.
//!
//! UI-TARS is a vision-language model that outputs actions in a specific format:
//! ```text
//! Thought: I need to click the submit button
//! Action: click(start_box='(197,456)')
//! ```
//!
//! This parser handles extraction of both the thought/reasoning and the action,
//! converting normalized coordinates (0-1000 range) to CSS pixel coordinates.

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Parsed action from UI-TARS output
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UiTarsAction {
    /// Single left click at coordinates
    Click { x: u32, y: u32 },
    /// Double left click at coordinates
    DoubleClick { x: u32, y: u32 },
    /// Single right click at coordinates
    RightClick { x: u32, y: u32 },
    /// Drag from start to end coordinates
    Drag { start: (u32, u32), end: (u32, u32) },
    /// Type text (keyboard input)
    Type { text: String },
    /// Scroll at coordinates in a direction
    Scroll {
        x: u32,
        y: u32,
        direction: ScrollDirection,
    },
    /// Press a hotkey combination (e.g., "ctrl+c")
    Hotkey { key: String },
    /// Wait for specified seconds
    Wait { seconds: f32 },
}

/// Scroll direction for scroll actions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

impl ScrollDirection {
    /// Parse scroll direction from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "up" => Some(ScrollDirection::Up),
            "down" => Some(ScrollDirection::Down),
            "left" => Some(ScrollDirection::Left),
            "right" => Some(ScrollDirection::Right),
            _ => None,
        }
    }
}

impl std::fmt::Display for ScrollDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScrollDirection::Up => write!(f, "up"),
            ScrollDirection::Down => write!(f, "down"),
            ScrollDirection::Left => write!(f, "left"),
            ScrollDirection::Right => write!(f, "right"),
        }
    }
}

/// UI-TARS coordinate factor (normalized to 0-1000)
pub const COORDINATE_FACTOR: u32 = 1000;

/// Parser for UI-TARS model outputs
///
/// UI-TARS outputs actions in a specific format that includes:
/// - A "Thought:" line with reasoning
/// - An "Action:" line with the action to perform
///
/// Supported action formats:
/// - `click(start_box='(x,y)')`
/// - `left_double(start_box='(x,y)')`
/// - `right_single(start_box='(x,y)')`
/// - `drag(start_box='(x1,y1)', end_box='(x2,y2)')`
/// - `type(content='text')`
/// - `scroll(start_box='(x,y)', direction='down')`
/// - `hotkey(key='ctrl+c')`
/// - `wait(time='2')`
pub struct UiTarsParser;

impl UiTarsParser {
    /// Parse UI-TARS response text into an action
    ///
    /// # Arguments
    /// * `response` - The raw response from UI-TARS model
    ///
    /// # Returns
    /// * `Ok(UiTarsAction)` - The parsed action
    /// * `Err(Error)` - If parsing fails
    ///
    /// # Example
    /// ```ignore
    /// let response = "Thought: I need to click the button\nAction: click(start_box='(197,456)')";
    /// let action = UiTarsParser::parse(response)?;
    /// assert_eq!(action, UiTarsAction::Click { x: 197, y: 456 });
    /// ```
    pub fn parse(response: &str) -> Result<UiTarsAction> {
        // Extract the action line
        let action_line = Self::extract_action_line(response)?;

        // Try each action pattern
        if let Some(action) = Self::try_parse_click(&action_line) {
            return Ok(action);
        }
        if let Some(action) = Self::try_parse_double_click(&action_line) {
            return Ok(action);
        }
        if let Some(action) = Self::try_parse_right_click(&action_line) {
            return Ok(action);
        }
        if let Some(action) = Self::try_parse_drag(&action_line) {
            return Ok(action);
        }
        if let Some(action) = Self::try_parse_type(&action_line) {
            return Ok(action);
        }
        if let Some(action) = Self::try_parse_scroll(&action_line) {
            return Ok(action);
        }
        if let Some(action) = Self::try_parse_hotkey(&action_line) {
            return Ok(action);
        }
        if let Some(action) = Self::try_parse_wait(&action_line) {
            return Ok(action);
        }

        Err(Error::InvalidInput(format!(
            "Unknown action format: {}",
            action_line
        )))
    }

    /// Extract the action line from UI-TARS response
    fn extract_action_line(response: &str) -> Result<String> {
        use std::sync::OnceLock;
        static ACTION_RE: OnceLock<Regex> = OnceLock::new();
        let action_re = ACTION_RE.get_or_init(|| Regex::new(r"(?i)Action:\s*(.+)").unwrap());

        if let Some(captures) = action_re.captures(response) {
            if let Some(action_match) = captures.get(1) {
                return Ok(action_match.as_str().trim().to_string());
            }
        }

        // If no "Action:" prefix, treat the whole response as an action
        // (for cases where only action is provided)
        let trimmed = response.trim();
        if trimmed.is_empty() {
            return Err(Error::InvalidInput("Empty response".to_string()));
        }

        Ok(trimmed.to_string())
    }

    /// Try to parse a click action: click(start_box='(x,y)')
    fn try_parse_click(action: &str) -> Option<UiTarsAction> {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(r#"click\s*\(\s*start_box\s*=\s*['"]?\((\d+)\s*,\s*(\d+)\)['"]?\s*\)"#)
                .unwrap()
        });

        if let Some(captures) = re.captures(action) {
            let x: u32 = captures.get(1)?.as_str().parse().ok()?;
            let y: u32 = captures.get(2)?.as_str().parse().ok()?;
            return Some(UiTarsAction::Click { x, y });
        }
        None
    }

    /// Try to parse a double click action: left_double(start_box='(x,y)')
    fn try_parse_double_click(action: &str) -> Option<UiTarsAction> {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(r#"left_double\s*\(\s*start_box\s*=\s*['"]?\((\d+)\s*,\s*(\d+)\)['"]?\s*\)"#)
                .unwrap()
        });

        if let Some(captures) = re.captures(action) {
            let x: u32 = captures.get(1)?.as_str().parse().ok()?;
            let y: u32 = captures.get(2)?.as_str().parse().ok()?;
            return Some(UiTarsAction::DoubleClick { x, y });
        }
        None
    }

    /// Try to parse a right click action: right_single(start_box='(x,y)')
    fn try_parse_right_click(action: &str) -> Option<UiTarsAction> {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(
                r#"right_single\s*\(\s*start_box\s*=\s*['"]?\((\d+)\s*,\s*(\d+)\)['"]?\s*\)"#,
            )
            .unwrap()
        });

        if let Some(captures) = re.captures(action) {
            let x: u32 = captures.get(1)?.as_str().parse().ok()?;
            let y: u32 = captures.get(2)?.as_str().parse().ok()?;
            return Some(UiTarsAction::RightClick { x, y });
        }
        None
    }

    /// Try to parse a drag action: drag(start_box='(x1,y1)', end_box='(x2,y2)')
    fn try_parse_drag(action: &str) -> Option<UiTarsAction> {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r#"drag\s*\(\s*start_box\s*=\s*['"]?\((\d+)\s*,\s*(\d+)\)['"]?\s*,\s*end_box\s*=\s*['"]?\((\d+)\s*,\s*(\d+)\)['"]?\s*\)"#).unwrap());

        if let Some(captures) = re.captures(action) {
            let x1: u32 = captures.get(1)?.as_str().parse().ok()?;
            let y1: u32 = captures.get(2)?.as_str().parse().ok()?;
            let x2: u32 = captures.get(3)?.as_str().parse().ok()?;
            let y2: u32 = captures.get(4)?.as_str().parse().ok()?;
            return Some(UiTarsAction::Drag {
                start: (x1, y1),
                end: (x2, y2),
            });
        }
        None
    }

    /// Try to parse a type action: type(content='text')
    fn try_parse_type(action: &str) -> Option<UiTarsAction> {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        // Handle both single and double quotes, and escaped quotes within
        let re = RE
            .get_or_init(|| Regex::new(r#"type\s*\(\s*content\s*=\s*['"](.+?)['"]\s*\)"#).unwrap());

        if let Some(captures) = re.captures(action) {
            let text = captures.get(1)?.as_str().to_string();
            // Unescape common escape sequences
            let text = text
                .replace("\\n", "\n")
                .replace("\\t", "\t")
                .replace("\\'", "'")
                .replace("\\\"", "\"");
            return Some(UiTarsAction::Type { text });
        }
        None
    }

    /// Try to parse a scroll action: scroll(start_box='(x,y)', direction='down')
    fn try_parse_scroll(action: &str) -> Option<UiTarsAction> {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r#"scroll\s*\(\s*start_box\s*=\s*['"]?\((\d+)\s*,\s*(\d+)\)['"]?\s*,\s*direction\s*=\s*['"](\w+)['"]\s*\)"#).unwrap());

        if let Some(captures) = re.captures(action) {
            let x: u32 = captures.get(1)?.as_str().parse().ok()?;
            let y: u32 = captures.get(2)?.as_str().parse().ok()?;
            let direction_str = captures.get(3)?.as_str();
            let direction = ScrollDirection::from_str(direction_str)?;
            return Some(UiTarsAction::Scroll { x, y, direction });
        }
        None
    }

    /// Try to parse a hotkey action: hotkey(key='ctrl+c')
    fn try_parse_hotkey(action: &str) -> Option<UiTarsAction> {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        let re =
            RE.get_or_init(|| Regex::new(r#"hotkey\s*\(\s*key\s*=\s*['"](.+?)['"]\s*\)"#).unwrap());

        if let Some(captures) = re.captures(action) {
            let key = captures.get(1)?.as_str().to_string();
            return Some(UiTarsAction::Hotkey { key });
        }
        None
    }

    /// Try to parse a wait action: wait(time='2')
    fn try_parse_wait(action: &str) -> Option<UiTarsAction> {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(r#"wait\s*\(\s*time\s*=\s*['"]?([\d.]+)['"]?\s*\)"#).unwrap()
        });

        if let Some(captures) = re.captures(action) {
            let seconds: f32 = captures.get(1)?.as_str().parse().ok()?;
            return Some(UiTarsAction::Wait { seconds });
        }
        None
    }

    /// Clamp normalized coordinates to the valid 0-1000 range.
    ///
    /// UI-TARS 7B sometimes returns coordinates >1000 (e.g., 3439 for a Settings gear icon).
    /// This function clamps them to the valid range and reports whether clamping occurred.
    ///
    /// # Returns
    /// * `(clamped_x, clamped_y, was_clamped)` - The clamped coordinates and whether any clamping occurred
    pub fn clamp_normalized(normalized_x: u32, normalized_y: u32) -> (u32, u32, bool) {
        let clamped_x = normalized_x.min(COORDINATE_FACTOR);
        let clamped_y = normalized_y.min(COORDINATE_FACTOR);
        let was_clamped = clamped_x != normalized_x || clamped_y != normalized_y;

        if was_clamped {
            tracing::warn!(
                original_coords = format!("({}, {})", normalized_x, normalized_y),
                clamped_coords = format!("({}, {})", clamped_x, clamped_y),
                "UI-TARS returned out-of-range normalized coordinates (>1000), clamping to valid range"
            );
        }

        (clamped_x, clamped_y, was_clamped)
    }

    /// Convert normalized coordinates (0-1000) to CSS pixels
    ///
    /// UI-TARS outputs coordinates normalized to a 0-1000 range.
    /// This function converts them to actual CSS pixel coordinates
    /// based on the image dimensions.
    ///
    /// Coordinates >1000 are clamped to 1000 before conversion (UI-TARS 7B
    /// sometimes returns out-of-range values like 3439).
    ///
    /// # Formula
    /// ```text
    /// css_x = (normalized_x / 1000) * image_width
    /// css_y = (normalized_y / 1000) * image_height
    /// ```
    ///
    /// # Arguments
    /// * `normalized_x` - X coordinate in 0-1000 range (clamped if >1000)
    /// * `normalized_y` - Y coordinate in 0-1000 range (clamped if >1000)
    /// * `image_width` - Width of the screenshot in CSS pixels
    /// * `image_height` - Height of the screenshot in CSS pixels
    ///
    /// # Returns
    /// * `(u32, u32)` - The (x, y) coordinates in CSS pixels
    ///
    /// # Example
    /// ```ignore
    /// // For a 1920x1080 screen
    /// let (css_x, css_y) = UiTarsParser::to_css_pixels(500, 500, 1920, 1080);
    /// assert_eq!(css_x, 960);  // 500/1000 * 1920 = 960
    /// assert_eq!(css_y, 540);  // 500/1000 * 1080 = 540
    /// ```
    pub fn to_css_pixels(
        normalized_x: u32,
        normalized_y: u32,
        image_width: u32,
        image_height: u32,
    ) -> (u32, u32) {
        let (clamped_x, clamped_y, _) = Self::clamp_normalized(normalized_x, normalized_y);
        let css_x =
            ((clamped_x as f64 / COORDINATE_FACTOR as f64) * image_width as f64).round() as u32;
        let css_y =
            ((clamped_y as f64 / COORDINATE_FACTOR as f64) * image_height as f64).round() as u32;
        (css_x, css_y)
    }

    /// Convert CSS pixel coordinates back to normalized (0-1000) range
    ///
    /// This is the inverse of `to_css_pixels`.
    ///
    /// # Arguments
    /// * `css_x` - X coordinate in CSS pixels
    /// * `css_y` - Y coordinate in CSS pixels
    /// * `image_width` - Width of the screenshot in CSS pixels
    /// * `image_height` - Height of the screenshot in CSS pixels
    ///
    /// # Returns
    /// * `(u32, u32)` - The (x, y) coordinates in 0-1000 range
    pub fn to_normalized(
        css_x: u32,
        css_y: u32,
        image_width: u32,
        image_height: u32,
    ) -> (u32, u32) {
        let normalized_x = if image_width > 0 {
            ((css_x as f64 / image_width as f64) * COORDINATE_FACTOR as f64).round() as u32
        } else {
            0
        };
        let normalized_y = if image_height > 0 {
            ((css_y as f64 / image_height as f64) * COORDINATE_FACTOR as f64).round() as u32
        } else {
            0
        };
        (normalized_x, normalized_y)
    }

    /// Extract the thought/reasoning from UI-TARS response
    ///
    /// # Arguments
    /// * `response` - The raw response from UI-TARS model
    ///
    /// # Returns
    /// * `Some(String)` - The extracted thought if found
    /// * `None` - If no thought is present
    ///
    /// # Example
    /// ```ignore
    /// let response = "Thought: I need to click the submit button\nAction: click(start_box='(197,456)')";
    /// let thought = UiTarsParser::extract_thought(response);
    /// assert_eq!(thought, Some("I need to click the submit button".to_string()));
    /// ```
    pub fn extract_thought(response: &str) -> Option<String> {
        use std::sync::OnceLock;
        static THOUGHT_RE: OnceLock<Regex> = OnceLock::new();
        let thought_re =
            THOUGHT_RE.get_or_init(|| Regex::new(r"(?i)Thought:\s*(.+?)(?:\n|$)").unwrap());

        if let Some(captures) = thought_re.captures(response) {
            if let Some(thought_match) = captures.get(1) {
                let thought = thought_match.as_str().trim();
                if !thought.is_empty() {
                    return Some(thought.to_string());
                }
            }
        }
        None
    }

    /// Parse a complete UI-TARS response returning both thought and action
    ///
    /// # Arguments
    /// * `response` - The raw response from UI-TARS model
    ///
    /// # Returns
    /// * `Ok((Option<String>, UiTarsAction))` - The thought (if present) and parsed action
    /// * `Err(Error)` - If action parsing fails
    pub fn parse_full(response: &str) -> Result<(Option<String>, UiTarsAction)> {
        let thought = Self::extract_thought(response);
        let action = Self::parse(response)?;
        Ok((thought, action))
    }

    /// Convert a parsed action to CSS pixels
    ///
    /// This is a convenience method that takes a parsed action and converts
    /// all coordinates from normalized (0-1000) to CSS pixels.
    ///
    /// # Arguments
    /// * `action` - The parsed action with normalized coordinates
    /// * `image_width` - Width of the screenshot in CSS pixels
    /// * `image_height` - Height of the screenshot in CSS pixels
    ///
    /// # Returns
    /// * The action with coordinates converted to CSS pixels
    pub fn action_to_css_pixels(
        action: UiTarsAction,
        image_width: u32,
        image_height: u32,
    ) -> UiTarsAction {
        match action {
            UiTarsAction::Click { x, y } => {
                let (css_x, css_y) = Self::to_css_pixels(x, y, image_width, image_height);
                UiTarsAction::Click { x: css_x, y: css_y }
            }
            UiTarsAction::DoubleClick { x, y } => {
                let (css_x, css_y) = Self::to_css_pixels(x, y, image_width, image_height);
                UiTarsAction::DoubleClick { x: css_x, y: css_y }
            }
            UiTarsAction::RightClick { x, y } => {
                let (css_x, css_y) = Self::to_css_pixels(x, y, image_width, image_height);
                UiTarsAction::RightClick { x: css_x, y: css_y }
            }
            UiTarsAction::Drag { start, end } => {
                let (start_x, start_y) =
                    Self::to_css_pixels(start.0, start.1, image_width, image_height);
                let (end_x, end_y) = Self::to_css_pixels(end.0, end.1, image_width, image_height);
                UiTarsAction::Drag {
                    start: (start_x, start_y),
                    end: (end_x, end_y),
                }
            }
            UiTarsAction::Scroll { x, y, direction } => {
                let (css_x, css_y) = Self::to_css_pixels(x, y, image_width, image_height);
                UiTarsAction::Scroll {
                    x: css_x,
                    y: css_y,
                    direction,
                }
            }
            // These actions don't have coordinates
            action @ UiTarsAction::Type { .. }
            | action @ UiTarsAction::Hotkey { .. }
            | action @ UiTarsAction::Wait { .. } => action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_click_action() {
        let response = "Thought: I need to click the button\nAction: click(start_box='(197,456)')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(action, UiTarsAction::Click { x: 197, y: 456 });
    }

    #[test]
    fn test_parse_click_action_no_quotes() {
        let response = "Action: click(start_box=(100,200))";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(action, UiTarsAction::Click { x: 100, y: 200 });
    }

    #[test]
    fn test_parse_click_action_double_quotes() {
        let response = "Action: click(start_box=\"(300,400)\")";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(action, UiTarsAction::Click { x: 300, y: 400 });
    }

    #[test]
    fn test_parse_click_action_only() {
        // Test without "Action:" prefix
        let response = "click(start_box='(500,600)')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(action, UiTarsAction::Click { x: 500, y: 600 });
    }

    #[test]
    fn test_parse_double_click_action() {
        let response = "Action: left_double(start_box='(250,350)')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(action, UiTarsAction::DoubleClick { x: 250, y: 350 });
    }

    #[test]
    fn test_parse_right_click_action() {
        let response = "Action: right_single(start_box='(400,500)')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(action, UiTarsAction::RightClick { x: 400, y: 500 });
    }

    #[test]
    fn test_parse_drag_action() {
        let response = "Thought: Dragging the slider\nAction: drag(start_box='(100,200)', end_box='(300,200)')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(
            action,
            UiTarsAction::Drag {
                start: (100, 200),
                end: (300, 200),
            }
        );
    }

    #[test]
    fn test_parse_drag_action_double_quotes() {
        let response = "Action: drag(start_box=\"(50,100)\", end_box=\"(150,100)\")";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(
            action,
            UiTarsAction::Drag {
                start: (50, 100),
                end: (150, 100),
            }
        );
    }

    #[test]
    fn test_parse_type_action() {
        let response = "Action: type(content='Hello World')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(
            action,
            UiTarsAction::Type {
                text: "Hello World".to_string()
            }
        );
    }

    #[test]
    fn test_parse_type_action_double_quotes() {
        let response = "Action: type(content=\"Hello World\")";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(
            action,
            UiTarsAction::Type {
                text: "Hello World".to_string()
            }
        );
    }

    #[test]
    fn test_parse_type_action_with_escapes() {
        let response = "Action: type(content='Line1\\nLine2')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(
            action,
            UiTarsAction::Type {
                text: "Line1\nLine2".to_string()
            }
        );
    }

    #[test]
    fn test_parse_scroll_action() {
        let response = "Action: scroll(start_box='(500,500)', direction='down')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(
            action,
            UiTarsAction::Scroll {
                x: 500,
                y: 500,
                direction: ScrollDirection::Down,
            }
        );
    }

    #[test]
    fn test_parse_scroll_action_up() {
        let response = "Action: scroll(start_box='(300,400)', direction='up')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(
            action,
            UiTarsAction::Scroll {
                x: 300,
                y: 400,
                direction: ScrollDirection::Up,
            }
        );
    }

    #[test]
    fn test_parse_scroll_action_left_right() {
        let response_left = "Action: scroll(start_box='(100,100)', direction='left')";
        let action_left = UiTarsParser::parse(response_left).unwrap();
        assert_eq!(
            action_left,
            UiTarsAction::Scroll {
                x: 100,
                y: 100,
                direction: ScrollDirection::Left,
            }
        );

        let response_right = "Action: scroll(start_box='(200,200)', direction='right')";
        let action_right = UiTarsParser::parse(response_right).unwrap();
        assert_eq!(
            action_right,
            UiTarsAction::Scroll {
                x: 200,
                y: 200,
                direction: ScrollDirection::Right,
            }
        );
    }

    #[test]
    fn test_parse_hotkey_action() {
        let response = "Action: hotkey(key='ctrl+c')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(
            action,
            UiTarsAction::Hotkey {
                key: "ctrl+c".to_string()
            }
        );
    }

    #[test]
    fn test_parse_hotkey_action_complex() {
        let response = "Action: hotkey(key='ctrl+shift+s')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(
            action,
            UiTarsAction::Hotkey {
                key: "ctrl+shift+s".to_string()
            }
        );
    }

    #[test]
    fn test_parse_wait_action() {
        let response = "Action: wait(time='2')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(action, UiTarsAction::Wait { seconds: 2.0 });
    }

    #[test]
    fn test_parse_wait_action_float() {
        let response = "Action: wait(time='1.5')";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(action, UiTarsAction::Wait { seconds: 1.5 });
    }

    #[test]
    fn test_parse_wait_action_no_quotes() {
        let response = "Action: wait(time=3)";
        let action = UiTarsParser::parse(response).unwrap();
        assert_eq!(action, UiTarsAction::Wait { seconds: 3.0 });
    }

    #[test]
    fn test_coordinate_conversion() {
        // Test basic conversion for a 1920x1080 screen
        let (css_x, css_y) = UiTarsParser::to_css_pixels(500, 500, 1920, 1080);
        assert_eq!(css_x, 960); // 500/1000 * 1920 = 960
        assert_eq!(css_y, 540); // 500/1000 * 1080 = 540
    }

    #[test]
    fn test_coordinate_conversion_corners() {
        // Test corner cases
        let (x0, y0) = UiTarsParser::to_css_pixels(0, 0, 1920, 1080);
        assert_eq!(x0, 0);
        assert_eq!(y0, 0);

        let (x1000, y1000) = UiTarsParser::to_css_pixels(1000, 1000, 1920, 1080);
        assert_eq!(x1000, 1920);
        assert_eq!(y1000, 1080);
    }

    #[test]
    fn test_coordinate_conversion_quarter() {
        // Test quarter points
        let (x, y) = UiTarsParser::to_css_pixels(250, 250, 1920, 1080);
        assert_eq!(x, 480); // 250/1000 * 1920 = 480
        assert_eq!(y, 270); // 250/1000 * 1080 = 270
    }

    #[test]
    fn test_coordinate_conversion_roundtrip() {
        // Test that conversion is reversible (within rounding)
        let original_x = 333;
        let original_y = 666;
        let (css_x, css_y) = UiTarsParser::to_css_pixels(original_x, original_y, 1920, 1080);
        let (back_x, back_y) = UiTarsParser::to_normalized(css_x, css_y, 1920, 1080);
        // Allow for rounding differences
        assert!((back_x as i32 - original_x as i32).abs() <= 1);
        assert!((back_y as i32 - original_y as i32).abs() <= 1);
    }

    #[test]
    fn test_extract_thought() {
        let response =
            "Thought: I need to click the submit button\nAction: click(start_box='(197,456)')";
        let thought = UiTarsParser::extract_thought(response);
        assert_eq!(
            thought,
            Some("I need to click the submit button".to_string())
        );
    }

    #[test]
    fn test_extract_thought_multiline() {
        let response =
            "Thought: Looking at the page, I see a login form\nAction: type(content='user@example.com')";
        let thought = UiTarsParser::extract_thought(response);
        assert_eq!(
            thought,
            Some("Looking at the page, I see a login form".to_string())
        );
    }

    #[test]
    fn test_extract_thought_none() {
        let response = "Action: click(start_box='(100,200)')";
        let thought = UiTarsParser::extract_thought(response);
        assert!(thought.is_none());
    }

    #[test]
    fn test_extract_thought_case_insensitive() {
        let response = "THOUGHT: Testing case insensitivity\nAction: wait(time='1')";
        let thought = UiTarsParser::extract_thought(response);
        assert_eq!(thought, Some("Testing case insensitivity".to_string()));
    }

    #[test]
    fn test_parse_full() {
        let response =
            "Thought: I need to search for something\nAction: type(content='search query')";
        let (thought, action) = UiTarsParser::parse_full(response).unwrap();
        assert_eq!(thought, Some("I need to search for something".to_string()));
        assert_eq!(
            action,
            UiTarsAction::Type {
                text: "search query".to_string()
            }
        );
    }

    #[test]
    fn test_parse_unknown_action() {
        let response = "Action: unknown_action(param='value')";
        let result = UiTarsParser::parse(response);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_response() {
        let response = "";
        let result = UiTarsParser::parse(response);
        assert!(result.is_err());
    }

    #[test]
    fn test_action_to_css_pixels() {
        let action = UiTarsAction::Click { x: 500, y: 500 };
        let converted = UiTarsParser::action_to_css_pixels(action, 1920, 1080);
        assert_eq!(converted, UiTarsAction::Click { x: 960, y: 540 });
    }

    #[test]
    fn test_action_to_css_pixels_drag() {
        let action = UiTarsAction::Drag {
            start: (100, 200),
            end: (300, 400),
        };
        let converted = UiTarsParser::action_to_css_pixels(action, 1920, 1080);
        assert_eq!(
            converted,
            UiTarsAction::Drag {
                start: (192, 216), // 100/1000*1920=192, 200/1000*1080=216
                end: (576, 432),   // 300/1000*1920=576, 400/1000*1080=432
            }
        );
    }

    #[test]
    fn test_action_to_css_pixels_type_unchanged() {
        let action = UiTarsAction::Type {
            text: "hello".to_string(),
        };
        let converted = UiTarsParser::action_to_css_pixels(action.clone(), 1920, 1080);
        assert_eq!(converted, action);
    }

    #[test]
    fn test_scroll_direction_display() {
        assert_eq!(ScrollDirection::Up.to_string(), "up");
        assert_eq!(ScrollDirection::Down.to_string(), "down");
        assert_eq!(ScrollDirection::Left.to_string(), "left");
        assert_eq!(ScrollDirection::Right.to_string(), "right");
    }

    #[test]
    fn test_scroll_direction_from_str() {
        assert_eq!(ScrollDirection::from_str("up"), Some(ScrollDirection::Up));
        assert_eq!(
            ScrollDirection::from_str("DOWN"),
            Some(ScrollDirection::Down)
        );
        assert_eq!(
            ScrollDirection::from_str("Left"),
            Some(ScrollDirection::Left)
        );
        assert_eq!(ScrollDirection::from_str("invalid"), None);
    }

    #[test]
    fn test_clamp_normalized_within_range() {
        let (x, y, clamped) = UiTarsParser::clamp_normalized(500, 500);
        assert_eq!(x, 500);
        assert_eq!(y, 500);
        assert!(!clamped);
    }

    #[test]
    fn test_clamp_normalized_out_of_range() {
        let (x, y, clamped) = UiTarsParser::clamp_normalized(3439, 62);
        assert_eq!(x, 1000);
        assert_eq!(y, 62);
        assert!(clamped);
    }

    #[test]
    fn test_clamp_normalized_both_out_of_range() {
        let (x, y, clamped) = UiTarsParser::clamp_normalized(1500, 2039);
        assert_eq!(x, 1000);
        assert_eq!(y, 1000);
        assert!(clamped);
    }

    #[test]
    fn test_to_css_pixels_clamps_out_of_range() {
        // UI-TARS 7B returned 3439 for Settings gear - should clamp to 1000
        let (css_x, css_y) = UiTarsParser::to_css_pixels(3439, 62, 1920, 1080);
        assert_eq!(css_x, 1920); // clamped to 1000/1000 * 1920
        assert_eq!(css_y, 67); // 62/1000 * 1080 = 66.96 ≈ 67
    }

    #[test]
    fn test_action_serialization() {
        let action = UiTarsAction::Click { x: 100, y: 200 };
        let json = serde_json::to_string(&action).unwrap();
        let deserialized: UiTarsAction = serde_json::from_str(&json).unwrap();
        assert_eq!(action, deserialized);
    }

    #[test]
    fn test_scroll_action_serialization() {
        let action = UiTarsAction::Scroll {
            x: 500,
            y: 500,
            direction: ScrollDirection::Down,
        };
        let json = serde_json::to_string(&action).unwrap();
        let deserialized: UiTarsAction = serde_json::from_str(&json).unwrap();
        assert_eq!(action, deserialized);
    }
}
