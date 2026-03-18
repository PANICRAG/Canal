//! LLM Router Adapter
//!
//! Bridges the agent's `LlmClient` trait with the `LlmRouter` from the llm module.

use std::sync::Arc;

use async_trait::async_trait;

use crate::agent::r#loop::{AgentError, LlmClient, LlmResponse, StopReason};
use crate::agent::types::{
    AgentMessage, AssistantMessage, ContentBlock as AgentContentBlock, MessageContent,
    ToolResultContent, Usage as AgentUsage, UserMessage,
};
use crate::llm::router::{
    ChatRequest, ChatResponse, ContentBlock as LlmContentBlock, LlmRouter, Message,
    StopReason as LlmStopReason, ToolDefinition,
};

/// Adapter that implements `LlmClient` trait using `LlmRouter`
pub struct LlmRouterAdapter {
    router: Arc<LlmRouter>,
    model: Option<String>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    /// Profile ID for model routing (dynamic routing)
    profile_id: Option<String>,
    /// Task type hint for task-based routing
    task_type: Option<String>,
}

impl LlmRouterAdapter {
    /// Create a new adapter wrapping an `LlmRouter`
    pub fn new(router: Arc<LlmRouter>) -> Self {
        Self {
            router,
            model: None,
            max_tokens: None,
            temperature: None,
            profile_id: None,
            task_type: None,
        }
    }

    /// Set the model to use
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set max tokens
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set temperature
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Set profile ID for dynamic routing
    pub fn with_profile_id(mut self, profile_id: impl Into<String>) -> Self {
        self.profile_id = Some(profile_id.into());
        self
    }

    /// Set task type hint for task-based routing
    pub fn with_task_type(mut self, task_type: impl Into<String>) -> Self {
        self.task_type = Some(task_type.into());
        self
    }

    /// Get current profile ID
    pub fn profile_id(&self) -> Option<&str> {
        self.profile_id.as_deref()
    }

    /// Set profile ID (mutable reference version)
    pub fn set_profile_id(&mut self, profile_id: Option<String>) {
        self.profile_id = profile_id;
    }

    /// Set task type (mutable reference version)
    pub fn set_task_type(&mut self, task_type: Option<String>) {
        self.task_type = task_type;
    }

