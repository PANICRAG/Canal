//! Multimodal processing module.
//!
//! Detects content modality (text/vision/hybrid) from incoming requests
//! and routes to the appropriate model via `MultimodalRoutingStrategy`.

mod router;

pub use router::MultimodalRoutingStrategy;

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::llm::router::{ChatRequest, ContentBlock};

/// Content modality detected from a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentModality {
    /// Request contains only text content.
    TextOnly,
    /// Request contains only image/vision content.
    VisionOnly,
    /// Request contains both text reasoning and vision content.
    Hybrid,
}

/// Detects the content modality of incoming chat requests.
///
/// Uses a 3-rule priority chain:
/// 1. Content block scan (Image blocks in messages)
/// 2. Task type hint ("browser", "vision" in task_type field)
/// 3. Keyword scan (vision-related keywords in last user message)
#[derive(Debug, Clone)]
pub struct MultimodalDetector {
    /// Keywords that suggest vision processing (e.g., "screenshot", "image")
    vision_keywords: Vec<String>,
    /// MIME types that indicate vision content
    vision_mime_types: HashSet<String>,
    /// Tool names that produce visual artifacts
    vision_tool_names: HashSet<String>,
    /// Task types that imply vision
    vision_task_types: HashSet<String>,
}

impl Default for MultimodalDetector {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl MultimodalDetector {
    /// Create a detector with default rules.
    pub fn with_defaults() -> Self {
        Self {
            vision_keywords: vec![
                "screenshot".into(),
                "image".into(),
                "picture".into(),
                "photo".into(),
                "screen".into(),
                "visual".into(),
                "ui".into(),
            ],
            vision_mime_types: HashSet::from([
                "image/png".into(),
                "image/jpeg".into(),
                "image/webp".into(),
                "image/gif".into(),
                "image/svg+xml".into(),
            ]),
            vision_tool_names: HashSet::from([
                "computer_screenshot".into(),
                "omniparser_detect".into(),
                "uitars_detect".into(),
            ]),
            vision_task_types: HashSet::from(["browser".into(), "vision".into(), "visual".into()]),
        }
    }

    /// Create an empty detector (for testing).
    pub fn empty() -> Self {
        Self {
            vision_keywords: Vec::new(),
            vision_mime_types: HashSet::new(),
            vision_tool_names: HashSet::new(),
            vision_task_types: HashSet::new(),
        }
    }

    /// Detect the content modality of a chat request.
    ///
    /// # Detection Rules (priority order)
    ///
    /// 1. **Content Block Scan**: Check messages for `ContentBlock::Image`
    /// 2. **Task Type Hint**: Check `request.task_type` for vision-related types
    /// 3. **Keyword Scan**: Check last user message for vision keywords
    #[tracing::instrument(skip(self, request), fields(modality))]
    pub fn detect(&self, request: &ChatRequest) -> ContentModality {
        let has_image = self.has_image_content(request);
        let has_text = self.has_text_content(request);

        // Rule 1: Explicit image content blocks
        if has_image && !has_text {
            tracing::Span::current().record("modality", "vision_only");
            return ContentModality::VisionOnly;
        }
        if has_image && has_text {
            tracing::Span::current().record("modality", "hybrid");
            return ContentModality::Hybrid;
        }

        // Rule 2: Task type hint
        if let Some(ref task_type) = request.task_type {
            if self
                .vision_task_types
                .contains(&task_type.trim().to_lowercase())
            {
                tracing::Span::current().record("modality", "hybrid");
                return ContentModality::Hybrid;
            }
        }

        // Rule 3: Vision keywords in last user message
        if self.has_vision_keywords(request) {
            tracing::Span::current().record("modality", "hybrid");
            return ContentModality::Hybrid;
        }

        tracing::Span::current().record("modality", "text_only");
        ContentModality::TextOnly
    }

