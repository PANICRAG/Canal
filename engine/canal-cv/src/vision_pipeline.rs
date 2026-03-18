//! Code-First Vision Pipeline — deterministic matching before VLM fallback.
//!
//! # 5-Step Cascade
//!
//! ```text
//! Step 0: A11y match (optional)        → 0 tokens, ~98% accurate
//! Step 1: Exact text match on boxes    → 0 tokens, ~99% accurate
//! Step 2: Fuzzy text match on boxes    → 0 tokens, ~90% accurate
//! Step 3: Element heuristic match      → 0 tokens, ~85% accurate
//! Step 4: VLM + box validation         → ~1K tokens, ~92% accurate
//! Step 5: VLM raw (last resort)        → ~1K tokens, ~78% accurate
//! ```
//!
//! Code-first steps (0-3) cost zero LLM tokens and execute in <1ms.
//! VLM steps (4-5) only run when code steps all fail.

use std::sync::Arc;

use async_trait::async_trait;

use crate::box_detector::{BoundingBox, BoxDetector};
use crate::types::InteractiveElement;
use crate::vision_detector::{DetectionInput, DetectionResult, VisionDetector};

// ─── Stop words for fuzzy matching ─────────────────────────────────────────

/// Action verbs filtered out during fuzzy matching.
const ACTION_VERBS: &[&str] = &[
    "click", "tap", "press", "type", "enter", "select", "scroll", "drag", "open", "close", "find",
    "go",
];

/// Articles filtered during fuzzy matching.
const ARTICLES: &[&str] = &["the", "a", "an", "this", "that"];

/// Modifiers and position words.
const MODIFIERS: &[&str] = &[
    "blue", "red", "green", "large", "small", "big", "first", "last", "new", "old", "left",
    "right", "top", "bottom", "above", "below",
];

// ─── Configuration ─────────────────────────────────────────────────────────

/// Configuration for the vision pipeline.
#[derive(Debug, Clone)]
pub struct VisionPipelineConfig {
    /// Max pixel distance for snapping a VLM point to a nearby box center.
    pub snap_distance_px: u32,
    /// Enable code-first steps before VLM.
    pub enable_code_first: bool,
}

impl Default for VisionPipelineConfig {
    fn default() -> Self {
        Self {
            snap_distance_px: 50,
            enable_code_first: true,
        }
    }
}

// ─── VisionPipeline ────────────────────────────────────────────────────────

/// Code-first vision pipeline with VLM fallback.
///
/// Orchestrates the 5-step cascade:
/// 1. Code-based matching against OmniParser bounding boxes (0 tokens)
/// 2. VLM pointing + box validation (~1K tokens, only if code steps fail)
pub struct VisionPipeline {
    box_detector: Arc<dyn BoxDetector>,
    pointing_model: Arc<dyn VisionDetector>,
    config: VisionPipelineConfig,
}

impl VisionPipeline {
    /// Create a new pipeline.
    pub fn new(
        box_detector: Arc<dyn BoxDetector>,
        pointing_model: Arc<dyn VisionDetector>,
        config: VisionPipelineConfig,
    ) -> Self {
        Self {
            box_detector,
            pointing_model,
            config,
        }
    }

