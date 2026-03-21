//! Intent recognition for user messages

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::error::Result;
use crate::llm::{ChatRequest, LlmRouter, Message};

/// Recognized intent from user message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    pub intent_type: IntentType,
    pub task_type: Option<TaskType>,
    pub confidence: f32,
    pub entities: serde_json::Value,
}

/// Intent types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IntentType {
    /// Simple conversation, greeting, chitchat
    SimpleChat,
    /// Task that requires planning and execution
    Task,
    /// Need more information to understand
    Clarification,
    /// System command (settings, help, etc.)
    SystemCommand,
}

/// Task types for complex operations
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    /// Create content (scripts, documents, etc.)
    CreateContent,
    /// Publish content to platforms
    Publish,
    /// Analyze data, generate reports
    Analyze,
    /// Search and retrieve information
    Search,
    /// Manage operations (edit, delete, organize)
    Manage,
    /// Generate media (video, audio, images)
    GenerateMedia,
    /// Workflow execution
    ExecuteWorkflow,
}

/// Intent recognizer using LLM
pub struct IntentRecognizer {
    llm_router: Arc<LlmRouter>,
}

impl IntentRecognizer {
    /// Create a new intent recognizer
    pub fn new(llm_router: Arc<LlmRouter>) -> Self {
        Self { llm_router }
    }

    /// Recognize intent from user message
    pub async fn recognize(&self, message: &str, context: Option<&str>) -> Result<Intent> {
        let context_str = context.unwrap_or("No previous context");

        let prompt = format!(
            r#"Analyze the user's message and determine the intent.

User message: "{message}"

Previous context: {context_str}

Intent types:
1. simple_chat - Simple conversation, greeting, question that can be answered directly
2. task - Complex operation requiring multiple steps or tool usage
3. clarification - Message is unclear, need more information
4. system_command - Settings, help, or system-related request

If intent is "task", also identify the task type:
- create_content: Create scripts, documents, text content
- publish: Publish or share content to platforms
- analyze: Analyze data, generate reports
- search: Search for information
- manage: Edit, delete, organize items
- generate_media: Create video, audio, images
- execute_workflow: Run a predefined workflow

Return JSON:
{{
    "intent_type": "simple_chat|task|clarification|system_command",
    "task_type": "create_content|publish|analyze|search|manage|generate_media|execute_workflow|null",
    "confidence": 0.0-1.0,
    "entities": {{}}
}}

Only return valid JSON, no other text."#
        );

        let request = ChatRequest {
            messages: vec![Message::text("user", prompt)],
            model: None,
            max_tokens: Some(500),
            temperature: Some(0.0),
            stream: false,
            ..Default::default()
        };

        let response = self.llm_router.route(request).await?;

        let content = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        // Parse JSON response
        let intent: Intent = serde_json::from_str(&content).unwrap_or_else(|e| {
            // R1-M3: Log parse failure instead of silently defaulting
            tracing::warn!(error = %e, "Failed to parse intent classification from LLM, defaulting to SimpleChat");
            Intent {
                intent_type: IntentType::SimpleChat,
                task_type: None,
                confidence: 0.1, // Low confidence to indicate parse failure
                entities: serde_json::json!({}),
            }
        });

        Ok(intent)
    }

    /// Quick intent check without full analysis
    pub fn quick_check(&self, message: &str) -> IntentType {
        let message_lower = message.to_lowercase();

        // Simple greetings
        if message_lower.starts_with("hi")
            || message_lower.starts_with("hello")
            || message_lower.starts_with("hey")
            || message_lower == "thanks"
            || message_lower == "thank you"
        {
            return IntentType::SimpleChat;
        }

        // Task keywords
        let task_keywords = [
            "create",
            "generate",
            "write",
            "make",
            "build",
            "publish",
            "post",
            "share",
            "upload",
            "analyze",
            "report",
            "summarize",
            "search",
            "find",
            "look for",
            "delete",
            "remove",
            "edit",
            "update",
            "modify",
            "organize",
            "video",
            "script",
            "content",
        ];

        for keyword in task_keywords {
            if message_lower.contains(keyword) {
                return IntentType::Task;
            }
        }

        // Questions that might need clarification
        if message.trim().len() < 10 || (message.ends_with('?') && message.len() < 30) {
            return IntentType::SimpleChat;
        }

        // Default to task for longer messages
        if message.len() > 100 {
            return IntentType::Task;
        }

        IntentType::SimpleChat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quick_check_greeting() {
        let llm_router = Arc::new(LlmRouter::new(crate::llm::LlmConfig::default()));
        let recognizer = IntentRecognizer::new(llm_router);

        assert_eq!(recognizer.quick_check("Hello"), IntentType::SimpleChat);
        assert_eq!(recognizer.quick_check("Hi there!"), IntentType::SimpleChat);
    }

    #[test]
    fn test_quick_check_task() {
        let llm_router = Arc::new(LlmRouter::new(crate::llm::LlmConfig::default()));
        let recognizer = IntentRecognizer::new(llm_router);

        assert_eq!(
            recognizer.quick_check("Create a video script about cats"),
            IntentType::Task
        );
        assert_eq!(
            recognizer.quick_check("Publish this to TikTok"),
            IntentType::Task
        );
        assert_eq!(
            recognizer.quick_check("Analyze my video performance"),
            IntentType::Task
        );
    }
}
