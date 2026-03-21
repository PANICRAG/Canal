//! LLM Router implementation
//!
//! The LLM Router provides intelligent routing of chat requests to multiple
//! LLM providers with automatic fallback and retry capabilities.

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use super::routing_engine::RoutingEngine;
use crate::error::{Error, Result};

/// Stream chunk from LLM (for streaming responses)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StreamChunk {
    /// Text delta
    TextDelta { text: String },
    /// Thinking delta (extended thinking streaming)
    ThinkingDelta { text: String },
    /// Tool use start
    ToolUseStart { id: String, name: String },
    /// Tool use input delta (JSON fragment)
    ToolUseInputDelta { id: String, input_delta: String },
    /// Tool use complete
    ToolUseComplete {
        id: String,
        input: serde_json::Value,
    },
    /// Message complete with usage
    Done {
        usage: Usage,
        stop_reason: Option<StopReason>,
    },
    /// Error
    Error { message: String },
}

/// Type alias for streaming response
pub type StreamResponse = Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>;

// ============================================================================
// Tool Use Types (Claude API compatible)
// ============================================================================

/// Tool definition for LLM (Claude API format)
///
/// Represents a tool that can be called by the LLM during a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool name (e.g., "filesystem_read_file")
    pub name: String,
    /// Human-readable description of what the tool does
    pub description: String,
    /// JSON Schema defining the tool's input parameters
    pub input_schema: serde_json::Value,
}

/// Tool use request from LLM
///
/// When the LLM wants to call a tool, it returns this in the response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUse {
    /// Unique ID for this tool call (used to match with tool_result)
    pub id: String,
    /// Name of the tool to call
    pub name: String,
    /// Input parameters for the tool
    pub input: serde_json::Value,
}

/// Tool result to send back to LLM
///
/// After executing a tool, send this back to the LLM to continue the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// ID of the tool_use this is responding to
    pub tool_use_id: String,
    /// Result content (typically JSON stringified)
    pub content: String,
    /// Whether the tool execution resulted in an error
    #[serde(default)]
    pub is_error: bool,
}

/// Content block in a message (Claude API format)
///
/// Messages can contain multiple content blocks of different types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Plain text content
    #[serde(rename = "text")]
    Text { text: String },

    /// Tool use request from assistant
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    /// Tool result from user (response to tool_use)
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },

    /// Image content (for vision/multimodal requests)
    #[serde(rename = "image")]
    Image {
        source_type: String, // "base64"
        media_type: String,  // "image/jpeg"
        data: String,        // base64 encoded image data
    },

    /// Thinking/reasoning content from extended thinking models
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
}

/// Tool choice configuration
///
/// Controls how the LLM decides whether to use tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolChoice {
    /// Let the model decide whether to use tools
    #[serde(rename = "auto")]
    Auto,

    /// Model should use at least one tool
    #[serde(rename = "any")]
    Any,

    /// Force the model to use a specific tool
    #[serde(rename = "tool")]
    Tool { name: String },

    /// Disable tool use entirely
    #[serde(rename = "none")]
    None,
}

impl Default for ToolChoice {
    fn default() -> Self {
        ToolChoice::Auto
    }
}

/// Stop reason from LLM response
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Normal completion
    EndTurn,
    /// Hit max tokens limit
    MaxTokens,
    /// Model wants to use a tool
    ToolUse,
    /// Stopped due to stop sequence
    StopSequence,
}

// ============================================================================
// Message Types
// ============================================================================

/// Chat message
///
/// Represents a single message in the conversation.
/// For simple text-only messages, use `content` field.
/// For messages with tool calls, use `content_blocks` field.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Message {
    /// Role of the message sender ("user", "assistant", "system")
    #[serde(default)]
    pub role: String,
    /// Simple text content (for backward compatibility)
    #[serde(default)]
    pub content: String,
    /// Structured content blocks (for tool_use messages)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_blocks: Vec<ContentBlock>,
}

