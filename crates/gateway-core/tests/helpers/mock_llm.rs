//! Programmable Mock LLM Client
//!
//! Provides a configurable LLM client for testing the agent loop without
//! requiring real API calls. Supports response queues, failure injection,
//! latency simulation, and detailed call logging.

use async_trait::async_trait;
use gateway_core::agent::r#loop::runner::{LlmClient, LlmResponse, StopReason};
use gateway_core::agent::{AgentError, AgentMessage, ContentBlock, Usage};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

/// A single recorded LLM call
#[derive(Debug, Clone)]
pub struct LlmCallRecord {
    pub call_index: u64,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<Value>,
    pub response: MockLlmResponse,
    pub duration_ms: u64,
    pub timestamp: std::time::SystemTime,
}

/// Mock LLM response types
#[derive(Debug, Clone)]
pub enum MockLlmResponse {
    /// Pure text response
    Text(String),
    /// Single tool use call
    ToolUse { name: String, input: Value },
    /// Multiple tool uses in one response
    MultiToolUse(Vec<ToolUseBlock>),
    /// Text followed by a tool use
    TextThenTool {
        text: String,
        tool_use: ToolUseBlock,
    },
    /// Simulate an error
    Error(String),
    /// Simulate a timeout (sleep indefinitely)
    Timeout,
    /// Generate a large text response of specified size in KB
    LargeText { size_kb: usize },
}

/// A single tool use block
#[derive(Debug, Clone)]
pub struct ToolUseBlock {
    pub name: String,
    pub input: Value,
}

impl ToolUseBlock {
    pub fn new(name: impl Into<String>, input: Value) -> Self {
        Self {
            name: name.into(),
            input,
        }
    }
}

/// Programmable mock LLM client
pub struct MockLlmClient {
    /// Response queue: returns pre-set responses in order
    response_queue: Arc<Mutex<VecDeque<MockLlmResponse>>>,
    /// Default response when queue is empty
    default_response: Arc<RwLock<MockLlmResponse>>,
    /// Call counter
    call_count: Arc<AtomicU64>,
    /// Per-call latency in milliseconds
    latency_ms: Arc<AtomicU64>,
    /// Fail at specific call index (0-based)
    fail_at_call: Arc<RwLock<Option<u64>>>,
    /// Cumulative input tokens
    total_input_tokens: Arc<AtomicU64>,
    /// Cumulative output tokens
    total_output_tokens: Arc<AtomicU64>,
    /// Call log for inspection
    call_log: Arc<Mutex<Vec<LlmCallRecord>>>,
}

impl MockLlmClient {
    /// Create a new mock LLM with a text default response
    pub fn new() -> Self {
        Self {
            response_queue: Arc::new(Mutex::new(VecDeque::new())),
            default_response: Arc::new(RwLock::new(MockLlmResponse::Text(
                "Mock response".to_string(),
            ))),
            call_count: Arc::new(AtomicU64::new(0)),
            latency_ms: Arc::new(AtomicU64::new(0)),
            fail_at_call: Arc::new(RwLock::new(None)),
            total_input_tokens: Arc::new(AtomicU64::new(0)),
            total_output_tokens: Arc::new(AtomicU64::new(0)),
            call_log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Set the response queue
    pub fn with_responses(self, responses: Vec<MockLlmResponse>) -> Self {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            let mut queue = self.response_queue.lock().await;
            *queue = VecDeque::from(responses);
        });
        self
    }

    /// Queue a single response
    pub async fn queue_response(&self, response: MockLlmResponse) {
        self.response_queue.lock().await.push_back(response);
    }

    /// Queue multiple responses
    pub async fn queue_responses(&self, responses: Vec<MockLlmResponse>) {
        let mut queue = self.response_queue.lock().await;
        for r in responses {
            queue.push_back(r);
        }
    }

    /// Set the default response (used when queue is empty)
    pub async fn set_default_response(&self, response: MockLlmResponse) {
        *self.default_response.write().await = response;
    }

