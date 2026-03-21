//! Context Compaction - Summarize and compress conversation history
//!
//! This module provides intelligent context compression to manage token usage
//! in long conversations. It uses LLM-based summarization to preserve context
//! while reducing token count.
//!
//! # Features
//!
//! - Automatic token estimation (character-based or tiktoken-compatible)
//! - Threshold-triggered compaction (default 80%)
//! - LLM-based message summarization
//! - Preservation of recent messages while maintaining context
//!
//! # Usage
//!
//! ```ignore
//! use gateway_core::agent::session::{ContextCompactor, CompactTrigger};
//!
//! let compactor = ContextCompactor::builder()
//!     .max_tokens(100_000)
//!     .threshold_ratio(0.8)
//!     .keep_recent(10)
//!     .with_llm_router(router)
//!     .build();
//!
//! let result = compactor.compact_if_needed(&messages).await?;
//! ```

use crate::agent::types::{AgentMessage, ContentBlock, SystemMessage};
use crate::llm::{ChatRequest, LlmRouter, Message as LlmMessage};
use async_trait::async_trait;
use std::sync::Arc;

/// Token estimation strategy
#[derive(Debug, Clone, Copy, Default)]
pub enum TokenEstimationStrategy {
    /// Simple character-based estimation (4 chars per token)
    #[default]
    CharacterBased,
    /// More accurate word-based estimation (0.75 tokens per word)
    WordBased,
    /// Use a precise token count (requires external tokenizer)
    Precise,
}

/// Context compaction trigger
#[derive(Debug, Clone, Copy)]
pub enum CompactTrigger {
    /// Token limit exceeded
    TokenLimit(usize),
    /// Message count exceeded
    MessageCount(usize),
    /// Automatic threshold trigger (e.g., 80%)
    ThresholdExceeded { current: usize, threshold: usize },
    /// Manual trigger
    Manual,
}

/// Configuration for context compaction
#[derive(Debug, Clone)]
pub struct CompactConfig {
    /// Maximum tokens before compaction is triggered
    pub max_tokens: usize,
    /// Target tokens after compaction
    pub target_tokens: usize,
    /// Threshold ratio to trigger compaction (e.g., 0.8 for 80%)
    pub threshold_ratio: f64,
    /// Number of recent messages to always keep
    pub keep_recent: usize,
    /// Token estimation strategy
    pub estimation_strategy: TokenEstimationStrategy,
    /// Model to use for summarization
    pub summarization_model: Option<String>,
    /// Maximum tokens for summary output
    pub summary_max_tokens: u32,
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            // Conservative defaults for multilingual support (Chinese uses ~1.5 chars/token)
            max_tokens: 40_000, // Trigger compaction check at 40k estimated tokens
            target_tokens: 20_000, // Target 20k tokens after compaction
            threshold_ratio: 0.7, // Trigger at 70% (28k tokens) for safety margin
            keep_recent: 5,     // Keep fewer messages to be more aggressive
            estimation_strategy: TokenEstimationStrategy::CharacterBased,
            summarization_model: None,
            summary_max_tokens: 2000,
        }
    }
}

/// Builder for ContextCompactor
pub struct ContextCompactorBuilder {
    config: CompactConfig,
    llm_router: Option<Arc<LlmRouter>>,
    custom_summarizer: Option<Box<dyn Summarizer>>,
}

impl Default for ContextCompactorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextCompactorBuilder {
    /// Create a new builder with default config
    pub fn new() -> Self {
        Self {
            config: CompactConfig::default(),
            llm_router: None,
            custom_summarizer: None,
        }
    }

    /// Set maximum tokens
    pub fn max_tokens(mut self, tokens: usize) -> Self {
        self.config.max_tokens = tokens;
        self
    }

    /// Set target tokens after compaction
    pub fn target_tokens(mut self, tokens: usize) -> Self {
        self.config.target_tokens = tokens;
        self
    }

    /// Set threshold ratio (0.0 - 1.0)
    pub fn threshold_ratio(mut self, ratio: f64) -> Self {
        self.config.threshold_ratio = ratio.clamp(0.0, 1.0);
        self
    }

    /// Set number of recent messages to keep
    pub fn keep_recent(mut self, count: usize) -> Self {
        self.config.keep_recent = count;
        self
    }