impl Message {
    /// Create a simple text message
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            content_blocks: Vec::new(),
        }
    }

    /// Create a message with content blocks
    pub fn with_blocks(role: impl Into<String>, blocks: Vec<ContentBlock>) -> Self {
        // Extract text content from blocks for backward compatibility
        let content = blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                ContentBlock::Thinking { thinking } => {
                    Some(format!("<thinking>{}</thinking>", thinking))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");

        Self {
            role: role.into(),
            content,
            content_blocks: blocks,
        }
    }

    /// Check if this message contains tool use requests
    pub fn has_tool_use(&self) -> bool {
        self.content_blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
    }

    /// Extract tool use requests from this message
    pub fn get_tool_uses(&self) -> Vec<ToolUse> {
        self.content_blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => Some(ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }),
                _ => None,
            })
            .collect()
    }
}

// ============================================================================
// Request/Response Types
// ============================================================================

/// Chat request
///
/// Request to send to an LLM provider for chat completion.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChatRequest {
    /// Conversation messages
    #[serde(default)]
    pub messages: Vec<Message>,
    /// Model to use (provider-specific)
    #[serde(default)]
    pub model: Option<String>,
    /// Maximum tokens to generate
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Sampling temperature (0.0 - 1.0)
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Whether to stream the response
    #[serde(default)]
    pub stream: bool,
    /// Available tools for the LLM to call
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    /// How the LLM should choose to use tools
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Profile ID for model routing (if None, use default profile)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// Task type hint for task-based routing (e.g., "code", "analysis", "chat")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_type: Option<String>,
    /// Budget tokens for extended thinking (Anthropic extended thinking, Qwen reasoning)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
}

impl ChatRequest {
    /// Builder-style setter for thinking budget
    pub fn with_thinking_budget(mut self, budget: u32) -> Self {
        self.thinking_budget = Some(budget);
        self
    }
}

/// Chat response choice
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Choice {
    #[serde(default)]
    pub index: i32,
    #[serde(default)]
    pub message: Message,
    #[serde(default)]
    pub finish_reason: String,
    /// Structured stop reason (for tool_use detection)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
}

/// Token usage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

/// Chat response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

impl ChatResponse {
    /// Check if the response requires tool execution
    pub fn requires_tool_use(&self) -> bool {
        self.choices
            .iter()
            .any(|c| c.stop_reason == Some(StopReason::ToolUse) || c.message.has_tool_use())
    }

    /// Get all tool use requests from the response
    pub fn get_tool_uses(&self) -> Vec<ToolUse> {
        self.choices
            .iter()
            .flat_map(|c| c.message.get_tool_uses())
            .collect()
    }
}

/// LLM Provider trait
///
/// Implement this trait to add support for new LLM providers.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a chat request to the provider
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse>;

    /// Send a streaming chat request to the provider
    async fn chat_stream(&self, request: ChatRequest) -> Result<StreamResponse> {
        // Default implementation: fall back to non-streaming
        let response = self.chat(request).await?;
        let text = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();
        let usage = response.usage.clone();
        let stop_reason = response.choices.first().and_then(|c| c.stop_reason.clone());

        let stream = futures::stream::iter(vec![
            Ok(StreamChunk::TextDelta { text }),
            Ok(StreamChunk::Done { usage, stop_reason }),
        ]);

        Ok(Box::pin(stream))
    }

    /// Get the provider name
    fn name(&self) -> &str;

    /// Check if the provider is available
    async fn is_available(&self) -> bool;
}

/// LLM Router configuration
#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    pub default_provider: String,
    pub fallback_enabled: bool,
    pub timeout_seconds: u64,
    pub max_retries: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            default_provider: "anthropic".to_string(),
            fallback_enabled: true,
            timeout_seconds: 30,
            max_retries: 2,
        }
    }
}

/// LLM Router
///
/// Routes chat requests to the appropriate LLM provider with automatic
/// fallback and retry capabilities.
pub struct LlmRouter {
    providers: HashMap<String, Arc<dyn LlmProvider>>,
    config: LlmConfig,
    /// Optional routing engine for profile-based routing
    routing_engine: Option<Arc<RoutingEngine>>,
}