    /// Main detection entry point.
    ///
    /// Runs the full cascade: a11y → code-first → VLM + box validation → raw VLM.
    ///
    /// # Arguments
    /// * `input` - Screenshot and coordinate info
    /// * `target` - What to find (e.g., "Submit", "the search box")
    /// * `a11y_elements` - Optional accessibility tree elements (desktop/browser)
    pub async fn detect(
        &self,
        input: &DetectionInput,
        target: &str,
        a11y_elements: Option<&[InteractiveElement]>,
    ) -> anyhow::Result<DetectionResult> {
        // Step 0: A11y match (if elements provided)
        if let Some(elements) = a11y_elements {
            if let Some(result) = match_a11y_elements(elements, target) {
                tracing::debug!(target = %target, provider = %result.provider, "Step 0: a11y match");
                return Ok(result);
            }
        }

        // Get bounding boxes from OmniParser
        let boxes = match self.box_detector.detect_boxes(input).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "OmniParser unavailable, falling through to VLM");
                vec![] // Degrade gracefully — skip code-first steps
            }
        };

        if self.config.enable_code_first && !boxes.is_empty() {
            // Step 1: Exact text match
            if let Some(bbox) = exact_text_match(&boxes, target) {
                tracing::debug!(target = %target, label = %bbox.label, "Step 1: exact text match");
                return Ok(DetectionResult {
                    x: bbox.center_x(),
                    y: bbox.center_y(),
                    confidence: 0.99,
                    label: Some(bbox.label.clone()),
                    provider: "omniparser(exact)".into(),
                });
            }

            // Step 2: Fuzzy text match
            if let Some(bbox) = fuzzy_text_match(&boxes, target) {
                tracing::debug!(target = %target, label = %bbox.label, "Step 2: fuzzy text match");
                return Ok(DetectionResult {
                    x: bbox.center_x(),
                    y: bbox.center_y(),
                    confidence: 0.90,
                    label: Some(bbox.label.clone()),
                    provider: "omniparser(fuzzy)".into(),
                });
            }

            // Step 3: Element heuristic match
            if let Some(bbox) = element_heuristic_match(&boxes, target) {
                tracing::debug!(target = %target, label = %bbox.label, "Step 3: heuristic match");
                return Ok(DetectionResult {
                    x: bbox.center_x(),
                    y: bbox.center_y(),
                    confidence: 0.85,
                    label: Some(bbox.label.clone()),
                    provider: "omniparser(heuristic)".into(),
                });
            }
        }

        // Step 4+5: VLM pointing
        let vlm_results = self.pointing_model.detect(input, target).await?;
        let vlm_name = self.pointing_model.name();

        let vlm_point = vlm_results
            .first()
            .cloned()
            .unwrap_or_else(|| DetectionResult::not_found(target));

        if vlm_point.confidence <= 0.0 {
            return Ok(vlm_point);
        }

        // Step 4: VLM + box validation
        if !boxes.is_empty() {
            // Check if VLM point is inside a box
            if let Some(bbox) = find_containing_box(&boxes, vlm_point.x, vlm_point.y) {
                return Ok(DetectionResult {
                    x: bbox.center_x(),
                    y: bbox.center_y(),
                    confidence: 0.92,
                    label: Some(bbox.label.clone()),
                    provider: format!("box+{vlm_name}"),
                });
            }

            // Check if VLM point is near a box (snap)
            if let Some(bbox) = find_nearest_box(
                &boxes,
                vlm_point.x,
                vlm_point.y,
                self.config.snap_distance_px,
            ) {
                return Ok(DetectionResult {
                    x: bbox.center_x(),
                    y: bbox.center_y(),
                    confidence: 0.70,
                    label: Some(bbox.label.clone()),
                    provider: format!("box+{vlm_name}(snapped)"),
                });
            }
        }

        // Step 5: Raw VLM point (no box validation available)
        Ok(DetectionResult {
            x: vlm_point.x,
            y: vlm_point.y,
            confidence: 0.50,
            label: vlm_point.label,
            provider: format!("{vlm_name}(raw)"),
        })
    }
}

// ─── FallbackDetector ──────────────────────────────────────────────────────

/// Composite detector that tries primary, falls back to secondary on error.
pub struct FallbackDetector {
    primary: Arc<dyn VisionDetector>,
    fallback: Arc<dyn VisionDetector>,
}

impl FallbackDetector {
    /// Create a fallback detector.
    pub fn new(primary: Arc<dyn VisionDetector>, fallback: Arc<dyn VisionDetector>) -> Self {
        Self { primary, fallback }
    }
}

#[async_trait]
impl VisionDetector for FallbackDetector {
    fn name(&self) -> &str {
        "fallback"
    }

    fn supports_multi_point(&self) -> bool {
        self.primary.supports_multi_point()
    }

    async fn detect(
        &self,
        input: &DetectionInput,
        task: &str,
    ) -> anyhow::Result<Vec<DetectionResult>> {
        match self.primary.detect(input, task).await {
            Ok(results) => Ok(results),
            Err(e) => {
                tracing::warn!(
                    primary = self.primary.name(),
                    fallback = self.fallback.name(),
                    error = %e,
                    "Primary detector failed, using fallback"
                );
                self.fallback.detect(input, task).await
            }
        }
    }
}

// ─── Code-first matching helpers ───────────────────────────────────────────