    /// Set token estimation strategy
    pub fn estimation_strategy(mut self, strategy: TokenEstimationStrategy) -> Self {
        self.config.estimation_strategy = strategy;
        self
    }

    /// Set LLM router for summarization
    pub fn with_llm_router(mut self, router: Arc<LlmRouter>) -> Self {
        self.llm_router = Some(router);
        self
    }

    /// Set custom summarizer
    pub fn with_summarizer(mut self, summarizer: Box<dyn Summarizer>) -> Self {
        self.custom_summarizer = Some(summarizer);
        self
    }

    /// Set summarization model
    pub fn summarization_model(mut self, model: impl Into<String>) -> Self {
        self.config.summarization_model = Some(model.into());
        self
    }

    /// Set summary max tokens
    pub fn summary_max_tokens(mut self, tokens: u32) -> Self {
        self.config.summary_max_tokens = tokens;
        self
    }

    /// Build the ContextCompactor
    pub fn build(self) -> ContextCompactor {
        // If we have an LLM router, create an LLM-based summarizer
        let summarizer = if let Some(custom) = self.custom_summarizer {
            Some(custom)
        } else {
            self.llm_router.as_ref().map(|router| {
                Box::new(LlmSummarizer::new(
                    router.clone(),
                    self.config.summarization_model.clone(),
                    self.config.summary_max_tokens,
                )) as Box<dyn Summarizer>
            })
        };

        ContextCompactor {
            config: self.config,
            llm_router: self.llm_router,
            summarizer,
        }
    }
}

/// Context compactor for managing conversation length
pub struct ContextCompactor {
    /// Configuration
    config: CompactConfig,
    /// LLM router for summarization (kept for potential direct access)
    #[allow(dead_code)]
    llm_router: Option<Arc<LlmRouter>>,
    /// Summarizer function
    summarizer: Option<Box<dyn Summarizer>>,
}

impl Default for ContextCompactor {
    fn default() -> Self {
        Self {
            config: CompactConfig::default(),
            llm_router: None,
            summarizer: None,
        }
    }
}

impl ContextCompactor {
    /// Create a new compactor with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a builder for configuring the compactor
    pub fn builder() -> ContextCompactorBuilder {
        ContextCompactorBuilder::new()
    }

    /// Set max tokens (for backward compatibility)
    pub fn max_tokens(mut self, tokens: usize) -> Self {
        self.config.max_tokens = tokens;
        self
    }

    /// Set target tokens (for backward compatibility)
    pub fn target_tokens(mut self, tokens: usize) -> Self {
        self.config.target_tokens = tokens;
        self
    }

    /// Set recent messages to keep (for backward compatibility)
    pub fn keep_recent(mut self, count: usize) -> Self {
        self.config.keep_recent = count;
        self
    }

    /// Set summarizer (for backward compatibility)
    pub fn with_summarizer(mut self, summarizer: Box<dyn Summarizer>) -> Self {
        self.summarizer = Some(summarizer);
        self
    }

    /// Get current config
    pub fn config(&self) -> &CompactConfig {
        &self.config
    }

    /// Calculate the threshold token count
    pub fn threshold_tokens(&self) -> usize {
        (self.config.max_tokens as f64 * self.config.threshold_ratio) as usize
    }

    /// Estimate token count for messages using the configured strategy
    pub fn estimate_tokens(&self, messages: &[AgentMessage]) -> usize {
        match self.config.estimation_strategy {
            TokenEstimationStrategy::CharacterBased => self.estimate_tokens_char_based(messages),
            TokenEstimationStrategy::WordBased => self.estimate_tokens_word_based(messages),
            TokenEstimationStrategy::Precise => {
                // Fall back to character-based for now
                // In a production system, this would use tiktoken or similar
                self.estimate_tokens_char_based(messages)
            }
        }
    }