impl LlmRouter {
    /// Create a new LLM router
    pub fn new(config: LlmConfig) -> Self {
        Self {
            providers: HashMap::new(),
            config,
            routing_engine: None,
        }
    }

    /// Register a provider
    pub fn register_provider(&mut self, name: &str, provider: Arc<dyn LlmProvider>) {
        tracing::info!("Registering LLM provider: {}", name);
        self.providers.insert(name.to_string(), provider);
    }

    /// Set the default provider name
    pub fn set_default_provider(&mut self, name: &str) {
        self.config.default_provider = name.to_string();
    }

    /// Get the list of registered providers
    pub fn list_providers(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// Set the routing engine for profile-based routing
    pub fn set_routing_engine(&mut self, engine: Arc<RoutingEngine>) {
        tracing::info!("Routing engine attached to LLM router");
        self.routing_engine = Some(engine);
    }

    /// Get the routing engine if set
    pub fn routing_engine(&self) -> Option<&Arc<RoutingEngine>> {
        self.routing_engine.as_ref()
    }

    /// Get a provider by name
    pub fn get_provider(&self, name: &str) -> Option<&Arc<dyn LlmProvider>> {
        self.providers.get(name)
    }

    /// Get providers map (for routing engine integration)
    pub fn providers(&self) -> &HashMap<String, Arc<dyn LlmProvider>> {
        &self.providers
    }

    /// Route a streaming chat request to the appropriate provider
    pub async fn route_stream(&self, request: ChatRequest) -> Result<StreamResponse> {
        // Try profile-based routing first if routing engine is available and profile_id is set
        let (provider_name, model_override) = self.resolve_provider(&request).await?;

        let provider = self
            .providers
            .get(&provider_name)
            .ok_or_else(|| Error::Llm(format!("Provider not found: {}", provider_name)))?;

        // Apply model override if routing engine specified one
        let mut routed_request = request;
        if let Some(model) = model_override {
            routed_request.model = Some(model);
        }

        tracing::info!(
            provider = provider_name,
            model = ?routed_request.model,
            "Routing streaming request"
        );

        provider.chat_stream(routed_request).await
    }

    /// Resolve which provider and model to use for the request
    async fn resolve_provider(&self, request: &ChatRequest) -> Result<(String, Option<String>)> {
        // Check if we should use profile-based routing
        if let Some(ref engine) = self.routing_engine {
            if request.profile_id.is_some() || request.task_type.is_some() {
                let profile_id = request.profile_id.as_deref().unwrap_or("default");

                match engine.route_with_profile(profile_id, request).await {
                    Ok(decision) => {
                        tracing::info!(
                            profile = profile_id,
                            provider = %decision.target.provider,
                            model = %decision.target.model,
                            reason = %decision.reason,
                            "Profile-based routing decision"
                        );
                        return Ok((decision.target.provider, Some(decision.target.model)));
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            profile = profile_id,
                            "Profile-based routing failed, falling back to default"
                        );
                        // Fall through to default routing
                    }
                }
            }
        }

        // Model-prefix routing: infer provider from model name
        if let Some(ref model) = request.model {
            let provider = Self::infer_provider_from_model(model);
            if let Some(p) = provider {
                if self.providers.contains_key(p) {
                    tracing::info!(model = %model, provider = p, "Model-prefix routing");
                    return Ok((p.to_string(), None));
                }
            }
        }

        // Default routing: use configured default provider
        Ok((self.config.default_provider.clone(), None))
    }