    /// Set per-call latency
    pub fn with_latency_ms(self, ms: u64) -> Self {
        self.latency_ms.store(ms, Ordering::SeqCst);
        self
    }

    /// Set which call index should fail
    pub async fn set_fail_at_call(&self, call_index: u64) {
        *self.fail_at_call.write().await = Some(call_index);
    }

    /// Clear failure injection
    pub async fn clear_fail_at_call(&self) {
        *self.fail_at_call.write().await = None;
    }

    /// Get total call count
    pub fn call_count(&self) -> u64 {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Get total input tokens consumed
    pub fn total_input_tokens(&self) -> u64 {
        self.total_input_tokens.load(Ordering::SeqCst)
    }

    /// Get total output tokens produced
    pub fn total_output_tokens(&self) -> u64 {
        self.total_output_tokens.load(Ordering::SeqCst)
    }

    /// Get the full call log
    pub async fn call_log(&self) -> Vec<LlmCallRecord> {
        self.call_log.lock().await.clone()
    }

    /// Get the Nth call record
    pub async fn get_call(&self, index: usize) -> Option<LlmCallRecord> {
        self.call_log.lock().await.get(index).cloned()
    }

    /// Get the last call record
    pub async fn last_call(&self) -> Option<LlmCallRecord> {
        self.call_log.lock().await.last().cloned()
    }

    /// Reset all state
    pub async fn reset(&self) {
        self.response_queue.lock().await.clear();
        self.call_count.store(0, Ordering::SeqCst);
        self.total_input_tokens.store(0, Ordering::SeqCst);
        self.total_output_tokens.store(0, Ordering::SeqCst);
        self.call_log.lock().await.clear();
        *self.fail_at_call.write().await = None;
    }

    /// Build content blocks and stop reason from a MockLlmResponse
    fn build_response_content(response: &MockLlmResponse) -> (Vec<ContentBlock>, StopReason) {
        static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

        match response {
            MockLlmResponse::Text(text) => {
                (vec![ContentBlock::text(text.clone())], StopReason::EndTurn)
            }
            MockLlmResponse::ToolUse { name, input } => {
                let id = format!(
                    "toolu_{:016x}",
                    TOOL_CALL_COUNTER.fetch_add(1, Ordering::SeqCst)
                );
                (
                    vec![ContentBlock::tool_use(id, name.clone(), input.clone())],
                    StopReason::ToolUse,
                )
            }
            MockLlmResponse::MultiToolUse(tools) => {
                let blocks: Vec<ContentBlock> = tools
                    .iter()
                    .map(|t| {
                        let id = format!(
                            "toolu_{:016x}",
                            TOOL_CALL_COUNTER.fetch_add(1, Ordering::SeqCst)
                        );
                        ContentBlock::tool_use(id, t.name.clone(), t.input.clone())
                    })
                    .collect();
                (blocks, StopReason::ToolUse)
            }
            MockLlmResponse::TextThenTool { text, tool_use } => {
                let id = format!(
                    "toolu_{:016x}",
                    TOOL_CALL_COUNTER.fetch_add(1, Ordering::SeqCst)
                );
                (
                    vec![
                        ContentBlock::text(text.clone()),
                        ContentBlock::tool_use(id, tool_use.name.clone(), tool_use.input.clone()),
                    ],
                    StopReason::ToolUse,
                )
            }
            MockLlmResponse::LargeText { size_kb } => {
                let text = "X".repeat(size_kb * 1024);
                (vec![ContentBlock::text(text)], StopReason::EndTurn)
            }
            // Error and Timeout are handled before reaching here
            MockLlmResponse::Error(_) | MockLlmResponse::Timeout => unreachable!(),
        }
    }

    /// Estimate input tokens from messages (rough: 4 chars per token)
    fn estimate_input_tokens(messages: &[AgentMessage]) -> u32 {
        let total_chars: usize = messages
            .iter()
            .map(|m| match m {
                AgentMessage::User(u) => u.content.to_string_content().len(),
                AgentMessage::Assistant(a) => a
                    .content
                    .iter()
                    .map(|b| match b {
                        ContentBlock::Text { text } => text.len(),
                        ContentBlock::ToolUse { input, .. } => input.to_string().len(),
                        ContentBlock::ToolResult { content, .. } => content
                            .as_ref()
                            .map(|c| c.to_string_content().len())
                            .unwrap_or(0),
                        _ => 0,
                    })
                    .sum(),
                AgentMessage::System(s) => s.data.to_string().len(),
                _ => 0,
            })
            .sum();
        (total_chars / 4).max(1) as u32
    }

    /// Estimate output tokens from content blocks
    fn estimate_output_tokens(blocks: &[ContentBlock]) -> u32 {
        let total_chars: usize = blocks
            .iter()
            .map(|b| match b {
                ContentBlock::Text { text } => text.len(),
                ContentBlock::ToolUse { input, .. } => input.to_string().len() + 50,
                _ => 0,
            })
            .sum();
        (total_chars / 4).max(1) as u32
    }
}

impl Default for MockLlmClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn generate(
        &self,
        messages: Vec<AgentMessage>,
        tools: Vec<Value>,
    ) -> Result<LlmResponse, AgentError> {
        let call_index = self.call_count.fetch_add(1, Ordering::SeqCst);
        let start = Instant::now();

        // Simulate latency
        let latency = self.latency_ms.load(Ordering::SeqCst);
        if latency > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(latency)).await;
        }

        // Check for failure injection
        if let Some(fail_at) = *self.fail_at_call.read().await {
            if call_index == fail_at {
                let response = MockLlmResponse::Error("Injected failure".to_string());
                self.call_log.lock().await.push(LlmCallRecord {
                    call_index,
                    messages: messages.clone(),
                    tools: tools.clone(),
                    response,
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: std::time::SystemTime::now(),
                });
                return Err(AgentError::ApiError("Injected failure".to_string()));
            }
        }

        // Get next response from queue or use default
        let response = {
            let mut queue = self.response_queue.lock().await;
            if let Some(r) = queue.pop_front() {
                r
            } else {
                self.default_response.read().await.clone()
            }
        };

        // Handle special response types
        match &response {
            MockLlmResponse::Error(msg) => {
                self.call_log.lock().await.push(LlmCallRecord {
                    call_index,
                    messages,
                    tools,
                    response: response.clone(),
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: std::time::SystemTime::now(),
                });
                return Err(AgentError::ApiError(msg.clone()));
            }
            MockLlmResponse::Timeout => {
                // Sleep for a very long time to simulate timeout
                self.call_log.lock().await.push(LlmCallRecord {
                    call_index,
                    messages,
                    tools,
                    response: response.clone(),
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: std::time::SystemTime::now(),
                });
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                return Err(AgentError::Timeout("LLM timeout".to_string()));
            }
            _ => {}
        }

        let (content, stop_reason) = Self::build_response_content(&response);

        // Track tokens
        let input_tokens = Self::estimate_input_tokens(&messages);
        let output_tokens = Self::estimate_output_tokens(&content);
        self.total_input_tokens
            .fetch_add(input_tokens as u64, Ordering::SeqCst);
        self.total_output_tokens
            .fetch_add(output_tokens as u64, Ordering::SeqCst);

        // Log the call
        self.call_log.lock().await.push(LlmCallRecord {
            call_index,
            messages,
            tools,
            response: response.clone(),
            duration_ms: start.elapsed().as_millis() as u64,
            timestamp: std::time::SystemTime::now(),
        });

        Ok(LlmResponse {
            content,
            model: "mock-model".to_string(),
            usage: Usage {
                input_tokens,
                output_tokens,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            stop_reason,
        })
    }
}
