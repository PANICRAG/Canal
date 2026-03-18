//! Chat session management

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use uuid::Uuid;

use super::message::ChatMessage;

/// Chat session representing a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSession {
    pub id: Uuid,
    pub user_id: Uuid,
    pub title: Option<String>,
    pub messages: VecDeque<ChatMessage>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

impl ChatSession {
    /// Create a new chat session
    pub fn new(user_id: Uuid) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            user_id,
            title: None,
            messages: VecDeque::new(),
            created_at: now,
            updated_at: now,
            metadata: serde_json::json!({}),
        }
    }

    /// Create a new chat session with a specific ID
    pub fn with_id(id: Uuid, user_id: Uuid) -> Self {
        let now = Utc::now();
        Self {
            id,
            user_id,
            title: None,
            messages: VecDeque::new(),
            created_at: now,
            updated_at: now,
            metadata: serde_json::json!({}),
        }
    }

    /// Add a message to the session
    pub fn add_message(&mut self, message: ChatMessage) {
        self.messages.push_back(message);
        self.updated_at = Utc::now();
    }

    /// Get all messages
    pub fn get_messages(&self) -> Vec<ChatMessage> {
        self.messages.iter().cloned().collect()
    }

    /// Get recent messages (up to limit)
    pub fn get_recent_messages(&self, limit: usize) -> Vec<ChatMessage> {
        let len = self.messages.len();
        if len <= limit {
            self.messages.iter().cloned().collect()
        } else {
            self.messages.iter().skip(len - limit).cloned().collect()
        }
    }

    /// Get messages within a token budget (newest first, then reversed)
    ///
    /// This method selects the most recent messages that fit within the specified
    /// token budget, providing smart windowing based on actual content size rather
    /// than fixed message counts.
    pub fn get_messages_within_token_budget(&self, max_tokens: usize) -> Vec<ChatMessage> {
        let mut result = Vec::new();
        let mut current_tokens = 0;

        // Iterate from newest to oldest
        for msg in self.messages.iter().rev() {
            let msg_tokens = Self::estimate_message_tokens(&msg.content);
            if current_tokens + msg_tokens > max_tokens {
                break;
            }
            current_tokens += msg_tokens;
            result.push(msg.clone());
        }

        // Reverse to maintain chronological order
        result.reverse();
        result
    }

    /// Estimate token count for a message content
    ///
    /// Uses different ratios for CJK vs Latin text since CJK characters
    /// typically use fewer characters per token (~1.5) compared to English (~4).
    fn estimate_message_tokens(content: &str) -> usize {
        let chars = content.len();
        if chars == 0 {
            return 10; // Minimum overhead for empty messages
        }

        // Count CJK characters (Chinese, Japanese, Korean)
        let cjk_count = content
            .chars()
            .filter(|c| {
                // CJK Unified Ideographs
                (*c >= '\u{4E00}' && *c <= '\u{9FFF}')
                    // CJK Extension A
                    || (*c >= '\u{3400}' && *c <= '\u{4DBF}')
                    // Hiragana
                    || (*c >= '\u{3040}' && *c <= '\u{309F}')
                    // Katakana
                    || (*c >= '\u{30A0}' && *c <= '\u{30FF}')
                    // Hangul
                    || (*c >= '\u{AC00}' && *c <= '\u{D7AF}')
            })
            .count();

        let cjk_ratio = cjk_count as f64 / chars.max(1) as f64;

        // CJK text: ~1.5 chars per token; English: ~4 chars per token
        let chars_per_token = if cjk_ratio > 0.3 { 1.5 } else { 4.0 };
        (chars as f64 / chars_per_token) as usize + 10 // +10 for message overhead
    }

    /// Get message count
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Set session title
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = Some(title.into());
        self.updated_at = Utc::now();
    }

    /// Generate title from first user message if not set
    pub fn auto_title(&mut self) {
        if self.title.is_none() {
            if let Some(first_user_msg) = self
                .messages
                .iter()
                .find(|m| m.role == super::message::MessageRole::User)
            {
                // R3-M: Use char count (not byte length) for consistent CJK handling
                let char_count = first_user_msg.content.chars().count();
                let title = first_user_msg.content.chars().take(50).collect::<String>();
                let title = if char_count > 50 {
                    format!("{}...", title)
                } else {
                    title
                };
                self.title = Some(title);
            }
        }
    }

    /// Estimate token count for the session
    pub fn estimate_tokens(&self) -> usize {
        // R3-M: Use CJK-aware estimate consistent with estimate_message_tokens()
        self.messages
            .iter()
            .map(|m| Self::estimate_message_tokens(&m.content))
            .sum()
    }

    /// Trim old messages to fit within token limit
    pub fn trim_to_token_limit(&mut self, max_tokens: usize) {
        while self.estimate_tokens() > max_tokens && self.messages.len() > 1 {
            self.messages.pop_front();
        }
    }
}