    /// Route a chat request to the appropriate provider
    pub async fn route(&self, request: ChatRequest) -> Result<ChatResponse> {
        // Resolve provider using profile-based routing or default
        let (provider_name, model_override) = self.resolve_provider(&request).await?;

        let provider = self
            .providers
            .get(&provider_name)
            .ok_or_else(|| Error::Llm(format!("Provider not found: {}", provider_name)))?;

        // R3-H11: Use the owned request directly instead of cloning.
        // `request` is already passed by value, so the clone was redundant.
        let mut routed_request = request;
        if let Some(model) = model_override.clone() {
            routed_request.model = Some(model);
        }

        let mut last_error = None;
        let start = Instant::now();

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                tracing::warn!(
                    "Retrying request to {}, attempt {}",
                    provider_name,
                    attempt + 1
                );
                // R3-L118: Cap backoff at 30s to prevent overflow at attempt 30+
                let backoff_ms = (100 * 2u64.saturating_pow(attempt)).min(30_000);
                tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;
            }

            match provider.chat(routed_request.clone()).await {
                Ok(response) => {
                    let latency = start.elapsed();

                    // Record success to health tracker if routing engine is available
                    if let Some(ref engine) = self.routing_engine {
                        let target = super::strategies::ModelTarget {
                            provider: provider_name.clone(),
                            model: model_override
                                .clone()
                                .unwrap_or_else(|| response.model.clone()),
                        };
                        engine.record_success(&target, latency);
                    }

                    tracing::info!(
                        provider = provider_name,
                        model = response.model,
                        tokens = response.usage.total_tokens,
                        latency_ms = latency.as_millis(),
                        "Chat request completed"
                    );
                    return Ok(response);
                }
                Err(e) => {
                    // Record failure to health tracker if routing engine is available
                    if let Some(ref engine) = self.routing_engine {
                        let target = super::strategies::ModelTarget {
                            provider: provider_name.clone(),
                            model: model_override.clone().unwrap_or_default(),
                        };
                        engine.record_failure(&target);
                    }

                    tracing::error!(
                        provider = provider_name,
                        error = %e,
                        attempt = attempt + 1,
                        "Provider request failed"
                    );
                    last_error = Some(e);
                }
            }
        }

        // Try fallback providers (R3-L: sorted for deterministic order)
        if self.config.fallback_enabled {
            let mut fallback_list: Vec<_> = self
                .providers
                .iter()
                .filter(|(name, _)| *name != &provider_name)
                .collect();
            fallback_list.sort_by_key(|(name, _)| (*name).clone());
            for (name, fallback_provider) in fallback_list {
                tracing::info!("Trying fallback provider: {}", name);

                match fallback_provider.chat(routed_request.clone()).await {
                    Ok(response) => {
                        tracing::info!(
                            provider = name,
                            model = response.model,
                            "Fallback provider succeeded"
                        );
                        return Ok(response);
                    }
                    Err(e) => {
                        tracing::warn!(provider = name, error = %e, "Fallback provider failed");
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| Error::Llm("All providers failed".to_string())))
    }

    /// Route a chat request with explicit profile selection
    pub async fn route_with_profile(
        &self,
        profile_id: &str,
        request: ChatRequest,
    ) -> Result<ChatResponse> {
        let mut req = request;
        req.profile_id = Some(profile_id.to_string());
        self.route(req).await
    }

    /// Route a streaming chat request with explicit profile selection
    pub async fn route_stream_with_profile(
        &self,
        profile_id: &str,
        request: ChatRequest,
    ) -> Result<StreamResponse> {
        let mut req = request;
        req.profile_id = Some(profile_id.to_string());
        self.route_stream(req).await
    }

    /// Infer provider from model name prefix.
    fn infer_provider_from_model(model: &str) -> Option<&'static str> {
        let m = model.to_lowercase();
        if m.starts_with("claude") {
            Some("anthropic")
        } else if m.starts_with("gpt")
            || m.starts_with("o1")
            || m.starts_with("o3")
            || m.starts_with("o4")
        {
            Some("openai")
        } else if m.starts_with("gemini") {
            Some("google")
        } else if m.starts_with("qwen") {
            Some("qwen")
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider {
        name: String,
        should_fail: bool,
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse> {
            if self.should_fail {
                Err(Error::Llm("Mock failure".to_string()))
            } else {
                Ok(ChatResponse {
                    id: "test-id".to_string(),
                    model: "mock-model".to_string(),
                    choices: vec![Choice {
                        index: 0,
                        message: Message::text("assistant", "Mock response"),
                        finish_reason: "stop".to_string(),
                        stop_reason: Some(StopReason::EndTurn),
                    }],
                    usage: Usage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                        total_tokens: 15,
                    },
                })
            }
        }

        fn name(&self) -> &str {
            &self.name
        }

        async fn is_available(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn test_route_to_default_provider() {
        let config = LlmConfig {
            default_provider: "mock".to_string(),
            fallback_enabled: false,
            timeout_seconds: 30,
            max_retries: 0,
        };

        let mut router = LlmRouter::new(config);
        router.register_provider(
            "mock",
            Arc::new(MockProvider {
                name: "mock".to_string(),
                should_fail: false,
            }),
        );

        let request = ChatRequest {
            messages: vec![Message::text("user", "Hello")],
            model: None,
            max_tokens: None,
            temperature: None,
            stream: false,
            tools: vec![],
            tool_choice: None,
            ..Default::default()
        };

        let response = router.route(request).await;
        assert!(response.is_ok());
        let response = response.unwrap();
        assert_eq!(response.model, "mock-model");
    }

    #[tokio::test]
    async fn test_fallback_on_failure() {
        let config = LlmConfig {
            default_provider: "primary".to_string(),
            fallback_enabled: true,
            timeout_seconds: 30,
            max_retries: 0,
        };

        let mut router = LlmRouter::new(config);
        router.register_provider(
            "primary",
            Arc::new(MockProvider {
                name: "primary".to_string(),
                should_fail: true,
            }),
        );
        router.register_provider(
            "fallback",
            Arc::new(MockProvider {
                name: "fallback".to_string(),
                should_fail: false,
            }),
        );

        let request = ChatRequest {
            messages: vec![Message::text("user", "Hello")],
            model: None,
            max_tokens: None,
            temperature: None,
            stream: false,
            tools: vec![],
            tool_choice: None,
            ..Default::default()
        };

        let response = router.route(request).await;
        assert!(response.is_ok());
    }

    #[tokio::test]
    async fn test_provider_not_found() {
        let config = LlmConfig {
            default_provider: "nonexistent".to_string(),
            fallback_enabled: false,
            timeout_seconds: 30,
            max_retries: 0,
        };

        let router = LlmRouter::new(config);

        let request = ChatRequest {
            messages: vec![],
            model: None,
            max_tokens: None,
            temperature: None,
            stream: false,
            tools: vec![],
            tool_choice: None,
            ..Default::default()
        };

        let response = router.route(request).await;
        assert!(response.is_err());
    }

    #[tokio::test]
    async fn test_tool_use_response() {
        let tool_use_message = Message::with_blocks(
            "assistant",
            vec![ContentBlock::ToolUse {
                id: "call_123".to_string(),
                name: "filesystem_read".to_string(),
                input: serde_json::json!({"path": "/tmp/test.txt"}),
            }],
        );

        assert!(tool_use_message.has_tool_use());
        let tool_uses = tool_use_message.get_tool_uses();
        assert_eq!(tool_uses.len(), 1);
        assert_eq!(tool_uses[0].name, "filesystem_read");
    }

    #[tokio::test]
    async fn test_chat_response_requires_tool_use() {
        let response = ChatResponse {
            id: "test-id".to_string(),
            model: "test-model".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message::with_blocks(
                    "assistant",
                    vec![ContentBlock::ToolUse {
                        id: "call_456".to_string(),
                        name: "code_execute".to_string(),
                        input: serde_json::json!({"code": "print('hello')"}),
                    }],
                ),
                finish_reason: "tool_use".to_string(),
                stop_reason: Some(StopReason::ToolUse),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        };

        assert!(response.requires_tool_use());
        let tool_uses = response.get_tool_uses();
        assert_eq!(tool_uses.len(), 1);
        assert_eq!(tool_uses[0].name, "code_execute");
    }
}
