//! Molmo 2 vision model output parser.
//!
//! Parses `<points coords="FRAME_ID [PID X Y]+">description</points>` XML tags
//! from Molmo 2 model output. Coordinates use 0-1000 normalized space, same as
//! UI-TARS but with a different output format.
//!
//! # Coordinate System
//!
//! Molmo outputs point coordinates normalized to 0-1000 range where:
//! - (0, 0) = top-left corner
//! - (1000, 1000) = bottom-right corner
//!
//! Conversion to display pixels: `display_px = (norm / 1000) * display_size`

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Normalized coordinate scale (0-1000).
pub const COORDINATE_SCALE: u32 = 1000;

/// A single parsed point from Molmo output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MolmoParsedPoint {
    /// Point ID within the response
    pub point_id: u32,
    /// X coordinate in 0-1000 normalized space
    pub x: u32,
    /// Y coordinate in 0-1000 normalized space
    pub y: u32,
    /// Frame ID (typically 0 for single-image input)
    pub frame_id: u32,
}

/// Result of parsing a Molmo response.
#[derive(Debug, Clone)]
pub struct MolmoParseResult {
    /// All parsed points
    pub points: Vec<MolmoParsedPoint>,
    /// Description text from within the points tag (if any)
    pub description: Option<String>,
    /// Model reasoning text before the points tag (if any)
    pub reasoning: Option<String>,
}

/// Error type for Molmo parsing failures.
#[derive(Debug, thiserror::Error)]
pub enum MolmoParseError {
    #[error("no points found in response")]
    NoPointsFound,
    #[error("expected single point but found {0}")]
    MultiplePoints(usize),
    #[error("invalid coordinate: {0}")]
    InvalidCoordinate(String),
}

/// Parser for Molmo 2 vision model output.
///
/// Molmo outputs detected points in XML-like tags:
/// ```text
/// <points coords="0 1 081 202">the submit button</points>
/// ```
///
/// Where coords format is: `FRAME_ID [POINT_ID X Y]+`
pub struct MolmoParser;

impl MolmoParser {
    /// Parse full Molmo response, extracting all points and metadata.
    ///
    /// # Example
    /// ```ignore
    /// let response = r#"I can see a button. <points coords="0 1 500 300">submit button</points>"#;
    /// let result = MolmoParser::parse(response);
    /// assert_eq!(result.points.len(), 1);
    /// assert_eq!(result.points[0].x, 500);
    /// ```
    pub fn parse(response: &str) -> MolmoParseResult {
        // Extract reasoning (text before first <points> or <tracks> tag)
        let reasoning = Self::extract_reasoning(response);

        // Regex for <points> or <tracks> tags with coords attribute
        let coords_re = Regex::new(
            r#"<(?:points|tracks)[^>]*?\scoords="([0-9\t:;, .]+)"[^>]*/?\s*>(?:([^<]*)</(?:points|tracks)>)?"#,
        )
        .unwrap();

        // Regex for individual PID X Y triplets within coords
        let points_re = Regex::new(r"(\d+)\s+(\d{1,4})\s+(\d{1,4})").unwrap();

        let mut all_points = Vec::new();
        let mut description = None;

        for caps in coords_re.captures_iter(response) {
            let coords_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");

            // Extract description text if present
            if let Some(desc_match) = caps.get(2) {
                let desc = desc_match.as_str().trim();
                if !desc.is_empty() {
                    description = Some(desc.to_string());
                }
            }

            // First token is frame_id
            let tokens: Vec<&str> = coords_str.split_whitespace().collect();
            let frame_id = tokens
                .first()
                .and_then(|t| t.parse::<u32>().ok())
                .unwrap_or(0);

            // Skip frame_id, parse remaining as PID X Y triplets
            let remaining = if !tokens.is_empty() {
                coords_str
                    .trim_start()
                    .trim_start_matches(|c: char| c.is_ascii_digit())
                    .trim_start()
            } else {
                coords_str
            };

            for point_caps in points_re.captures_iter(remaining) {
                if let (Some(pid), Some(x), Some(y)) = (
                    point_caps
                        .get(1)
                        .and_then(|m| m.as_str().parse::<u32>().ok()),
                    point_caps
                        .get(2)
                        .and_then(|m| m.as_str().parse::<u32>().ok()),
                    point_caps
                        .get(3)
                        .and_then(|m| m.as_str().parse::<u32>().ok()),
                ) {
                    all_points.push(MolmoParsedPoint {
                        point_id: pid,
                        x,
                        y,
                        frame_id,
                    });
                }
            }
        }

        MolmoParseResult {
            points: all_points,
            description,
            reasoning,
        }
    }