    /// Check if any message contains image content blocks.
    fn has_image_content(&self, request: &ChatRequest) -> bool {
        request.messages.iter().any(|m| {
            m.content_blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. }))
        })
    }

    /// Check if any message contains text content (either plain or blocks).
    fn has_text_content(&self, request: &ChatRequest) -> bool {
        request.messages.iter().any(|m| {
            !m.content.is_empty()
                || m.content_blocks
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { .. }))
        })
    }

    /// Check if the last user message contains vision-related keywords.
    fn has_vision_keywords(&self, request: &ChatRequest) -> bool {
        let last_user = request.messages.iter().rev().find(|m| m.role == "user");
        if let Some(msg) = last_user {
            let text = if !msg.content.is_empty() {
                msg.content.to_lowercase()
            } else {
                msg.content_blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.to_lowercase()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            // Use word-boundary matching to avoid false positives
            // (e.g., "screenwriter" should not match "screen")
            let text_words: Vec<String> = text
                .split_whitespace()
                .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
                .collect();
            return self
                .vision_keywords
                .iter()
                .any(|kw| text_words.iter().any(|w| w == kw));
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::router::{ContentBlock, Message};

    fn text_request(text: &str) -> ChatRequest {
        ChatRequest {
            messages: vec![Message::text("user", text)],
            ..Default::default()
        }
    }

    fn image_request() -> ChatRequest {
        ChatRequest {
            messages: vec![Message {
                role: "user".into(),
                content: String::new(),
                content_blocks: vec![ContentBlock::Image {
                    source_type: "base64".into(),
                    media_type: "image/png".into(),
                    data: "iVBOR...".into(),
                }],
            }],
            ..Default::default()
        }
    }

    fn hybrid_request() -> ChatRequest {
        ChatRequest {
            messages: vec![Message {
                role: "user".into(),
                content: "What is in this image?".into(),
                content_blocks: vec![ContentBlock::Image {
                    source_type: "base64".into(),
                    media_type: "image/png".into(),
                    data: "iVBOR...".into(),
                }],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn test_text_only_detection() {
        let detector = MultimodalDetector::with_defaults();
        assert_eq!(
            detector.detect(&text_request("Hello world")),
            ContentModality::TextOnly
        );
    }

    #[test]
    fn test_vision_only_detection() {
        let detector = MultimodalDetector::with_defaults();
        assert_eq!(
            detector.detect(&image_request()),
            ContentModality::VisionOnly
        );
    }

    #[test]
    fn test_hybrid_detection_image_plus_text() {
        let detector = MultimodalDetector::with_defaults();
        assert_eq!(detector.detect(&hybrid_request()), ContentModality::Hybrid);
    }

    #[test]
    fn test_keyword_detection_english() {
        let detector = MultimodalDetector::with_defaults();
        assert_eq!(
            detector.detect(&text_request("Take a screenshot of the page")),
            ContentModality::Hybrid,
        );
    }

    #[test]
    fn test_task_type_hint_browser() {
        let detector = MultimodalDetector::with_defaults();
        let mut req = text_request("Click the button");
        req.task_type = Some("browser".into());
        assert_eq!(detector.detect(&req), ContentModality::Hybrid);
    }

    #[test]
    fn test_task_type_hint_code_stays_text() {
        let detector = MultimodalDetector::with_defaults();
        let mut req = text_request("Write a function");
        req.task_type = Some("code".into());
        assert_eq!(detector.detect(&req), ContentModality::TextOnly);
    }

    #[test]
    fn test_empty_detector_always_text() {
        let detector = MultimodalDetector::empty();
        assert_eq!(
            detector.detect(&text_request("Take a screenshot")),
            ContentModality::TextOnly
        );
    }

    #[test]
    fn test_empty_request() {
        let detector = MultimodalDetector::with_defaults();
        let req = ChatRequest {
            messages: vec![],
            ..Default::default()
        };
        assert_eq!(detector.detect(&req), ContentModality::TextOnly);
    }

    #[test]
    fn test_no_false_positive_for_similar_words() {
        let detector = MultimodalDetector::with_defaults();
        assert_eq!(
            detector.detect(&text_request("Please do a code review")),
            ContentModality::TextOnly,
        );
    }

    #[test]
    fn test_default_is_same_as_with_defaults() {
        let d1 = MultimodalDetector::default();
        let d2 = MultimodalDetector::with_defaults();
        assert_eq!(d1.vision_keywords, d2.vision_keywords);
    }

    #[test]
    fn test_multiple_messages_mixed() {
        let detector = MultimodalDetector::with_defaults();
        let req = ChatRequest {
            messages: vec![
                Message::text("user", "Hello"),
                Message::text("assistant", "Hi there!"),
                Message {
                    role: "user".into(),
                    content: "What is in this image?".into(),
                    content_blocks: vec![ContentBlock::Image {
                        source_type: "base64".into(),
                        media_type: "image/png".into(),
                        data: "test".into(),
                    }],
                },
            ],
            ..Default::default()
        };
        assert_eq!(detector.detect(&req), ContentModality::Hybrid);
    }

    #[test]
    fn test_content_modality_serialization() {
        let modality = ContentModality::Hybrid;
        let json = serde_json::to_string(&modality).unwrap();
        assert_eq!(json, "\"hybrid\"");

        let deserialized: ContentModality = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ContentModality::Hybrid);
    }

    #[test]
    fn test_content_modality_all_variants_serialize() {
        let variants = vec![
            (ContentModality::TextOnly, "\"text_only\""),
            (ContentModality::VisionOnly, "\"vision_only\""),
            (ContentModality::Hybrid, "\"hybrid\""),
        ];
        for (variant, expected) in variants {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected);
        }
    }

    #[test]
    fn test_keyword_in_content_blocks() {
        let detector = MultimodalDetector::with_defaults();
        let req = ChatRequest {
            messages: vec![Message {
                role: "user".into(),
                content: String::new(),
                content_blocks: vec![ContentBlock::Text {
                    text: "Take a screenshot of the page".into(),
                }],
            }],
            ..Default::default()
        };
        assert_eq!(detector.detect(&req), ContentModality::Hybrid);
    }

    #[test]
    fn test_keyword_only_checks_last_user_message() {
        let detector = MultimodalDetector::with_defaults();
        let req = ChatRequest {
            messages: vec![
                Message::text("user", "Take a screenshot"),
                Message::text("assistant", "Here is the screenshot"),
                Message::text("user", "Write a function"),
            ],
            ..Default::default()
        };
        // Last user message is "Write a function" which has no vision keywords
        assert_eq!(detector.detect(&req), ContentModality::TextOnly);
    }

    #[test]
    fn test_task_type_case_insensitive() {
        let detector = MultimodalDetector::with_defaults();
        let mut req = text_request("Click the button");
        req.task_type = Some("Browser".into());
        assert_eq!(detector.detect(&req), ContentModality::Hybrid);
    }
}