    /// Character-based token estimation
    /// Uses adaptive estimation based on content:
    /// - Chinese/CJK text: ~1.5 chars per token (conservative)
    /// - English text: ~4 chars per token
    /// - JSON/code: ~3 chars per token
    fn estimate_tokens_char_based(&self, messages: &[AgentMessage]) -> usize {
        messages
            .iter()
            .map(|m| {
                let text = self.message_to_text(m);
                let chars = text.len();

                // Count CJK characters (Chinese, Japanese, Korean)
                let cjk_count = text
                    .chars()
                    .filter(|c| {
                        let code = *c as u32;
                        // CJK Unified Ideographs and common ranges
                        (0x4E00..=0x9FFF).contains(&code) ||  // CJK Unified Ideographs
                    (0x3400..=0x4DBF).contains(&code) ||  // CJK Extension A
                    (0x3000..=0x303F).contains(&code) ||  // CJK Symbols
                    (0xFF00..=0xFFEF).contains(&code) // Fullwidth forms
                    })
                    .count();

                // Calculate ratio of CJK characters
                let cjk_ratio = if chars > 0 {
                    cjk_count as f64 / chars as f64
                } else {
                    0.0
                };

                // Adaptive chars_per_token based on content type
                let chars_per_token = if cjk_ratio > 0.3 {
                    // Significant CJK content: use conservative estimate
                    1.5
                } else if text.contains('{') || text.contains('[') {
                    // JSON/structured data
                    3.0
                } else {
                    // English/mixed text
                    4.0
                };

                // Add overhead for message structure (role, etc.)
                ((chars as f64 / chars_per_token) as usize) + 10
            })
            .sum()
    }

    /// Word-based token estimation (approximately 0.75 tokens per word)
    fn estimate_tokens_word_based(&self, messages: &[AgentMessage]) -> usize {
        messages
            .iter()
            .map(|m| {
                let text = self.message_to_text(m);
                let word_count = text.split_whitespace().count();
                // Approximately 0.75 tokens per word, plus message overhead
                ((word_count as f64 * 1.33) as usize) + 4
            })
            .sum()
    }