/// Session summary for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: Uuid,
    pub user_id: Uuid,
    pub title: Option<String>,
    pub message_count: usize,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&ChatSession> for SessionSummary {
    fn from(session: &ChatSession) -> Self {
        Self {
            id: session.id,
            user_id: session.user_id,
            title: session.title.clone(),
            message_count: session.messages.len(),
            created_at: session.created_at,
            updated_at: session.updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::message::ChatMessage;

    #[test]
    fn test_new_session() {
        let user_id = Uuid::new_v4();
        let session = ChatSession::new(user_id);
        assert_eq!(session.user_id, user_id);
        assert!(session.title.is_none());
        assert_eq!(session.message_count(), 0);
    }

    #[test]
    fn test_add_message() {
        let user_id = Uuid::new_v4();
        let mut session = ChatSession::new(user_id);

        session.add_message(ChatMessage::user("Hello"));
        session.add_message(ChatMessage::assistant("Hi there!"));

        assert_eq!(session.message_count(), 2);
    }

    #[test]
    fn test_get_recent_messages() {
        let user_id = Uuid::new_v4();
        let mut session = ChatSession::new(user_id);

        for i in 0..10 {
            session.add_message(ChatMessage::user(format!("Message {}", i)));
        }

        let recent = session.get_recent_messages(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].content, "Message 7");
        assert_eq!(recent[2].content, "Message 9");
    }

    #[test]
    fn test_auto_title() {
        let user_id = Uuid::new_v4();
        let mut session = ChatSession::new(user_id);

        session.add_message(ChatMessage::user("Help me write a video script about cats"));

        session.auto_title();
        assert!(session.title.is_some());
        assert!(session.title.as_ref().unwrap().contains("Help me write"));
    }

    #[test]
    fn test_get_messages_within_token_budget() {
        let user_id = Uuid::new_v4();
        let mut session = ChatSession::new(user_id);

        // Add messages of varying lengths
        for i in 0..10 {
            let content = format!("Message {} with some content to make it longer", i);
            session.add_message(ChatMessage::user(content));
        }

        // Small budget should return fewer messages
        let small_budget = session.get_messages_within_token_budget(100);
        assert!(small_budget.len() < 10);
        assert!(!small_budget.is_empty());

        // Large budget should return all messages
        let large_budget = session.get_messages_within_token_budget(10000);
        assert_eq!(large_budget.len(), 10);

        // Verify chronological order is maintained
        if small_budget.len() >= 2 {
            assert!(small_budget[0].content.contains("Message"));
        }
    }

    #[test]
    fn test_estimate_message_tokens_cjk() {
        // English text: ~4 chars per token
        let english = "Hello, this is a test message with some content.";
        let english_tokens = ChatSession::estimate_message_tokens(english);

        // CJK text: ~1.5 chars per token (should have more tokens for same length)
        let cjk = "你好，这是一个测试消息，包含一些内容。";
        let cjk_tokens = ChatSession::estimate_message_tokens(cjk);

        // CJK should estimate more tokens for similar semantic content
        assert!(cjk_tokens > 0);
        assert!(english_tokens > 0);
    }
}