    /// Convert `AgentMessage` list to `ChatRequest`
    fn convert_messages_to_request(
        &self,
        messages: Vec<AgentMessage>,
        tools: Vec<serde_json::Value>,
    ) -> ChatRequest {
        let mut request_messages = Vec::new();

        for msg in messages {
            match msg {
                AgentMessage::User(user_msg) => {
                    let message = Self::convert_user_message(user_msg);
                    request_messages.push(message);
                }
                AgentMessage::Assistant(assistant_msg) => {
                    let message = Self::convert_assistant_message(assistant_msg);
                    request_messages.push(message);
                }
                AgentMessage::System(system_msg) => {
                    // Extract text from system message data
                    let text = match &system_msg.data {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    // Convert to system message (Anthropic provider handles role="system")
                    let message = Message::text("system", text);
                    request_messages.push(message);
                }
                AgentMessage::Result(_)
                | AgentMessage::StreamEvent(_)
                | AgentMessage::PermissionRequest(_) => {
                    // These are output-only message types, skip them
                }
            }
        }

        // Convert tool definitions from JSON
        let tool_definitions: Vec<ToolDefinition> = tools
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();

        ChatRequest {
            messages: request_messages,
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            stream: false,
            tools: tool_definitions,
            tool_choice: None,
            profile_id: self.profile_id.clone(),
            task_type: self.task_type.clone(),
            thinking_budget: None,
        }
    }

    /// Convert `UserMessage` to `Message`
    fn convert_user_message(user_msg: UserMessage) -> Message {
        match user_msg.content {
            MessageContent::Text(text) => Message::text("user", text),
            MessageContent::Blocks(blocks) => {
                let content_blocks: Vec<LlmContentBlock> = blocks
                    .into_iter()
                    .flat_map(|b| Self::convert_agent_content_block_to_llm(b))
                    .collect();

                if content_blocks.is_empty() {
                    Message::text("user", "")
                } else {
                    Message::with_blocks("user", content_blocks)
                }
            }
        }
    }

    /// Convert `AssistantMessage` to `Message`
    fn convert_assistant_message(assistant_msg: AssistantMessage) -> Message {
        let content_blocks: Vec<LlmContentBlock> = assistant_msg
            .content
            .into_iter()
            .flat_map(|b| Self::convert_agent_content_block_to_llm(b))
            .collect();

        if content_blocks.is_empty() {
            Message::text("assistant", "")
        } else {
            Message::with_blocks("assistant", content_blocks)
        }
    }

    /// Convert agent `ContentBlock` to llm `ContentBlock`(s).
    ///
    /// Returns a `Vec` because a single agent block (e.g. ToolResult with image)
    /// may expand into multiple LLM blocks (ToolResult text + Image).
    fn convert_agent_content_block_to_llm(block: AgentContentBlock) -> Vec<LlmContentBlock> {
        match block {
            AgentContentBlock::Text { text } => vec![LlmContentBlock::Text { text }],
            AgentContentBlock::ToolUse { id, name, input } => {
                vec![LlmContentBlock::ToolUse { id, name, input }]
            }
            AgentContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let mut blocks = Vec::new();

                match content {
                    Some(ToolResultContent::Blocks(ref result_blocks)) => {
                        // Extract text parts for the ToolResult content string
                        let text_parts: String = result_blocks
                            .iter()
                            .filter_map(|b| match b {
                                crate::agent::types::ToolResultBlock::Text { text } => {
                                    Some(text.as_str())
                                }
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("");

                        blocks.push(LlmContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: text_parts,
                            is_error: is_error.unwrap_or(false),
                        });

                        // Extract image blocks and emit them as separate Image ContentBlocks
                        for result_block in result_blocks {
                            if let crate::agent::types::ToolResultBlock::Image { source } =
                                result_block
                            {
                                if let crate::agent::types::ImageSource::Base64 {
                                    media_type,
                                    data,
                                } = source
                                {
                                    blocks.push(LlmContentBlock::Image {
                                        source_type: "base64".to_string(),
                                        media_type: media_type.clone(),
                                        data: data.clone(),
                                    });
                                }
                            }
                        }
                    }
                    Some(ToolResultContent::Text(text)) => {
                        blocks.push(LlmContentBlock::ToolResult {
                            tool_use_id,
                            content: text,
                            is_error: is_error.unwrap_or(false),
                        });
                    }
                    None => {
                        blocks.push(LlmContentBlock::ToolResult {
                            tool_use_id,
                            content: String::new(),
                            is_error: is_error.unwrap_or(false),
                        });
                    }
                }

                blocks
            }
            // Thinking blocks: preserve as native Thinking content blocks
            AgentContentBlock::Thinking { thinking, .. } => {
                vec![LlmContentBlock::Thinking { thinking }]
            }
            AgentContentBlock::Image { source, .. } => {
                // Pass through Image blocks for multimodal models
                if let crate::agent::types::ImageSource::Base64 { media_type, data } = source {
                    vec![LlmContentBlock::Image {
                        source_type: "base64".to_string(),
                        media_type,
                        data,
                    }]
                } else {
                    vec![]
                }
            }
            AgentContentBlock::Document { .. } => {
                // Documents don't have a direct LLM equivalent
                vec![]
            }
        }
    }

    /// Convert `ChatResponse` to `LlmResponse`
    fn convert_response_to_llm_response(response: ChatResponse) -> LlmResponse {
        let choice = response.choices.first();

        let content_blocks: Vec<AgentContentBlock> = choice
            .map(|c| {
                // First try content_blocks, then fall back to content string
                if !c.message.content_blocks.is_empty() {
                    c.message
                        .content_blocks
                        .iter()
                        .filter_map(|b| Self::convert_llm_content_block_to_agent(b.clone()))
                        .collect()
                } else if !c.message.content.is_empty() {
                    vec![AgentContentBlock::Text {
                        text: c.message.content.clone(),
                    }]
                } else {
                    vec![]
                }
            })
            .unwrap_or_default();

        let stop_reason = choice
            .and_then(|c| c.stop_reason.clone())
            .map(Self::convert_stop_reason)
            .unwrap_or(StopReason::EndTurn);

        // R1-M: Use saturating cast to prevent silent truncation on large token counts
        let usage = AgentUsage {
            input_tokens: u32::try_from(response.usage.prompt_tokens).unwrap_or(u32::MAX),
            output_tokens: u32::try_from(response.usage.completion_tokens).unwrap_or(u32::MAX),
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };

        LlmResponse {
            content: content_blocks,
            model: response.model,
            usage,
            stop_reason,
        }
    }

    /// Convert llm `ContentBlock` to agent `ContentBlock`
    fn convert_llm_content_block_to_agent(block: LlmContentBlock) -> Option<AgentContentBlock> {
        match block {
            LlmContentBlock::Text { text } => Some(AgentContentBlock::Text { text }),
            LlmContentBlock::ToolUse { id, name, input } => {
                Some(AgentContentBlock::ToolUse { id, name, input })
            }
            LlmContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some(AgentContentBlock::ToolResult {
                tool_use_id,
                content: Some(ToolResultContent::Text(content)),
                is_error: Some(is_error),
            }),
            LlmContentBlock::Thinking { thinking } => Some(AgentContentBlock::Thinking {
                thinking,
                signature: None,
            }),
            LlmContentBlock::Image { .. } => {
                tracing::debug!("Skipping Image content block in agent adapter (vision preprocessing handles this internally)");
                None
            }
        }
    }

    /// Convert llm `StopReason` to agent `StopReason`
    fn convert_stop_reason(reason: LlmStopReason) -> StopReason {
        match reason {
            LlmStopReason::EndTurn => StopReason::EndTurn,
            LlmStopReason::MaxTokens => StopReason::MaxTokens,
            LlmStopReason::ToolUse => StopReason::ToolUse,
            LlmStopReason::StopSequence => StopReason::StopSequence,
        }
    }
}

#[async_trait]
impl LlmClient for LlmRouterAdapter {
    async fn generate(
        &self,
        messages: Vec<AgentMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, AgentError> {
        let request = self.convert_messages_to_request(messages, tools);

        let response = self
            .router
            .route(request)
            .await
            .map_err(|e| AgentError::ApiError(e.to_string()))?;

        Ok(Self::convert_response_to_llm_response(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::router::{Choice, Usage as LlmUsage};

    #[test]
    fn test_convert_user_message_text() {
        let user_msg = UserMessage {
            content: MessageContent::Text("Hello, world!".to_string()),
            uuid: None,
            parent_tool_use_id: None,
            tool_use_result: None,
        };

        let message = LlmRouterAdapter::convert_user_message(user_msg);
        assert_eq!(message.role, "user");
        assert_eq!(message.content, "Hello, world!");
    }

    #[test]
    fn test_convert_assistant_message_with_tool_use() {
        let assistant_msg = AssistantMessage {
            content: vec![
                AgentContentBlock::Text {
                    text: "I'll read the file.".to_string(),
                },
                AgentContentBlock::ToolUse {
                    id: "tool_123".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "/tmp/test.txt"}),
                },
            ],
            model: "claude-3".to_string(),
            parent_tool_use_id: None,
            error: None,
        };

        let message = LlmRouterAdapter::convert_assistant_message(assistant_msg);
        assert_eq!(message.role, "assistant");
        assert_eq!(message.content_blocks.len(), 2);
        assert!(message.has_tool_use());
    }

    #[test]
    fn test_convert_stop_reason() {
        assert_eq!(
            LlmRouterAdapter::convert_stop_reason(LlmStopReason::EndTurn),
            StopReason::EndTurn
        );
        assert_eq!(
            LlmRouterAdapter::convert_stop_reason(LlmStopReason::ToolUse),
            StopReason::ToolUse
        );
        assert_eq!(
            LlmRouterAdapter::convert_stop_reason(LlmStopReason::MaxTokens),
            StopReason::MaxTokens
        );
    }

    #[test]
    fn test_convert_chat_response() {
        let response = ChatResponse {
            id: "resp_123".to_string(),
            model: "claude-3-opus".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message::text("assistant", "Hello!"),
                finish_reason: "end_turn".to_string(),
                stop_reason: Some(LlmStopReason::EndTurn),
            }],
            usage: LlmUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        };

        let llm_response = LlmRouterAdapter::convert_response_to_llm_response(response);
        assert_eq!(llm_response.model, "claude-3-opus");
        assert_eq!(llm_response.stop_reason, StopReason::EndTurn);
        assert_eq!(llm_response.usage.input_tokens, 10);
        assert_eq!(llm_response.usage.output_tokens, 5);
    }

    #[test]
    fn test_convert_tool_result_content_block() {
        let agent_block = AgentContentBlock::ToolResult {
            tool_use_id: "tool_456".to_string(),
            content: Some(ToolResultContent::Text("File contents here".to_string())),
            is_error: Some(false),
        };

        let llm_blocks = LlmRouterAdapter::convert_agent_content_block_to_llm(agent_block);
        assert_eq!(llm_blocks.len(), 1);

        if let LlmContentBlock::ToolResult {
            ref tool_use_id,
            ref content,
            is_error,
        } = llm_blocks[0]
        {
            assert_eq!(tool_use_id, "tool_456");
            assert_eq!(content, "File contents here");
            assert!(!is_error);
        } else {
            panic!("Expected ToolResult content block");
        }
    }
}