    /// Get character count for a message
    #[allow(dead_code)]
    fn message_char_count(&self, message: &AgentMessage) -> usize {
        match message {
            AgentMessage::User(m) => m.content.to_string_content().len(),
            AgentMessage::Assistant(m) => m
                .content
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => text.len(),
                    ContentBlock::Thinking { thinking, .. } => thinking.len(),
                    ContentBlock::ToolUse { input, .. } => input.to_string().len(),
                    ContentBlock::ToolResult { content, .. } => content
                        .as_ref()
                        .map(|c| c.to_string_content().len())
                        .unwrap_or(0),
                    _ => 0,
                })
                .sum(),
            AgentMessage::System(m) => m.data.to_string().len(),
            AgentMessage::Result(m) => m.result.as_ref().map(|r| r.len()).unwrap_or(0),
            AgentMessage::StreamEvent(_) => 0,
            AgentMessage::PermissionRequest(_) => 0,
        }
    }

    /// Convert message to text for word counting
    fn message_to_text(&self, message: &AgentMessage) -> String {
        match message {
            AgentMessage::User(m) => m.content.to_string_content(),
            AgentMessage::Assistant(m) => m
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    ContentBlock::Thinking { thinking, .. } => Some(thinking.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
            AgentMessage::System(m) => m.data.to_string(),
            AgentMessage::Result(m) => m.result.clone().unwrap_or_default(),
            AgentMessage::StreamEvent(_) => String::new(),
            AgentMessage::PermissionRequest(_) => String::new(),
        }
    }

    /// Check if compaction is needed based on threshold
    pub fn needs_compaction(&self, messages: &[AgentMessage]) -> bool {
        let current_tokens = self.estimate_tokens(messages);
        current_tokens >= self.threshold_tokens()
    }

    /// Check if compaction is needed and return the trigger if so
    pub fn check_compaction_trigger(&self, messages: &[AgentMessage]) -> Option<CompactTrigger> {
        let current_tokens = self.estimate_tokens(messages);
        let threshold = self.threshold_tokens();

        if current_tokens >= threshold {
            Some(CompactTrigger::ThresholdExceeded {
                current: current_tokens,
                threshold,
            })
        } else {
            None
        }
    }

    /// Compact messages if needed, otherwise return original
    pub async fn compact_if_needed(
        &self,
        messages: &[AgentMessage],
    ) -> Result<CompactionResult, CompactionError> {
        if let Some(trigger) = self.check_compaction_trigger(messages) {
            self.compact(messages, trigger).await
        } else {
            let current_tokens = self.estimate_tokens(messages);
            Ok(CompactionResult {
                messages: messages.to_vec(),
                summary: None,
                tokens_before: current_tokens,
                tokens_after: current_tokens,
                messages_removed: 0,
                was_compacted: false,
            })
        }
    }

    /// Compact messages
    pub async fn compact(
        &self,
        messages: &[AgentMessage],
        trigger: CompactTrigger,
    ) -> Result<CompactionResult, CompactionError> {
        let current_tokens = self.estimate_tokens(messages);

        // Don't compact if under threshold (unless manual)
        if !matches!(trigger, CompactTrigger::Manual) && current_tokens < self.threshold_tokens() {
            return Ok(CompactionResult {
                messages: messages.to_vec(),
                summary: None,
                tokens_before: current_tokens,
                tokens_after: current_tokens,
                messages_removed: 0,
                was_compacted: false,
            });
        }

        let total = messages.len();
        if total <= self.config.keep_recent {
            return Ok(CompactionResult {
                messages: messages.to_vec(),
                summary: None,
                tokens_before: current_tokens,
                tokens_after: current_tokens,
                messages_removed: 0,
                was_compacted: false,
            });
        }

        // Split into old and recent
        let split_point = total.saturating_sub(self.config.keep_recent);
        let old_messages = &messages[..split_point];
        let recent_messages = &messages[split_point..];

        // Generate summary
        let summary = if let Some(summarizer) = &self.summarizer {
            match summarizer.summarize(old_messages).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("LLM summarization failed, using default: {}", e);
                    self.default_summary(old_messages)
                }
            }
        } else {
            self.default_summary(old_messages)
        };

        // Create compacted message list
        let mut compacted = Vec::with_capacity(1 + recent_messages.len());

        // Add summary as system message
        compacted.push(AgentMessage::System(SystemMessage {
            subtype: "context_summary".to_string(),
            data: serde_json::json!({
                "summary": summary,
                "compacted_messages": old_messages.len(),
                "trigger": format!("{:?}", trigger),
                "original_token_count": current_tokens,
            }),
        }));

        // Add recent messages
        compacted.extend(recent_messages.iter().cloned());

        let tokens_after = self.estimate_tokens(&compacted);

        tracing::info!(
            tokens_before = current_tokens,
            tokens_after = tokens_after,
            messages_removed = old_messages.len(),
            messages_kept = recent_messages.len(),
            "Context compacted"
        );

        Ok(CompactionResult {
            messages: compacted,
            summary: Some(summary),
            tokens_before: current_tokens,
            tokens_after,
            messages_removed: old_messages.len(),
            was_compacted: true,
        })
    }

    /// Generate default summary without LLM
    fn default_summary(&self, messages: &[AgentMessage]) -> String {
        let mut summary = String::from("Previous conversation summary:\n\n");

        let mut tool_uses = Vec::new();
        let mut topics = Vec::new();

        for msg in messages {
            match msg {
                AgentMessage::User(m) => {
                    let content = m.content.to_string_content();
                    if content.len() > 100 {
                        // Safe UTF-8 truncation to avoid panic on multi-byte chars
                        let safe_end = content
                            .char_indices()
                            .take_while(|(i, _)| *i < 100)
                            .last()
                            .map(|(i, c)| i + c.len_utf8())
                            .unwrap_or(content.len().min(100));
                        topics.push(format!("- User: {}...", &content[..safe_end]));
                    } else if !content.is_empty() {
                        topics.push(format!("- User: {}", content));
                    }
                }
                AgentMessage::Assistant(m) => {
                    for block in &m.content {
                        if let ContentBlock::ToolUse { name, .. } = block {
                            tool_uses.push(name.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        if !topics.is_empty() {
            summary.push_str("Key interactions:\n");
            for topic in topics.iter().take(5) {
                summary.push_str(topic);
                summary.push('\n');
            }
            if topics.len() > 5 {
                summary.push_str(&format!("... and {} more interactions\n", topics.len() - 5));
            }
            summary.push('\n');
        }

        if !tool_uses.is_empty() {
            summary.push_str("Tools used: ");
            let unique_tools: Vec<_> = tool_uses
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .take(10)
                .collect();
            summary.push_str(
                &unique_tools
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            summary.push('\n');
        }

        summary
    }
}

/// Summarizer trait for LLM-based summarization
#[async_trait]
pub trait Summarizer: Send + Sync {
    /// Generate a summary of the given messages
    async fn summarize(&self, messages: &[AgentMessage]) -> Result<String, CompactionError>;
}

/// LLM-based summarizer using the LlmRouter
pub struct LlmSummarizer {
    router: Arc<LlmRouter>,
    model: Option<String>,
    max_tokens: u32,
}

impl LlmSummarizer {
    /// Create a new LLM summarizer
    pub fn new(router: Arc<LlmRouter>, model: Option<String>, max_tokens: u32) -> Self {
        Self {
            router,
            model,
            max_tokens,
        }
    }

    /// Convert AgentMessages to a text representation for summarization
    fn messages_to_text(&self, messages: &[AgentMessage]) -> String {
        let mut text = String::new();

        for (i, msg) in messages.iter().enumerate() {
            match msg {
                AgentMessage::User(m) => {
                    text.push_str(&format!(
                        "[{}] User: {}\n",
                        i + 1,
                        m.content.to_string_content()
                    ));
                }
                AgentMessage::Assistant(m) => {
                    let content: String = m
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.clone()),
                            ContentBlock::ToolUse { name, .. } => Some(format!("[Tool: {}]", name)),
                            ContentBlock::ToolResult { content, .. } => {
                                content.as_ref().map(|c| {
                                    let s = c.to_string_content();
                                    if s.len() > 200 {
                                        // Safe truncation at char boundary
                                        let safe_end = s
                                            .char_indices()
                                            .take_while(|(i, _)| *i < 200)
                                            .last()
                                            .map(|(i, c)| i + c.len_utf8())
                                            .unwrap_or(s.len().min(200));
                                        format!("[Result: {}...]", &s[..safe_end])
                                    } else {
                                        format!("[Result: {}]", s)
                                    }
                                })
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    text.push_str(&format!("[{}] Assistant: {}\n", i + 1, content));
                }
                AgentMessage::System(m) => {
                    text.push_str(&format!("[{}] System: {}\n", i + 1, m.subtype));
                }
                _ => {}
            }
        }

        text
    }
}

#[async_trait]
impl Summarizer for LlmSummarizer {
    async fn summarize(&self, messages: &[AgentMessage]) -> Result<String, CompactionError> {
        let conversation_text = self.messages_to_text(messages);

        let system_prompt = r#"You are a conversation summarizer. Your task is to create a concise but comprehensive summary of the conversation history provided.

Focus on:
1. Key topics discussed
2. Important decisions made
3. Tools used and their outcomes
4. Any unresolved questions or tasks
5. Context that would be important for continuing the conversation

Be concise but preserve essential context. The summary should allow someone to continue the conversation naturally."#;

        let user_prompt = format!(
            "Please summarize the following conversation:\n\n{}\n\nProvide a structured summary.",
            conversation_text
        );

        let request = ChatRequest {
            messages: vec![
                LlmMessage::text("system", system_prompt),
                LlmMessage::text("user", user_prompt),
            ],
            model: self.model.clone(),
            max_tokens: Some(self.max_tokens),
            temperature: Some(0.3), // Lower temperature for consistent summaries
            stream: false,
            tools: vec![],
            tool_choice: None,
            ..Default::default()
        };

        match self.router.route(request).await {
            Ok(response) => {
                let summary = response
                    .choices
                    .first()
                    .map(|c| c.message.content.clone())
                    .unwrap_or_else(|| "Unable to generate summary.".to_string());

                Ok(summary)
            }
            Err(e) => Err(CompactionError::SummarizationFailed(e.to_string())),
        }
    }
}

/// Compaction result
#[derive(Debug)]
pub struct CompactionResult {
    /// Compacted messages
    pub messages: Vec<AgentMessage>,
    /// Generated summary
    pub summary: Option<String>,
    /// Token count before compaction
    pub tokens_before: usize,
    /// Token count after compaction
    pub tokens_after: usize,
    /// Number of messages removed
    pub messages_removed: usize,
    /// Whether compaction actually occurred
    pub was_compacted: bool,
}

impl CompactionResult {
    /// Get the token reduction ratio
    pub fn reduction_ratio(&self) -> f64 {
        if self.tokens_before == 0 {
            0.0
        } else {
            1.0 - (self.tokens_after as f64 / self.tokens_before as f64)
        }
    }

    /// Get the number of tokens saved
    pub fn tokens_saved(&self) -> usize {
        self.tokens_before.saturating_sub(self.tokens_after)
    }
}

/// Compaction error
#[derive(Debug)]
pub enum CompactionError {
    /// Summarization failed
    SummarizationFailed(String),
    /// Invalid configuration
    InvalidConfig(String),
}

impl std::fmt::Display for CompactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SummarizationFailed(msg) => write!(f, "Summarization failed: {}", msg),
            Self::InvalidConfig(msg) => write!(f, "Invalid configuration: {}", msg),
        }
    }
}

impl std::error::Error for CompactionError {}

/// Statistics about context usage
#[derive(Debug, Clone, Default)]
pub struct ContextStats {
    /// Current token count
    pub current_tokens: usize,
    /// Maximum allowed tokens
    pub max_tokens: usize,
    /// Threshold tokens (when compaction triggers)
    pub threshold_tokens: usize,
    /// Usage percentage
    pub usage_percent: f64,
    /// Number of messages
    pub message_count: usize,
    /// Number of compactions performed
    pub compaction_count: usize,
    /// Total tokens saved by compaction
    pub total_tokens_saved: usize,
}

impl ContextStats {
    /// Check if approaching the threshold
    pub fn is_warning(&self) -> bool {
        self.usage_percent >= 70.0 && self.usage_percent < 80.0
    }

    /// Check if at or above threshold
    pub fn needs_compaction(&self) -> bool {
        self.usage_percent >= 80.0
    }
}

/// Extension trait for Session to support compaction
#[async_trait]
pub trait CompactableSession {
    /// Get context statistics
    async fn context_stats(&self, compactor: &ContextCompactor) -> ContextStats;

    /// Compact the session if needed
    async fn compact_if_needed(
        &mut self,
        compactor: &ContextCompactor,
    ) -> Result<Option<CompactionResult>, CompactionError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::{AssistantMessage, MessageContent, UserMessage};

    fn create_test_messages(count: usize) -> Vec<AgentMessage> {
        (0..count)
            .map(|i| {
                AgentMessage::User(UserMessage {
                    content: MessageContent::text(format!(
                        "Message {} with some additional text to make it longer",
                        i
                    )),
                    uuid: None,
                    parent_tool_use_id: None,
                    tool_use_result: None,
                })
            })
            .collect()
    }

    fn create_long_messages(count: usize, chars_per_message: usize) -> Vec<AgentMessage> {
        (0..count)
            .map(|i| {
                let text = format!("Message {}: {}", i, "x".repeat(chars_per_message));
                AgentMessage::User(UserMessage {
                    content: MessageContent::text(text),
                    uuid: None,
                    parent_tool_use_id: None,
                    tool_use_result: None,
                })
            })
            .collect()
    }

    #[test]
    fn test_builder() {
        let compactor = ContextCompactor::builder()
            .max_tokens(50_000)
            .threshold_ratio(0.75)
            .keep_recent(5)
            .estimation_strategy(TokenEstimationStrategy::WordBased)
            .build();

        assert_eq!(compactor.config.max_tokens, 50_000);
        assert!((compactor.config.threshold_ratio - 0.75).abs() < f64::EPSILON);
        assert_eq!(compactor.config.keep_recent, 5);
    }

    #[test]
    fn test_threshold_calculation() {
        let compactor = ContextCompactor::builder()
            .max_tokens(100_000)
            .threshold_ratio(0.8)
            .build();

        assert_eq!(compactor.threshold_tokens(), 80_000);
    }

    #[test]
    fn test_estimate_tokens() {
        let compactor = ContextCompactor::new();
        let messages = create_test_messages(10);
        let tokens = compactor.estimate_tokens(&messages);
        assert!(tokens > 0);
    }

    #[test]
    fn test_needs_compaction() {
        let compactor = ContextCompactor::builder()
            .max_tokens(100)
            .threshold_ratio(0.8)
            .build();

        // Create messages that should exceed threshold
        let messages = create_long_messages(10, 100); // ~25 tokens each
        assert!(compactor.needs_compaction(&messages));

        // Create messages that should not exceed threshold
        let small_messages = create_test_messages(2);
        assert!(!compactor.needs_compaction(&small_messages));
    }

    #[test]
    fn test_check_compaction_trigger() {
        let compactor = ContextCompactor::builder()
            .max_tokens(100)
            .threshold_ratio(0.8)
            .build();

        let messages = create_long_messages(10, 100);
        let trigger = compactor.check_compaction_trigger(&messages);

        assert!(trigger.is_some());
        if let Some(CompactTrigger::ThresholdExceeded { current, threshold }) = trigger {
            assert!(current >= threshold);
        }
    }

    #[tokio::test]
    async fn test_compact_keeps_recent() {
        let compactor = ContextCompactor::builder()
            .max_tokens(0) // Force compaction
            .keep_recent(3)
            .build();

        let messages = create_test_messages(10);
        let result = compactor
            .compact(&messages, CompactTrigger::Manual)
            .await
            .unwrap();

        // Should have 1 summary + 3 recent = 4 messages
        assert_eq!(result.messages.len(), 4);
        assert_eq!(result.messages_removed, 7);
        assert!(result.summary.is_some());
        assert!(result.was_compacted);
    }

    #[tokio::test]
    async fn test_compact_no_op_when_small() {
        let compactor = ContextCompactor::builder()
            .max_tokens(1_000_000) // Very high limit
            .keep_recent(5)
            .build();

        let messages = create_test_messages(3);
        let result = compactor
            .compact(&messages, CompactTrigger::TokenLimit(100))
            .await
            .unwrap();

        // Should keep all messages
        assert_eq!(result.messages.len(), 3);
        assert_eq!(result.messages_removed, 0);
        assert!(!result.was_compacted);
    }

    #[tokio::test]
    async fn test_compact_if_needed() {
        let compactor = ContextCompactor::builder()
            .max_tokens(100)
            .threshold_ratio(0.8)
            .keep_recent(3)
            .build();

        // Should trigger compaction
        let messages = create_long_messages(10, 100);
        let result = compactor.compact_if_needed(&messages).await.unwrap();
        assert!(result.was_compacted);

        // Should not trigger compaction
        let small_messages = create_test_messages(2);
        let result = compactor.compact_if_needed(&small_messages).await.unwrap();
        assert!(!result.was_compacted);
    }

    #[test]
    fn test_compaction_result_metrics() {
        let result = CompactionResult {
            messages: vec![],
            summary: Some("Test summary".to_string()),
            tokens_before: 1000,
            tokens_after: 400,
            messages_removed: 10,
            was_compacted: true,
        };

        assert!((result.reduction_ratio() - 0.6).abs() < f64::EPSILON);
        assert_eq!(result.tokens_saved(), 600);
    }

    #[test]
    fn test_default_summary_generation() {
        let compactor = ContextCompactor::new();

        let messages = vec![
            AgentMessage::User(UserMessage {
                content: MessageContent::text("Hello, how are you?"),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            }),
            AgentMessage::Assistant(AssistantMessage {
                content: vec![
                    ContentBlock::Text {
                        text: "I'm doing well!".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool1".to_string(),
                        name: "filesystem_read".to_string(),
                        input: serde_json::json!({}),
                    },
                ],
                model: "test".to_string(),
                parent_tool_use_id: None,
                error: None,
            }),
        ];

        let summary = compactor.default_summary(&messages);
        assert!(summary.contains("Hello, how are you?"));
        assert!(summary.contains("filesystem_read"));
    }

    #[test]
    fn test_context_stats() {
        let stats = ContextStats {
            current_tokens: 75_000,
            max_tokens: 100_000,
            threshold_tokens: 80_000,
            usage_percent: 75.0,
            message_count: 50,
            compaction_count: 0,
            total_tokens_saved: 0,
        };

        assert!(stats.is_warning());
        assert!(!stats.needs_compaction());

        let high_stats = ContextStats {
            usage_percent: 85.0,
            ..stats
        };
        assert!(high_stats.needs_compaction());
    }

    #[test]
    fn test_word_based_estimation() {
        let compactor = ContextCompactor::builder()
            .estimation_strategy(TokenEstimationStrategy::WordBased)
            .build();

        let messages = create_test_messages(5);
        let tokens = compactor.estimate_tokens(&messages);

        // Word-based should give different results than character-based
        let char_compactor = ContextCompactor::new();
        let char_tokens = char_compactor.estimate_tokens(&messages);

        // Both should be non-zero
        assert!(tokens > 0);
        assert!(char_tokens > 0);
    }
}