    /// Parse response expecting exactly one point.
    ///
    /// Returns error if zero or more than one point found.
    pub fn parse_single_point(response: &str) -> Result<MolmoParsedPoint, MolmoParseError> {
        let result = Self::parse(response);
        match result.points.len() {
            0 => Err(MolmoParseError::NoPointsFound),
            1 => Ok(result.points.into_iter().next().unwrap()),
            n => Err(MolmoParseError::MultiplePoints(n)),
        }
    }

    /// Convert normalized 0-1000 coordinates to display pixels.
    ///
    /// # Formula
    /// ```text
    /// display_x = (normalized_x / 1000) * display_width
    /// display_y = (normalized_y / 1000) * display_height
    /// ```
    ///
    /// Result is clamped to `[0, display_size - 1]`.
    pub fn to_display_pixels(
        x: u32,
        y: u32,
        display_width: u32,
        display_height: u32,
    ) -> (u32, u32) {
        let (clamped_x, clamped_y, _) = Self::clamp(x, y);
        let px =
            ((clamped_x as f64 / COORDINATE_SCALE as f64) * display_width as f64).round() as u32;
        let py =
            ((clamped_y as f64 / COORDINATE_SCALE as f64) * display_height as f64).round() as u32;
        // Clamp to display bounds
        let px = px.min(display_width.saturating_sub(1));
        let py = py.min(display_height.saturating_sub(1));
        (px, py)
    }

    /// Clamp coordinates to 0-1000 range.
    ///
    /// Returns `(clamped_x, clamped_y, was_clamped)`.
    pub fn clamp(x: u32, y: u32) -> (u32, u32, bool) {
        let cx = x.min(COORDINATE_SCALE);
        let cy = y.min(COORDINATE_SCALE);
        let was_clamped = cx != x || cy != y;
        if was_clamped {
            tracing::warn!(
                original = format!("({}, {})", x, y),
                clamped = format!("({}, {})", cx, cy),
                "Molmo returned out-of-range coordinates, clamping"
            );
        }
        (cx, cy, was_clamped)
    }

