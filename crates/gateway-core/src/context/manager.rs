//! Context Manager implementation

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use crate::llm::Message;

/// Context configuration
#[derive(Debug, Clone, Deserialize)]
pub struct ContextConfig {
    /// Maximum number of tokens to keep in context
    pub max_tokens: usize,
    /// Maximum number of messages to keep
    pub max_messages: usize,
    /// Whether to summarize old context
    pub summarize_old_context: bool,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_tokens: 100000,
            max_messages: 100,
            summarize_old_context: false,
        }
    }
}

/// Conversation context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationContext {
    pub id: String,
    pub messages: VecDeque<Message>,
    pub metadata: serde_json::Value,
    pub estimated_tokens: usize,
}

impl ConversationContext {
    /// Create a new conversation context
    pub fn new(id: String) -> Self {
        Self {
            id,
            messages: VecDeque::new(),
            metadata: serde_json::json!({}),
            estimated_tokens: 0,
        }
    }

    /// Add a message to the context
    pub fn add_message(&mut self, message: Message) {
        // Rough token estimation: ~4 characters per token
        let token_estimate = message.content.len() / 4;
        self.estimated_tokens += token_estimate;
        self.messages.push_back(message);
    }

    /// Get all messages
    pub fn get_messages(&self) -> Vec<Message> {
        self.messages.iter().cloned().collect()
    }

    /// Get the message count
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Clear all messages
    pub fn clear(&mut self) {
        self.messages.clear();
        self.estimated_tokens = 0;
    }
}

/// Context Manager
///
/// Manages conversation contexts with support for context window optimization.
pub struct ContextManager {
    config: ContextConfig,
}

impl ContextManager {
    /// Create a new context manager
    pub fn new(config: ContextConfig) -> Self {
        Self { config }
    }

    /// Create a new conversation context
    pub fn create_context(&self, id: String) -> ConversationContext {
        ConversationContext::new(id)
    }

    /// Optimize context to fit within limits
    pub fn optimize_context(&self, context: &mut ConversationContext) {
        // Remove oldest messages if over the limit
        while context.messages.len() > self.config.max_messages {
            if let Some(removed) = context.messages.pop_front() {
                let token_estimate = removed.content.len() / 4;
                context.estimated_tokens = context.estimated_tokens.saturating_sub(token_estimate);
            }
        }

        // Remove messages if over token limit
        while context.estimated_tokens > self.config.max_tokens && !context.messages.is_empty() {
            if let Some(removed) = context.messages.pop_front() {
                let token_estimate = removed.content.len() / 4;
                context.estimated_tokens = context.estimated_tokens.saturating_sub(token_estimate);
            }
        }
    }

    /// Check if context needs optimization
    pub fn needs_optimization(&self, context: &ConversationContext) -> bool {
        context.messages.len() > self.config.max_messages
            || context.estimated_tokens > self.config.max_tokens
    }

    /// Get context summary for logging
    pub fn get_summary(&self, context: &ConversationContext) -> ContextSummary {
        ContextSummary {
            id: context.id.clone(),
            message_count: context.messages.len(),
            estimated_tokens: context.estimated_tokens,
            max_messages: self.config.max_messages,
            max_tokens: self.config.max_tokens,
        }
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new(ContextConfig::default())
    }
}

/// Context summary for logging/debugging
#[derive(Debug, Clone, Serialize)]
pub struct ContextSummary {
    pub id: String,
    pub message_count: usize,
    pub estimated_tokens: usize,
    pub max_messages: usize,
    pub max_tokens: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_message() {
        let mut context = ConversationContext::new("test".to_string());

        context.add_message(Message::text("user", "Hello, world!"));

        assert_eq!(context.message_count(), 1);
        assert!(context.estimated_tokens > 0);
    }

    #[test]
    fn test_optimize_context_message_limit() {
        let config = ContextConfig {
            max_tokens: 1000000,
            max_messages: 2,
            summarize_old_context: false,
        };
        let manager = ContextManager::new(config);
        let mut context = ConversationContext::new("test".to_string());

        // Add 5 messages
        for i in 0..5 {
            context.add_message(Message::text("user", format!("Message {}", i)));
        }

        assert_eq!(context.message_count(), 5);

        manager.optimize_context(&mut context);

        assert_eq!(context.message_count(), 2);
    }

    #[test]
    fn test_needs_optimization() {
        let config = ContextConfig {
            max_tokens: 100,
            max_messages: 2,
            summarize_old_context: false,
        };
        let manager = ContextManager::new(config);
        let mut context = ConversationContext::new("test".to_string());

        // Add messages
        context.add_message(Message::text("user", "Short"));

        assert!(!manager.needs_optimization(&context));

        // Add more messages to exceed limit
        context.add_message(Message::text("user", "Another message"));
        context.add_message(Message::text("user", "Third message"));

        assert!(manager.needs_optimization(&context));
    }
}