/// Step 0: Match against accessibility tree elements.
pub fn match_a11y_elements(
    elements: &[InteractiveElement],
    target: &str,
) -> Option<DetectionResult> {
    let target_lower = target.to_lowercase();

    // Exact label match first
    for el in elements {
        if el.label.to_lowercase() == target_lower {
            return Some(DetectionResult {
                x: el.bounds.center_x() as u32,
                y: el.bounds.center_y() as u32,
                confidence: 0.98,
                label: Some(el.label.clone()),
                provider: "a11y(exact)".into(),
            });
        }
    }

    // Substring match
    for el in elements {
        if el.label.to_lowercase().contains(&target_lower)
            || target_lower.contains(&el.label.to_lowercase())
        {
            return Some(DetectionResult {
                x: el.bounds.center_x() as u32,
                y: el.bounds.center_y() as u32,
                confidence: 0.92,
                label: Some(el.label.clone()),
                provider: "a11y(contains)".into(),
            });
        }
    }

    None
}

/// Step 1: Case-insensitive exact label match.
pub fn exact_text_match<'a>(boxes: &'a [BoundingBox], target: &str) -> Option<&'a BoundingBox> {
    let target_lower = target.to_lowercase();
    boxes
        .iter()
        .find(|b| b.label.to_lowercase() == target_lower)
}

/// Step 2: Fuzzy text match using word overlap scoring.
///
/// Filters stop words (action verbs, articles, modifiers), then scores boxes
/// by what fraction of content words appear in the box label.
/// Threshold: >= 50% word overlap.
pub fn fuzzy_text_match<'a>(
    boxes: &'a [BoundingBox],
    instruction: &str,
) -> Option<&'a BoundingBox> {
    let words: Vec<String> = instruction
        .split_whitespace()
        .map(|w| {
            w.to_lowercase()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string()
        })
        .filter(|w| !w.is_empty() && !is_stop_word(w))
        .collect();

    if words.is_empty() {
        return None;
    }

    let mut best: Option<(&BoundingBox, f32)> = None;

    for bbox in boxes {
        let label_lower = bbox.label.to_lowercase();
        let matched = words
            .iter()
            .filter(|w| label_lower.contains(w.as_str()))
            .count();
        let score = matched as f32 / words.len() as f32;

        if score >= 0.5 {
            if best.is_none() || score > best.unwrap().1 {
                best = Some((bbox, score));
            }
        }
    }

    best.map(|(b, _)| b)
}

/// Step 3: Element type + position heuristic matching.
///
/// Infers element type from instruction keywords, filters boxes by type,
/// applies position preference (top/bottom).
pub fn element_heuristic_match<'a>(
    boxes: &'a [BoundingBox],
    instruction: &str,
) -> Option<&'a BoundingBox> {
    let lower = instruction.to_lowercase();

    // Infer target element type from keywords
    let target_type = infer_element_type(&lower)?;

    // Filter boxes by element type
    let mut candidates: Vec<&BoundingBox> = boxes
        .iter()
        .filter(|b| {
            b.element_type
                .as_ref()
                .map(|t| t.to_lowercase().contains(&target_type))
                .unwrap_or(false)
        })
        .collect();

    if candidates.is_empty() {
        return None;
    }

    // Apply position preference
    if lower.contains("bottom") || lower.contains("below") || lower.contains("last") {
        candidates.sort_by(|a, b| b.y.cmp(&a.y)); // descending Y
    } else if lower.contains("top") || lower.contains("above") || lower.contains("first") {
        candidates.sort_by(|a, b| a.y.cmp(&b.y)); // ascending Y
    }

    candidates.first().copied()
}

/// Find the first box that contains the given point.
pub fn find_containing_box<'a>(
    boxes: &'a [BoundingBox],
    x: u32,
    y: u32,
) -> Option<&'a BoundingBox> {
    boxes.iter().find(|b| b.contains(x, y))
}