    /// Extract reasoning text before the first `<points>` or `<tracks>` tag.
    fn extract_reasoning(response: &str) -> Option<String> {
        let tag_re = Regex::new(r"<(?:points|tracks)").unwrap();
        if let Some(m) = tag_re.find(response) {
            let before = response[..m.start()].trim();
            if !before.is_empty() {
                return Some(before.to_string());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_point() {
        let response = r#"<points coords="0 1 081 202">the button</points>"#;
        let result = MolmoParser::parse(response);
        assert_eq!(result.points.len(), 1);
        assert_eq!(result.points[0].point_id, 1);
        assert_eq!(result.points[0].x, 81);
        assert_eq!(result.points[0].y, 202);
        assert_eq!(result.points[0].frame_id, 0);
    }

    #[test]
    fn test_parse_single_point_strict() {
        let response = r#"<points coords="0 1 500 300">submit</points>"#;
        let point = MolmoParser::parse_single_point(response).unwrap();
        assert_eq!(point.x, 500);
        assert_eq!(point.y, 300);
    }

    #[test]
    fn test_parse_description() {
        let response = r#"<points coords="0 1 500 300">the submit button</points>"#;
        let result = MolmoParser::parse(response);
        assert_eq!(result.description, Some("the submit button".to_string()));
    }

    #[test]
    fn test_parse_multi_point() {
        let response =
            r#"<points coords="0 1 100 200 2 300 400 3 500 600">multiple items</points>"#;
        let result = MolmoParser::parse(response);
        assert_eq!(result.points.len(), 3);
        assert_eq!(result.points[0].x, 100);
        assert_eq!(result.points[0].y, 200);
        assert_eq!(result.points[1].x, 300);
        assert_eq!(result.points[1].y, 400);
        assert_eq!(result.points[2].x, 500);
        assert_eq!(result.points[2].y, 600);
    }

    #[test]
    fn test_parse_no_points() {
        let response = "I cannot find any matching elements on the screen.";
        let result = MolmoParser::parse(response);
        assert!(result.points.is_empty());
        assert!(result.description.is_none());
    }

    #[test]
    fn test_parse_single_point_error_none() {
        let response = "No elements found.";
        let err = MolmoParser::parse_single_point(response).unwrap_err();
        assert!(matches!(err, MolmoParseError::NoPointsFound));
    }

    #[test]
    fn test_parse_single_point_error_multiple() {
        let response = r#"<points coords="0 1 100 200 2 300 400">items</points>"#;
        let err = MolmoParser::parse_single_point(response).unwrap_err();
        assert!(matches!(err, MolmoParseError::MultiplePoints(2)));
    }

    #[test]
    fn test_parse_self_closing() {
        let response = r#"<points coords="0 1 250 750" />"#;
        let result = MolmoParser::parse(response);
        assert_eq!(result.points.len(), 1);
        assert_eq!(result.points[0].x, 250);
        assert_eq!(result.points[0].y, 750);
        assert!(result.description.is_none());
    }

    #[test]
    fn test_to_display_pixels_center() {
        let (px, py) = MolmoParser::to_display_pixels(500, 500, 1920, 1080);
        assert_eq!(px, 960);
        assert_eq!(py, 540);
    }

    #[test]
    fn test_to_display_pixels_origin() {
        let (px, py) = MolmoParser::to_display_pixels(0, 0, 1920, 1080);
        assert_eq!(px, 0);
        assert_eq!(py, 0);
    }

    #[test]
    fn test_to_display_pixels_max() {
        let (px, py) = MolmoParser::to_display_pixels(1000, 1000, 1920, 1080);
        // Should clamp to display_size - 1
        assert_eq!(px, 1919);
        assert_eq!(py, 1079);
    }

    #[test]
    fn test_to_display_pixels_retina() {
        // Retina (2.0) and non-retina should give same result
        // because conversion uses display_width/height, not physical
        let (px1, py1) = MolmoParser::to_display_pixels(500, 500, 1920, 1080);
        let (px2, py2) = MolmoParser::to_display_pixels(500, 500, 1920, 1080);
        assert_eq!(px1, px2);
        assert_eq!(py1, py2);
    }

    #[test]
    fn test_clamp_in_range() {
        let (x, y, was_clamped) = MolmoParser::clamp(500, 500);
        assert_eq!(x, 500);
        assert_eq!(y, 500);
        assert!(!was_clamped);
    }

    #[test]
    fn test_clamp_out_of_range() {
        let (x, y, was_clamped) = MolmoParser::clamp(1500, 2000);
        assert_eq!(x, 1000);
        assert_eq!(y, 1000);
        assert!(was_clamped);
    }

    #[test]
    fn test_parse_with_reasoning() {
        let response = r#"I can see a submit button in the form. <points coords="0 1 450 680">submit button</points>"#;
        let result = MolmoParser::parse(response);
        assert_eq!(result.points.len(), 1);
        assert_eq!(
            result.reasoning,
            Some("I can see a submit button in the form.".to_string())
        );
        assert_eq!(result.description, Some("submit button".to_string()));
    }

    #[test]
    fn test_parse_leading_zeros() {
        let response = r#"<points coords="0 1 081 022">item</points>"#;
        let result = MolmoParser::parse(response);
        assert_eq!(result.points.len(), 1);
        assert_eq!(result.points[0].x, 81);
        assert_eq!(result.points[0].y, 22);
    }
}
