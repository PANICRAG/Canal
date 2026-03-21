//! Test Scenario Builder
//!
//! Provides a builder pattern to construct complete test scenarios
//! with pre-configured Agent runners, mock LLM, mock tools, and sessions.

use crate::helpers::mock_llm::{MockLlmClient, MockLlmResponse};
use crate::helpers::mock_tools::{MockToolExecutor, MockToolResult};
use gateway_core::agent::r#loop::config::{AgentConfig, CompactionConfig};
use gateway_core::agent::r#loop::runner::AgentRunner;
use gateway_core::agent::session::MemorySessionStorage;
use gateway_core::agent::types::PermissionMode;
use serde_json::Value;
use std::sync::Arc;

/// A fully constructed test scenario
pub struct TestScenario {
    /// The agent runner to test
    pub agent_runner: AgentRunner,
    /// Reference to the mock LLM (for inspection)
    pub mock_llm: Arc<MockLlmClient>,
    /// Reference to the mock tools (for inspection)
    pub mock_tools: Arc<MockToolExecutor>,
    /// Session storage for persistence tests
    pub session_storage: Arc<MemorySessionStorage>,
}

/// Builder for constructing test scenarios
pub struct ScenarioBuilder {
    tools: Vec<ToolSpec>,
    llm_responses: Vec<MockLlmResponse>,
    default_llm_response: Option<MockLlmResponse>,
    permission_mode: PermissionMode,
    max_turns: u32,
    compaction: Option<CompactionConfig>,
    session_id: Option<String>,
    latency_ms: u64,
    system_prompt: Option<String>,
}

struct ToolSpec {
    name: String,
    description: String,
    schema: Value,
    result: MockToolResult,
}

impl ScenarioBuilder {
    /// Create a new scenario builder with defaults
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            llm_responses: Vec::new(),
            default_llm_response: None,
            permission_mode: PermissionMode::BypassPermissions,
            max_turns: 200,
            compaction: None,
            session_id: None,
            latency_ms: 0,
            system_prompt: None,
        }
    }

    /// Add a tool with its name, schema, and default result
    pub fn with_tool(
        mut self,
        name: &str,
        description: &str,
        schema: Value,
        result: MockToolResult,
    ) -> Self {
        self.tools.push(ToolSpec {
            name: name.to_string(),
            description: description.to_string(),
            schema,
            result,
        });
        self
    }

    /// Add common filesystem tools
    pub fn with_filesystem_tools(mut self) -> Self {
        self.tools.push(ToolSpec {
            name: "read_file".to_string(),
            description: "Read the contents of a file".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "path": {"type": "string"} },
                "required": ["path"]
            }),
            result: MockToolResult::Success(serde_json::json!({"content": "file contents"})),
        });
        self.tools.push(ToolSpec {
            name: "write_file".to_string(),
            description: "Write content to a file".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "path": {"type": "string"}, "content": {"type": "string"} },
                "required": ["path", "content"]
            }),
            result: MockToolResult::Success(serde_json::json!({"success": true})),
        });
        self.tools.push(ToolSpec {
            name: "search_files".to_string(),
            description: "Search for files matching a pattern".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "pattern": {"type": "string"} },
                "required": ["pattern"]
            }),
            result: MockToolResult::Success(serde_json::json!({"files": ["main.rs", "lib.rs"]})),
        });
        self
    }

    /// Add bash tool
    pub fn with_bash_tool(mut self) -> Self {
        self.tools.push(ToolSpec {
            name: "bash".to_string(),
            description: "Execute a shell command".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "command": {"type": "string"} },
                "required": ["command"]
            }),
            result: MockToolResult::Success(
                serde_json::json!({"output": "command output", "exit_code": 0}),
            ),
        });
        self
    }

    /// Set pre-programmed LLM responses (consumed in order)
    pub fn with_llm_responses(mut self, responses: Vec<MockLlmResponse>) -> Self {
        self.llm_responses = responses;
        self
    }

    /// Set the default LLM response (used when queue is empty)
    pub fn with_default_llm_response(mut self, response: MockLlmResponse) -> Self {
        self.default_llm_response = Some(response);
        self
    }

    /// Set the permission mode
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        self
    }

    /// Set the max turns
    pub fn with_max_turns(mut self, turns: u32) -> Self {
        self.max_turns = turns;
        self
    }

    /// Set compaction configuration
    pub fn with_compaction(
        mut self,
        max_tokens: usize,
        target_tokens: usize,
        keep_recent: usize,
    ) -> Self {
        self.compaction = Some(CompactionConfig {
            enabled: true,
            max_context_tokens: max_tokens,
            min_messages_to_keep: keep_recent,
            target_tokens,
        });
        self
    }

    /// Disable compaction
    pub fn without_compaction(mut self) -> Self {
        self.compaction = Some(CompactionConfig::disabled());
        self
    }

    /// Set specific session ID
    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Set LLM latency per call
    pub fn with_latency_ms(mut self, ms: u64) -> Self {
        self.latency_ms = ms;
        self
    }

    /// Set system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Build the test scenario
    pub async fn build(self) -> TestScenario {
        // Build mock LLM
        let mock_llm = Arc::new(MockLlmClient::new().with_latency_ms(self.latency_ms));

        // Queue responses
        if !self.llm_responses.is_empty() {
            mock_llm.queue_responses(self.llm_responses).await;
        }

        // Set default response
        if let Some(default) = self.default_llm_response {
            mock_llm.set_default_response(default).await;
        }

        // Build mock tools
        let mock_tools = Arc::new(MockToolExecutor::new());
        for tool in &self.tools {
            mock_tools
                .register_tool(
                    &tool.name,
                    &tool.description,
                    tool.schema.clone(),
                    tool.result.clone(),
                )
                .await;
        }

        // Build config
        let mut config = AgentConfig::default();
        config.max_turns = self.max_turns;
        config.permission_mode = self.permission_mode;
        if let Some(compaction) = self.compaction {
            config.compaction = compaction;
        }
        if let Some(prompt) = self.system_prompt {
            config.system_prompt = Some(prompt);
        }

        // Build agent runner
        let agent_runner = if let Some(session_id) = self.session_id {
            AgentRunner::with_session_id(config, session_id)
        } else {
            AgentRunner::new(config)
        }
        .with_llm(mock_llm.clone() as Arc<dyn gateway_core::agent::r#loop::runner::LlmClient>)
        .with_tools(
            mock_tools.clone() as Arc<dyn gateway_core::agent::r#loop::runner::ToolExecutor>
        );

        // Build session storage
        let session_storage = Arc::new(MemorySessionStorage::new());

        TestScenario {
            agent_runner,
            mock_llm,
            mock_tools,
            session_storage,
        }
    }
}