/// Find the nearest box within max_distance from the given point.
pub fn find_nearest_box<'a>(
    boxes: &'a [BoundingBox],
    x: u32,
    y: u32,
    max_distance: u32,
) -> Option<&'a BoundingBox> {
    boxes
        .iter()
        .filter(|b| b.distance_to(x, y) <= max_distance as f64)
        .min_by(|a, b| {
            a.distance_to(x, y)
                .partial_cmp(&b.distance_to(x, y))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

/// Check if a word is a stop word (action verb, article, or modifier).
fn is_stop_word(word: &str) -> bool {
    ACTION_VERBS.contains(&word) || ARTICLES.contains(&word) || MODIFIERS.contains(&word)
}

/// Infer target element type from instruction keywords.
fn infer_element_type(instruction: &str) -> Option<String> {
    let mappings = [
        (&["button", "btn"][..], "button"),
        (&["input", "field", "textbox", "text box"], "input"),
        (&["link", "href", "url"], "link"),
        (&["checkbox", "check box"], "checkbox"),
        (&["dropdown", "combobox", "combo box", "select"], "combobox"),
        (&["tab"], "tab"),
        (&["menu", "menuitem", "menu item"], "menuitem"),
        (&["switch", "toggle"], "switch"),
        (&["radio", "radio button"], "radio"),
        (&["slider", "range"], "slider"),
    ];

    for (keywords, element_type) in &mappings {
        for kw in *keywords {
            if instruction.contains(kw) {
                return Some(element_type.to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ElementBounds;

    fn make_box(label: &str, x: u32, y: u32, w: u32, h: u32, etype: Option<&str>) -> BoundingBox {
        BoundingBox {
            label: label.into(),
            x,
            y,
            width: w,
            height: h,
            confidence: 0.9,
            element_type: etype.map(String::from),
        }
    }

    fn make_element(label: &str, x: f32, y: f32, w: f32, h: f32) -> InteractiveElement {
        InteractiveElement {
            id: "ax_0".into(),
            element_type: "button".into(),
            label: label.into(),
            bounds: ElementBounds {
                x,
                y,
                width: w,
                height: h,
            },
        }
    }

    // ─── Step 0: A11y ─────────────────────────────

    #[test]
    fn test_a11y_exact_match() {
        let elements = vec![make_element("Submit", 100.0, 200.0, 80.0, 30.0)];
        let result = match_a11y_elements(&elements, "Submit").unwrap();
        assert_eq!(result.x, 140);
        assert_eq!(result.y, 215);
        assert_eq!(result.provider, "a11y(exact)");
    }

    #[test]
    fn test_a11y_no_match() {
        let elements = vec![make_element("Cancel", 100.0, 200.0, 80.0, 30.0)];
        assert!(match_a11y_elements(&elements, "Submit").is_none());
    }

    #[test]
    fn test_a11y_contains_match() {
        let elements = vec![make_element("Submit Form", 100.0, 200.0, 80.0, 30.0)];
        let result = match_a11y_elements(&elements, "Submit").unwrap();
        assert_eq!(result.provider, "a11y(contains)");
    }

    // ─── Step 1: Exact text ───────────────────────

    #[test]
    fn test_exact_text_match() {
        let boxes = vec![
            make_box("Cancel", 10, 10, 60, 30, None),
            make_box("Submit", 100, 10, 60, 30, None),
        ];
        let found = exact_text_match(&boxes, "Submit").unwrap();
        assert_eq!(found.label, "Submit");
    }

    #[test]
    fn test_exact_text_match_case_insensitive() {
        let boxes = vec![make_box("SUBMIT", 100, 10, 60, 30, None)];
        let found = exact_text_match(&boxes, "submit").unwrap();
        assert_eq!(found.label, "SUBMIT");
    }

    #[test]
    fn test_exact_text_no_match() {
        let boxes = vec![make_box("Cancel", 10, 10, 60, 30, None)];
        assert!(exact_text_match(&boxes, "Submit").is_none());
    }

    // ─── Step 2: Fuzzy text ──────────────────────

    #[test]
    fn test_fuzzy_text_match() {
        let boxes = vec![make_box("Submit Button", 100, 10, 80, 30, None)];
        // "Click the submit button" → content words: ["submit", "button"]
        // "Submit Button" contains both → 100% overlap
        let found = fuzzy_text_match(&boxes, "Click the submit button").unwrap();
        assert_eq!(found.label, "Submit Button");
    }

    #[test]
    fn test_fuzzy_text_no_match() {
        let boxes = vec![make_box("Cancel", 10, 10, 60, 30, None)];
        assert!(fuzzy_text_match(&boxes, "Click the submit button").is_none());
    }

    #[test]
    fn test_fuzzy_text_partial_match() {
        let boxes = vec![make_box("Submit Form Data", 100, 10, 80, 30, None)];
        // "submit form" → words: ["submit", "form"]
        // "Submit Form Data" contains both → 100%
        let found = fuzzy_text_match(&boxes, "submit form").unwrap();
        assert_eq!(found.label, "Submit Form Data");
    }

    // ─── Step 3: Heuristic ───────────────────────

    #[test]
    fn test_element_heuristic_button() {
        let boxes = vec![
            make_box("OK", 100, 10, 60, 30, Some("button")),
            make_box("Name", 100, 50, 200, 30, Some("input")),
        ];
        let found = element_heuristic_match(&boxes, "click the button").unwrap();
        assert_eq!(found.label, "OK");
    }

    #[test]
    fn test_element_heuristic_position_bottom() {
        let boxes = vec![
            make_box("OK", 100, 10, 60, 30, Some("button")),
            make_box("Apply", 100, 500, 60, 30, Some("button")),
        ];
        let found = element_heuristic_match(&boxes, "click the bottom button").unwrap();
        assert_eq!(found.label, "Apply");
    }

    #[test]
    fn test_element_heuristic_checkbox() {
        let boxes = vec![
            make_box("Remember", 10, 10, 20, 20, Some("checkbox")),
            make_box("Submit", 100, 10, 60, 30, Some("button")),
        ];
        let found = element_heuristic_match(&boxes, "check the checkbox").unwrap();
        assert_eq!(found.label, "Remember");
    }

    #[test]
    fn test_element_heuristic_no_type_match() {
        let boxes = vec![make_box("Submit", 100, 10, 60, 30, Some("button"))];
        // "slider" type not present
        assert!(element_heuristic_match(&boxes, "move the slider").is_none());
    }

    // ─── Box geometry ────────────────────────────

    #[test]
    fn test_find_containing_box() {
        let boxes = vec![
            make_box("A", 100, 100, 50, 50, None),
            make_box("B", 200, 200, 50, 50, None),
        ];
        let found = find_containing_box(&boxes, 120, 120).unwrap();
        assert_eq!(found.label, "A");
    }

    #[test]
    fn test_find_containing_box_miss() {
        let boxes = vec![make_box("A", 100, 100, 50, 50, None)];
        assert!(find_containing_box(&boxes, 50, 50).is_none());
    }

    #[test]
    fn test_find_nearest_box() {
        let boxes = vec![
            make_box("A", 100, 100, 50, 50, None), // center = (125, 125)
            make_box("B", 200, 200, 50, 50, None), // center = (225, 225)
        ];
        // Point (160, 130) is closer to A center (125,125) = ~38px
        let found = find_nearest_box(&boxes, 160, 130, 50).unwrap();
        assert_eq!(found.label, "A");
    }

    #[test]
    fn test_find_nearest_box_too_far() {
        let boxes = vec![make_box("A", 100, 100, 50, 50, None)]; // center = (125, 125)
                                                                 // Point (300, 300) is ~247px away, beyond 50px threshold
        assert!(find_nearest_box(&boxes, 300, 300, 50).is_none());
    }

    // ─── FallbackDetector ────────────────────────

    #[test]
    fn test_fallback_detector_name() {
        use std::sync::Arc;

        struct MockDetector(&'static str);
        #[async_trait::async_trait]
        impl VisionDetector for MockDetector {
            fn name(&self) -> &str {
                self.0
            }
            fn supports_multi_point(&self) -> bool {
                false
            }
            async fn detect(
                &self,
                _input: &crate::vision_detector::DetectionInput,
                _task: &str,
            ) -> anyhow::Result<Vec<crate::vision_detector::DetectionResult>> {
                Ok(vec![])
            }
        }

        let primary: Arc<dyn VisionDetector> = Arc::new(MockDetector("primary"));
        let fallback: Arc<dyn VisionDetector> = Arc::new(MockDetector("fallback"));
        let detector = FallbackDetector::new(primary, fallback);
        assert_eq!(detector.name(), "fallback");
    }

    // ─── Stop words ──────────────────────────────

    #[test]
    fn test_is_stop_word() {
        assert!(is_stop_word("click"));
        assert!(is_stop_word("the"));
        assert!(is_stop_word("blue"));
        assert!(!is_stop_word("submit"));
        assert!(!is_stop_word("username"));
    }

    // ─── Infer element type ──────────────────────

    #[test]
    fn test_infer_element_type() {
        assert_eq!(
            infer_element_type("click the button"),
            Some("button".into())
        );
        assert_eq!(
            infer_element_type("type in the input field"),
            Some("input".into())
        );
        assert_eq!(
            infer_element_type("select from dropdown"),
            Some("combobox".into())
        );
        assert!(infer_element_type("do something").is_none());
    }
}
