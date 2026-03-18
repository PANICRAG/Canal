//! ObservationNarrator — deterministic action narration for pipeline feedback.
//!
//! Adapted from Agent-S BBON (Behavior-Based Observation Narration).
//! Default mode is code-based (0 tokens, no hallucination).
//! Optional LLM mode for rich descriptions (~2K tokens per call).

use std::sync::Arc;

use crate::llm_client::{CvChatRequest, CvContent, CvLlmClient, CvMessage};
use crate::phash::hash_similarity;
use crate::types::ContextInfo;

/// Configuration for the observation narrator.
#[derive(Debug, Clone)]
pub struct NarrationConfig {
    /// Draw action markers on screenshots.
    pub enable_annotation: bool,
    /// Enable deterministic code-based narration (always recommended).
    pub enable_code_narration: bool,
    /// Enable LLM-based narration (expensive, ~2K tokens).
    pub enable_llm_narration: bool,
    /// Only use LLM narration when the action likely failed.
    pub llm_narration_on_failure_only: bool,
}

impl Default for NarrationConfig {
    fn default() -> Self {
        Self {
            enable_annotation: true,
            enable_code_narration: true,
            enable_llm_narration: false,
            llm_narration_on_failure_only: true,
        }
    }
}

/// Result of observing an action's effect.
#[derive(Debug, Clone)]
pub struct ActionObservation {
    /// Description of the action that was performed.
    pub action_description: String,
    /// Code-based narration of what happened (deterministic).
    pub narration: Option<String>,
    /// Similarity between before and after screenshots (0.0-1.0).
    pub similarity: f32,
}

/// Narrates action outcomes using deterministic code analysis.
///
/// Primary narration mode: `narrate_code()` — compares pHash similarity
/// and context changes to produce a description. Zero LLM tokens.
///
/// Optional: `narrate_llm()` — sends before/after screenshots to a VLM
/// for rich description. Expensive (~2K tokens), used sparingly.
pub struct ObservationNarrator {
    vision_llm: Option<Arc<dyn CvLlmClient>>,
    config: NarrationConfig,
}

impl ObservationNarrator {
    /// Create with code-only narration (no LLM).
    pub fn new(config: NarrationConfig) -> Self {
        Self {
            vision_llm: None,
            config,
        }
    }

    /// Create with optional LLM narration support.
    pub fn with_llm(config: NarrationConfig, vision_llm: Arc<dyn CvLlmClient>) -> Self {
        Self {
            vision_llm: Some(vision_llm),
            config,
        }
    }

    /// Deterministic code-based narration (0 tokens, no hallucination).
    ///
    /// Compares before/after pHash similarity and context changes to produce
    /// a human-readable description of what happened.
    ///
    /// # Returns
    /// A narration string like:
    /// - "Clicked Submit. Significant visual change (32% similarity)."
    /// - "Typed 'hello'. No visible change — action may have failed."
    pub fn narrate_code(
        &self,
        before_hash: u64,
        after_hash: u64,
        action: &str,
        before_context: Option<&ContextInfo>,
        after_context: Option<&ContextInfo>,
    ) -> String {
        let similarity = hash_similarity(before_hash, after_hash);
        let significant_change = similarity < 0.5;
        let context_changed = Self::context_changed(before_context, after_context);

        match (significant_change, context_changed) {
            (true, true) => format!("{action}. Context changed."),
            (true, false) => {
                format!(
                    "{action}. Significant visual change ({:.0}% similarity).",
                    similarity * 100.0
                )
            }
            (false, true) => {
                format!("{action}. Context changed but visual similar.")
            }
            (false, false) => {
                format!("{action}. No visible change — action may have failed.")
            }
        }
    }

    /// Full narration pipeline: code-based, optionally LLM-enhanced.
    ///
    /// Calls `narrate_code()` first. If LLM narration is enabled and the
    /// action appears to have failed, calls `narrate_llm()` for more detail.
    pub async fn narrate(
        &self,
        before_hash: u64,
        after_hash: u64,
        action: &str,
        before_context: Option<&ContextInfo>,
        after_context: Option<&ContextInfo>,
        before_base64: Option<&str>,
        after_base64: Option<&str>,
    ) -> ActionObservation {
        let similarity = hash_similarity(before_hash, after_hash);

        let mut narration = if self.config.enable_code_narration {
            Some(self.narrate_code(
                before_hash,
                after_hash,
                action,
                before_context,
                after_context,
            ))
        } else {
            None
        };

        // LLM narration: only if enabled, LLM available, and (always or on failure)
        if self.config.enable_llm_narration {
            let should_use_llm = if self.config.llm_narration_on_failure_only {
                similarity > 0.85 // No visible change = likely failure
            } else {
                true
            };

            if should_use_llm {
                if let (Some(before), Some(after)) = (before_base64, after_base64) {
                    if let Ok(llm_narration) = self.narrate_llm(before, after, action).await {
                        narration = Some(llm_narration);
                    }
                }
            }
        }

        ActionObservation {
            action_description: action.to_string(),
            narration,
            similarity,
        }
    }