impl Default for ScenarioBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to collect all messages from an agent query stream
pub async fn collect_messages(
    agent: &mut AgentRunner,
    prompt: &str,
) -> Vec<gateway_core::agent::AgentMessage> {
    use futures::StreamExt;
    use gateway_core::agent::AgentLoop;

    let stream = agent.query(prompt).await;
    futures::pin_mut!(stream);

    let mut messages = Vec::new();
    while let Some(result) = stream.next().await {
        match result {
            Ok(msg) => messages.push(msg),
            Err(e) => {
                // Store error as a system message for test inspection
                messages.push(gateway_core::agent::AgentMessage::System(
                    gateway_core::agent::SystemMessage {
                        subtype: "error".to_string(),
                        data: serde_json::json!({"error": e.to_string()}),
                    },
                ));
                break;
            }
        }
    }
    messages
}

/// Helper to run a multi-turn conversation and collect all messages per turn
pub async fn run_conversation(
    agent: &mut AgentRunner,
    prompts: &[&str],
) -> Vec<Vec<gateway_core::agent::AgentMessage>> {
    let mut all_turns = Vec::new();
    for prompt in prompts {
        let messages = collect_messages(agent, prompt).await;
        all_turns.push(messages);
    }
    all_turns
}

/// Helper to check if messages contain a result message
pub fn has_result_message(messages: &[gateway_core::agent::AgentMessage]) -> bool {
    messages
        .iter()
        .any(|m| matches!(m, gateway_core::agent::AgentMessage::Result(_)))
}

/// Helper to extract all text content from messages
pub fn extract_text_content(messages: &[gateway_core::agent::AgentMessage]) -> Vec<String> {
    let mut texts = Vec::new();
    for msg in messages {
        match msg {
            gateway_core::agent::AgentMessage::Assistant(a) => {
                for block in &a.content {
                    if let gateway_core::agent::ContentBlock::Text { text } = block {
                        texts.push(text.clone());
                    }
                }
            }
            _ => {}
        }
    }
    texts
}

/// Helper to count tool use blocks in messages
pub fn count_tool_uses(messages: &[gateway_core::agent::AgentMessage]) -> usize {
    let mut count = 0;
    for msg in messages {
        if let gateway_core::agent::AgentMessage::Assistant(a) = msg {
            count += a.content.iter().filter(|b| b.is_tool_use()).count();
        }
    }
    count
}