    /// LLM-based narration using before/after screenshots (~2K tokens).
    ///
    /// Sends both screenshots to a vision LLM with a narration prompt.
    async fn narrate_llm(
        &self,
        before_base64: &str,
        after_base64: &str,
        action: &str,
    ) -> anyhow::Result<String> {
        let llm = self
            .vision_llm
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No vision LLM configured for narration"))?;

        let message = CvMessage::new(
            "user",
            vec![
                CvContent::Text {
                    text: format!(
                        "I performed the action: \"{action}\". \
                         Compare the before and after screenshots and describe what changed. \
                         Be concise (1-2 sentences)."
                    ),
                },
                CvContent::Image {
                    media_type: "image/jpeg".to_string(),
                    base64_data: before_base64.to_string(),
                },
                CvContent::Image {
                    media_type: "image/jpeg".to_string(),
                    base64_data: after_base64.to_string(),
                },
            ],
        );

        let request = CvChatRequest {
            messages: vec![message],
            model: None,
            max_tokens: Some(256),
            temperature: Some(0.1),
        };

        let response = llm
            .chat(request)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        Ok(response.text)
    }

    /// Check if context (URL, title, app) changed between captures.
    fn context_changed(before: Option<&ContextInfo>, after: Option<&ContextInfo>) -> bool {
        match (before, after) {
            (Some(b), Some(a)) => b.url != a.url || b.title != a.title || b.app_name != a.app_name,
            (None, Some(_)) | (Some(_), None) => true,
            (None, None) => false,
        }
    }

    /// Get the narrator's configuration.
    pub fn config(&self) -> &NarrationConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_narration_config_defaults() {
        let config = NarrationConfig::default();
        assert!(config.enable_annotation);
        assert!(config.enable_code_narration);
        assert!(!config.enable_llm_narration);
        assert!(config.llm_narration_on_failure_only);
    }

    #[test]
    fn test_narrate_code_significant_change() {
        let narrator = ObservationNarrator::new(NarrationConfig::default());
        // Opposite hashes = 0% similarity = significant change
        let result = narrator.narrate_code(0, u64::MAX, "Clicked Submit", None, None);
        assert!(result.contains("Significant visual change"));
    }

    #[test]
    fn test_narrate_code_no_change() {
        let narrator = ObservationNarrator::new(NarrationConfig::default());
        // Same hash = 100% similarity = no change
        let result = narrator.narrate_code(12345, 12345, "Clicked Submit", None, None);
        assert!(result.contains("No visible change"));
    }

    #[test]
    fn test_narrate_code_context_changed() {
        let narrator = ObservationNarrator::new(NarrationConfig::default());
        let before = ContextInfo {
            url: Some("https://example.com/login".into()),
            title: Some("Login".into()),
            app_name: Some("Browser".into()),
            interactive_elements: None,
        };
        let after = ContextInfo {
            url: Some("https://example.com/dashboard".into()),
            title: Some("Dashboard".into()),
            app_name: Some("Browser".into()),
            interactive_elements: None,
        };
        // Opposite hashes + context change
        let result =
            narrator.narrate_code(0, u64::MAX, "Clicked Login", Some(&before), Some(&after));
        assert!(result.contains("Context changed"));
    }

    #[test]
    fn test_narrate_code_context_same_visual_change() {
        let narrator = ObservationNarrator::new(NarrationConfig::default());
        let ctx = ContextInfo {
            url: Some("https://example.com".into()),
            title: Some("Page".into()),
            app_name: None,
            interactive_elements: None,
        };
        let result = narrator.narrate_code(0, u64::MAX, "Scrolled down", Some(&ctx), Some(&ctx));
        assert!(result.contains("Significant visual change"));
        assert!(!result.contains("Context changed"));
    }

    #[test]
    fn test_context_changed_detection() {
        let a = ContextInfo {
            url: Some("a".into()),
            title: None,
            app_name: None,
            interactive_elements: None,
        };
        let b = ContextInfo {
            url: Some("b".into()),
            title: None,
            app_name: None,
            interactive_elements: None,
        };
        assert!(ObservationNarrator::context_changed(Some(&a), Some(&b)));
        assert!(!ObservationNarrator::context_changed(Some(&a), Some(&a)));
        assert!(!ObservationNarrator::context_changed(None, None));
        assert!(ObservationNarrator::context_changed(None, Some(&a)));
    }
}
