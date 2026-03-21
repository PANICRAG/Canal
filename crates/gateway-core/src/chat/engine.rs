//! Chat Engine - core message handling
//!
//! The ChatEngine manages conversations and message handling, including
//! tool use loop integration with the ToolUseEngine.

use dashmap::DashMap;
use futures::StreamExt;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;
use uuid::Uuid;

use super::artifact::StoredArtifact;
use super::artifact_extractor::{ArtifactExtractor, ArtifactExtractorConfig};
use super::artifact_store::ArtifactStore;
use super::message::{Artifact, ChatMessage};
use super::session::ChatSession;
use super::streaming::{StreamEvent, TokenUsage};
use super::tool_use::{ToolUseConfig, ToolUseEngine, ToolUseEvent};
use crate::agent::hooks::{HookCallback, HookExecutor, RegisteredHook, ShellHookRunner};
use crate::agent::memory::MemoryStore;
use crate::agent::session::{
    CompactTrigger, ContextCompactor, Session as AgentSession, SessionError, SessionManager,
};
use crate::agent::types::{
    AgentMessage, HookContext, HookEvent, MemoryLoadedHookData, MessageContent, SessionEndHookData,
    SessionStartHookData, UserMemory, UserMessage,
};

#[cfg(test)]
use crate::agent::memory::InMemoryStore;
#[cfg(test)]
use crate::agent::session::{DefaultSessionManager, MemorySessionStorage};
use crate::error::Result;
use crate::llm::{ChatRequest, LlmRouter, Message, StreamChunk};
use crate::mcp::gateway::McpGateway;

/// Chat response
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub message_id: Uuid,
    pub content: String,
    pub artifacts: Vec<Artifact>,
    pub usage: Option<TokenUsage>,
    /// Tool calls made during the response
    pub tool_calls: Vec<ToolCallInfo>,
}

/// Information about a tool call made during the conversation
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub result: String,
    pub is_error: bool,
}

/// Chat Engine configuration
#[derive(Debug, Clone)]
pub struct ChatEngineConfig {
    /// Whether tool use is enabled
    pub enable_tools: bool,
    /// Tool use configuration
    pub tool_use_config: ToolUseConfig,
    /// Enable session persistence
    pub enable_session_persistence: bool,
    /// Message count threshold for auto-compaction
    pub compaction_threshold: usize,
    /// Number of recent messages to keep after compaction
    pub compaction_keep_recent: usize,
    /// Enable user memory system
    pub enable_memory: bool,
    /// Include memories in system prompt
    pub include_memories_in_prompt: bool,
    /// Enable artifact extraction and storage
    pub enable_artifact_extraction: bool,
    /// Artifact extractor configuration
    pub artifact_extractor_config: ArtifactExtractorConfig,
}

impl Default for ChatEngineConfig {
    fn default() -> Self {
        Self {
            enable_tools: true,
            tool_use_config: ToolUseConfig::default(),
            enable_session_persistence: false,
            compaction_threshold: 50,
            compaction_keep_recent: 10,
            enable_memory: false,
            include_memories_in_prompt: true,
            enable_artifact_extraction: true,
            artifact_extractor_config: ArtifactExtractorConfig::default(),
        }
    }
}

/// Chat Engine - manages conversations and message handling
pub struct ChatEngine {
    sessions: Arc<DashMap<Uuid, ChatSession>>,
    llm_router: Arc<LlmRouter>,
    mcp_gateway: Option<Arc<McpGateway>>,
    config: ChatEngineConfig,
    hook_executor: Arc<HookExecutor>,
    /// Session manager for persistence (Agent SDK compatible)
    session_manager: Option<Arc<dyn SessionManager>>,
    /// Context compactor for auto-compaction
    context_compactor: ContextCompactor,
    /// Memory store for user preferences
    memory_store: Option<Arc<dyn MemoryStore>>,
    /// Artifact store for persisting extracted artifacts
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    /// Artifact extractor for detecting artifacts in responses
    artifact_extractor: ArtifactExtractor,
}

impl ChatEngine {
    /// Create a new chat engine (without tool support)
    pub fn new(llm_router: Arc<LlmRouter>) -> Self {
        let config = ChatEngineConfig::default();
        let context_compactor = ContextCompactor::new().keep_recent(config.compaction_keep_recent);
        let artifact_extractor = ArtifactExtractor::new(config.artifact_extractor_config.clone());
        Self {
            sessions: Arc::new(DashMap::new()),
            llm_router,
            mcp_gateway: None,
            config,
            hook_executor: Arc::new(HookExecutor::new()),
            session_manager: None,
            context_compactor,
            memory_store: None,
            artifact_store: None,
            artifact_extractor,
        }
    }

    /// Create a new chat engine with tool support
    pub fn with_tools(
        llm_router: Arc<LlmRouter>,
        mcp_gateway: Arc<McpGateway>,
        config: ChatEngineConfig,
    ) -> Self {
        let context_compactor = ContextCompactor::new().keep_recent(config.compaction_keep_recent);
        let artifact_extractor = ArtifactExtractor::new(config.artifact_extractor_config.clone());
        Self {
            sessions: Arc::new(DashMap::new()),
            llm_router,
            mcp_gateway: Some(mcp_gateway),
            config,
            hook_executor: Arc::new(HookExecutor::new()),
            session_manager: None,
            context_compactor,
            memory_store: None,
            artifact_store: None,
            artifact_extractor,
        }
    }

    /// Create a new chat engine with tool support and hook executor
    pub fn with_hooks(
        llm_router: Arc<LlmRouter>,
        mcp_gateway: Option<Arc<McpGateway>>,
        config: ChatEngineConfig,
        hook_executor: Arc<HookExecutor>,
    ) -> Self {
        let context_compactor = ContextCompactor::new().keep_recent(config.compaction_keep_recent);
        let artifact_extractor = ArtifactExtractor::new(config.artifact_extractor_config.clone());
        Self {
            sessions: Arc::new(DashMap::new()),
            llm_router,
            mcp_gateway,
            config,
            hook_executor,
            session_manager: None,
            context_compactor,
            memory_store: None,
            artifact_store: None,
            artifact_extractor,
        }
    }

    /// Create a new chat engine with session persistence
    pub fn with_session_manager(
        llm_router: Arc<LlmRouter>,
        mcp_gateway: Option<Arc<McpGateway>>,
        config: ChatEngineConfig,
        session_manager: Arc<dyn SessionManager>,
    ) -> Self {
        let context_compactor = ContextCompactor::new().keep_recent(config.compaction_keep_recent);
        let artifact_extractor = ArtifactExtractor::new(config.artifact_extractor_config.clone());
        Self {
            sessions: Arc::new(DashMap::new()),
            llm_router,
            mcp_gateway,
            config,
            hook_executor: Arc::new(HookExecutor::new()),
            session_manager: Some(session_manager),
            context_compactor,
            memory_store: None,
            artifact_store: None,
            artifact_extractor,
        }
    }

    /// Create a fully configured chat engine
    pub fn with_all(
        llm_router: Arc<LlmRouter>,
        mcp_gateway: Option<Arc<McpGateway>>,
        config: ChatEngineConfig,
        hook_executor: Arc<HookExecutor>,
        session_manager: Option<Arc<dyn SessionManager>>,
    ) -> Self {
        let context_compactor = ContextCompactor::new().keep_recent(config.compaction_keep_recent);
        let artifact_extractor = ArtifactExtractor::new(config.artifact_extractor_config.clone());
        Self {
            sessions: Arc::new(DashMap::new()),
            llm_router,
            mcp_gateway,
            config,
            hook_executor,
            session_manager,
            context_compactor,
            memory_store: None,
            artifact_store: None,
            artifact_extractor,
        }
    }

    /// Create a fully configured chat engine with memory support
    pub fn with_memory(
        llm_router: Arc<LlmRouter>,
        mcp_gateway: Option<Arc<McpGateway>>,
        config: ChatEngineConfig,
        hook_executor: Arc<HookExecutor>,
        session_manager: Option<Arc<dyn SessionManager>>,
        memory_store: Arc<dyn MemoryStore>,
    ) -> Self {
        let context_compactor = ContextCompactor::new().keep_recent(config.compaction_keep_recent);
        let artifact_extractor = ArtifactExtractor::new(config.artifact_extractor_config.clone());
        Self {
            sessions: Arc::new(DashMap::new()),
            llm_router,
            mcp_gateway,
            config,
            hook_executor,
            session_manager,
            context_compactor,
            memory_store: Some(memory_store),
            artifact_store: None,
            artifact_extractor,
        }
    }

    /// Set the session manager
    pub fn set_session_manager(&mut self, session_manager: Arc<dyn SessionManager>) {
        self.session_manager = Some(session_manager);
    }

    /// Get the session manager
    pub fn session_manager(&self) -> Option<&Arc<dyn SessionManager>> {
        self.session_manager.as_ref()
    }

    /// Set the memory store
    pub fn set_memory_store(&mut self, memory_store: Arc<dyn MemoryStore>) {
        self.memory_store = Some(memory_store);
    }

    /// Get the memory store
    pub fn memory_store(&self) -> Option<&Arc<dyn MemoryStore>> {
        self.memory_store.as_ref()
    }

    /// Check if memory is enabled and available
    pub fn memory_available(&self) -> bool {
        self.config.enable_memory && self.memory_store.is_some()
    }

    /// Set the artifact store
    pub fn set_artifact_store(&mut self, artifact_store: Arc<dyn ArtifactStore>) {
        self.artifact_store = Some(artifact_store);
    }

    /// Get the artifact store
    pub fn artifact_store(&self) -> Option<&Arc<dyn ArtifactStore>> {
        self.artifact_store.as_ref()
    }

    /// Check if artifact extraction is enabled and available
    pub fn artifact_extraction_available(&self) -> bool {
        self.config.enable_artifact_extraction
    }

    /// Extract artifacts from response content
    pub fn extract_artifacts(
        &self,
        content: &str,
        session_id: Option<Uuid>,
        message_id: Option<Uuid>,
    ) -> Vec<StoredArtifact> {
        if !self.artifact_extraction_available() {
            return vec![];
        }

        self.artifact_extractor
            .extract(content, session_id, message_id)
            .into_iter()
            .map(|extracted| extracted.artifact)
            .collect()
    }

    /// Extract and store artifacts from response content
    pub async fn extract_and_store_artifacts(
        &self,
        content: &str,
        session_id: Option<Uuid>,
        message_id: Option<Uuid>,
    ) -> Vec<StoredArtifact> {
        let artifacts = self.extract_artifacts(content, session_id, message_id);

        // Store artifacts if artifact store is available
        if let Some(store) = &self.artifact_store {
            for artifact in &artifacts {
                if let Err(e) = store.save(artifact).await {
                    tracing::warn!(
                        artifact_id = %artifact.id,
                        error = %e,
                        "Failed to store artifact"
                    );
                }
            }
        }

        artifacts
    }

    /// Get the hook executor
    pub fn hook_executor(&self) -> &Arc<HookExecutor> {
        &self.hook_executor
    }

    /// Register a hook
    pub async fn register_hook(&self, hook: RegisteredHook) {
        self.hook_executor.register(hook).await;
    }

    /// Register a shell hook for a specific event
    pub async fn register_shell_hook(&self, event: HookEvent, command: String) {
        let runner =
            ShellHookRunner::new(format!("shell_hook_{:?}", event), command).events(vec![event]);
        let hook = RegisteredHook::new(Arc::new(runner) as Arc<dyn HookCallback>, vec![event]);
        self.hook_executor.register(hook).await;
    }

    /// Check if tool use is enabled and available
    pub fn tools_available(&self) -> bool {
        self.config.enable_tools && self.mcp_gateway.is_some()
    }

    /// Handle incoming message
    pub async fn handle_message(
        &self,
        user_id: Uuid,
        conversation_id: Uuid,
        message: &str,
        stream_tx: broadcast::Sender<StreamEvent>,
    ) -> Result<ChatResponse> {
        let session_start = Instant::now();

        // 1. Get or create session
        let mut session = self.get_or_create_session(user_id, conversation_id);

        // 2. Create message ID for this response
        let message_id = Uuid::new_v4();

        // 3. Create hook context
        let hook_context = HookContext {
            session_id: conversation_id.to_string(),
            cwd: std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string()),
            env: None,
            metadata: None,
        };

        // 4. Trigger SessionStart hook (non-blocking)
        let start_data = SessionStartHookData {
            session_id: conversation_id.to_string(),
            initial_prompt: Some(message.to_string()),
            config: None,
        };
        let hook_executor = self.hook_executor.clone();
        let hook_context_clone = hook_context.clone();
        tokio::spawn(async move {
            hook_executor
                .execute(
                    HookEvent::SessionStart,
                    serde_json::to_value(&start_data).unwrap_or_default(),
                    &hook_context_clone,
                )
                .await;
        });

        // 5. Send start event
        if stream_tx
            .send(StreamEvent::start(conversation_id, message_id))
            .is_err()
        {
            tracing::debug!("Stream receiver closed, client likely disconnected");
        }

        // 6. Add user message to session
        session.add_message(ChatMessage::user(message));

        // 7. Send thinking event
        if stream_tx
            .send(StreamEvent::thinking("Processing your request..."))
            .is_err()
        {
            tracing::debug!("Stream receiver closed, client likely disconnected");
        }

        // 8. Build chat request for LLM
        let mut messages: Vec<Message> = Vec::new();

        // Add system prompt for tool use if tools are available
        if self.tools_available() {
            let tool_names = if let Some(gateway) = &self.mcp_gateway {
                let tools = gateway.get_tools().await;
                tools
                    .iter()
                    .map(|t| format!("{}_{}", t.namespace, t.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                String::new()
            };

            let system_prompt = format!(
                "You are a helpful AI assistant with access to tools that can help you complete tasks. \
                When a user asks you to perform an action that requires using tools (like browsing the web, \
                reading files, or executing code), you should use the appropriate tool.\n\n\
                Available tools include: {}\n\n\
                When using tools:\n\
                1. Analyze what the user wants to accomplish\n\
                2. Choose the appropriate tool(s) to help achieve that goal\n\
                3. Execute the tool with the correct parameters\n\
                4. Interpret the results and provide a helpful response\n\n\
                # Browser Tools - IMPORTANT\n\
                For browser automation, follow this workflow:\n\n\
                1. **Use browser_snapshot as your PRIMARY tool** to see the page state. It returns:\n\
                   - The page's accessibility tree with unique ref IDs (e.g., 'e15', 'e23')\n\
                   - Only ~2-5K tokens vs ~20K for screenshots - much more efficient!\n\
                   - Interactive elements like buttons, links, inputs with their ref IDs\n\n\
                2. **Click and fill using ref IDs**:\n\
                   - browser_click: Use `ref: \"e15\"` instead of CSS selectors\n\
                   - browser_fill: Use `ref: \"e23\"` and `text: \"your input\"`\n\
                   - Ref IDs are faster and more reliable than CSS selectors\n\n\
                3. **For text extraction**, use browser_get_page_text (very low token count)\n\n\
                4. **For finding elements by description**, use browser_find (natural language search)\n\n\
                5. **AVOID browser_screenshot** unless you specifically need visual information.\n\
                   Screenshots are expensive (~20K tokens) and can overflow context limits.\n\n\
                Example browser workflow:\n\
                - browser_navigate -> browser_snapshot -> browser_click(ref: \"e5\") -> browser_snapshot\n\n\
                For filesystem tools, you can read and write files within allowed directories.\n\n\
                For code execution tools, you can run Python, JavaScript, Bash, and other code snippets.\n\n\
                Always be proactive in using tools when they would help complete the user's request.",
                tool_names
            );
            messages.push(Message::text("system", system_prompt));
        }

        // Add conversation history
        let history: Vec<Message> = session
            .get_recent_messages(20)
            .iter()
            .map(|m| Message::text(m.role.to_string(), m.content.clone()))
            .collect();
        messages.extend(history);

        let request = ChatRequest {
            messages,
            model: None,
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stream: true,
            ..Default::default()
        };

        // 9. Execute with or without tools
        let (content, tool_calls, usage) = if self.tools_available() {
            match self.execute_with_tools(request, &stream_tx).await {
                Ok((llm_response, tool_calls)) => {
                    let content = llm_response
                        .choices
                        .first()
                        .map(|c| c.message.content.clone())
                        .unwrap_or_default();
                    let usage = TokenUsage {
                        prompt_tokens: llm_response.usage.prompt_tokens,
                        completion_tokens: llm_response.usage.completion_tokens,
                        total_tokens: llm_response.usage.total_tokens,
                    };
                    (content, tool_calls, usage)
                }
                Err(e) => {
                    // Trigger SessionEnd hook with error
                    let duration_ms = session_start.elapsed().as_millis() as u64;
                    let end_data = SessionEndHookData {
                        session_id: conversation_id.to_string(),
                        duration_ms,
                        num_turns: 1,
                        is_error: true,
                        result: Some(e.to_string()),
                    };
                    let hook_executor = self.hook_executor.clone();
                    let hook_context = hook_context.clone();
                    tokio::spawn(async move {
                        hook_executor
                            .execute(
                                HookEvent::SessionEnd,
                                serde_json::to_value(&end_data).unwrap_or_default(),
                                &hook_context,
                            )
                            .await;
                    });
                    return Err(e);
                }
            }
        } else {
            // No tools, use streaming
            match self.execute_streaming(request, &stream_tx).await {
                Ok(result) => result,
                Err(e) => {
                    // Trigger SessionEnd hook with error
                    let duration_ms = session_start.elapsed().as_millis() as u64;
                    let end_data = SessionEndHookData {
                        session_id: conversation_id.to_string(),
                        duration_ms,
                        num_turns: 1,
                        is_error: true,
                        result: Some(e.to_string()),
                    };
                    let hook_executor = self.hook_executor.clone();
                    let hook_context = hook_context.clone();
                    tokio::spawn(async move {
                        hook_executor
                            .execute(
                                HookEvent::SessionEnd,
                                serde_json::to_value(&end_data).unwrap_or_default(),
                                &hook_context,
                            )
                            .await;
                    });
                    return Err(e);
                }
            }
        };

        // 10. Add assistant message to session
        let assistant_msg = ChatMessage::assistant(&content);
        session.add_message(assistant_msg);

        // 11. Auto-generate title if needed
        session.auto_title();

        // 12. Store updated session
        self.sessions.insert(conversation_id, session);

        // 13. Trigger SessionEnd hook (non-blocking)
        let duration_ms = session_start.elapsed().as_millis() as u64;
        let end_data = SessionEndHookData {
            session_id: conversation_id.to_string(),
            duration_ms,
            num_turns: 1,
            is_error: false,
            result: Some(content.clone()),
        };
        let hook_executor = self.hook_executor.clone();
        tokio::spawn(async move {
            hook_executor
                .execute(
                    HookEvent::SessionEnd,
                    serde_json::to_value(&end_data).unwrap_or_default(),
                    &hook_context,
                )
                .await;
        });

        // 14. Send done event
        // R3-M: Log stream send failures for observability
        if stream_tx
            .send(StreamEvent::done_with_usage(
                message_id,
                vec![],
                usage.clone(),
            ))
            .is_err()
        {
            tracing::debug!("Stream receiver closed, client likely disconnected");
        }

        Ok(ChatResponse {
            message_id,
            content,
            artifacts: vec![],
            usage: Some(usage),
            tool_calls,
        })
    }

    /// Execute streaming request without tools
    async fn execute_streaming(
        &self,
        request: ChatRequest,
        stream_tx: &broadcast::Sender<StreamEvent>,
    ) -> Result<(String, Vec<ToolCallInfo>, TokenUsage)> {
        let mut stream = self.llm_router.route_stream(request).await?;
        let mut content = String::new();
        let mut usage = TokenUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        };

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => match chunk {
                    StreamChunk::TextDelta { text } => {
                        content.push_str(&text);
                        if stream_tx.send(StreamEvent::text(&text)).is_err() {
                            tracing::debug!("Stream closed during text delta");
                        }
                    }
                    StreamChunk::Done {
                        usage: chunk_usage,
                        stop_reason: _,
                    } => {
                        usage = TokenUsage {
                            prompt_tokens: chunk_usage.prompt_tokens,
                            completion_tokens: chunk_usage.completion_tokens,
                            total_tokens: chunk_usage.total_tokens,
                        };
                    }
                    StreamChunk::ToolUseStart { id, name } => {
                        if stream_tx
                            .send(StreamEvent::tool_call(&id, &name, &serde_json::json!({})))
                            .is_err()
                        {
                            tracing::debug!("Stream closed during tool call");
                        }
                    }
                    StreamChunk::ToolUseComplete { id, input } => {
                        // This shouldn't happen without tools, but handle it
                        tracing::warn!(
                            "Received tool use complete without tool support: {} {:?}",
                            id,
                            input
                        );
                    }
                    StreamChunk::ToolUseInputDelta { .. } => {
                        // Ignore input deltas in non-tool mode
                    }
                    StreamChunk::ThinkingDelta { text } => {
                        if stream_tx.send(StreamEvent::thinking(&text)).is_err() {
                            tracing::debug!("Stream closed during thinking delta");
                        }
                    }
                    StreamChunk::Error { message } => {
                        if stream_tx.send(StreamEvent::error(&message, false)).is_err() {
                            tracing::debug!("Stream closed while sending error");
                        }
                        return Err(crate::error::Error::Llm(message));
                    }
                },
                Err(e) => {
                    if stream_tx
                        .send(StreamEvent::error(&e.to_string(), false))
                        .is_err()
                    {
                        tracing::debug!("Stream closed while sending error");
                    }
                    return Err(e.into());
                }
            }
        }

        Ok((content, vec![], usage))
    }

    /// Execute a request with tool support
    async fn execute_with_tools(
        &self,
        request: ChatRequest,
        stream_tx: &broadcast::Sender<StreamEvent>,
    ) -> Result<(crate::llm::ChatResponse, Vec<ToolCallInfo>)> {
        // R3-H19: Graceful error instead of panic when MCP gateway is not configured
        let mcp_gateway = self.mcp_gateway.as_ref().ok_or_else(|| {
            crate::error::Error::Mcp("MCP Gateway not available for tool execution".into())
        })?;

        let tool_use_engine = ToolUseEngine::new(
            self.llm_router.clone(),
            mcp_gateway.clone(),
            self.config.tool_use_config.clone(),
        );

        // Create a channel for tool use events
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<ToolUseEvent>(100);

        // Clone stream_tx for the event handler task
        let stream_tx_clone = stream_tx.clone();

        // Collect tool calls
        let tool_calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let tool_calls_clone = tool_calls.clone();

        // Spawn task to handle tool use events and forward to stream
        let event_handler = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match event {
                    ToolUseEvent::ToolCall {
                        id,
                        name,
                        arguments,
                    } => {
                        let _ =
                            stream_tx_clone.send(StreamEvent::tool_call(&id, &name, &arguments));
                        // Store tool call info (will be updated with result later)
                        let mut calls = tool_calls_clone.lock().await;
                        calls.push(ToolCallInfo {
                            id: id.clone(),
                            name,
                            arguments,
                            result: String::new(),
                            is_error: false,
                        });
                    }
                    ToolUseEvent::ToolResult {
                        id,
                        name,
                        result,
                        is_error,
                    } => {
                        let result_value = serde_json::json!({
                            "content": result.clone(),
                            "is_error": is_error
                        });
                        let _ = stream_tx_clone.send(StreamEvent::tool_result(
                            &id,
                            &name,
                            &result_value,
                        ));
                        // Update tool call with result
                        let mut calls = tool_calls_clone.lock().await;
                        if let Some(call) = calls.iter_mut().find(|c| c.id == id) {
                            call.result = result;
                            call.is_error = is_error;
                        }
                    }
                    ToolUseEvent::ToolCancelled { id, name, reason } => {
                        let result_value = serde_json::json!({
                            "cancelled": true,
                            "reason": reason.clone()
                        });
                        let _ = stream_tx_clone.send(StreamEvent::tool_result(
                            &id,
                            &name,
                            &result_value,
                        ));
                        // Mark tool as cancelled
                        let mut calls = tool_calls_clone.lock().await;
                        if let Some(call) = calls.iter_mut().find(|c| c.id == id) {
                            call.result = format!("Cancelled: {}", reason);
                            call.is_error = true;
                        }
                    }
                    ToolUseEvent::Text(text) => {
                        let _ = stream_tx_clone.send(StreamEvent::text(&text));
                    }
                    ToolUseEvent::Thinking(msg) => {
                        let _ = stream_tx_clone.send(StreamEvent::thinking(&msg));
                    }
                    ToolUseEvent::Done => {
                        // Done event will be sent by the main flow
                    }
                    ToolUseEvent::Error(err) => {
                        let _ = stream_tx_clone.send(StreamEvent::error(&err, true));
                    }
                }
            }
        });

        // Execute with tools
        let response = tool_use_engine
            .execute_with_tools(request, event_tx)
            .await?;

        // Wait for event handler to finish
        let _ = event_handler.await;

        // Get collected tool calls
        let tool_calls = match Arc::try_unwrap(tool_calls) {
            Ok(mutex) => mutex.into_inner(),
            Err(arc) => arc.lock().await.clone(),
        };

        Ok((response, tool_calls))
    }

    /// Get or create a chat session
    fn get_or_create_session(&self, user_id: Uuid, conversation_id: Uuid) -> ChatSession {
        if let Some(session) = self.sessions.get(&conversation_id) {
            session.clone()
        } else {
            ChatSession::with_id(conversation_id, user_id)
        }
    }

    /// Get a session by ID
    pub fn get_session(&self, conversation_id: &Uuid) -> Option<ChatSession> {
        self.sessions.get(conversation_id).map(|s| s.clone())
    }

    /// Get all sessions for a user
    pub fn get_user_sessions(&self, user_id: &Uuid) -> Vec<ChatSession> {
        self.sessions
            .iter()
            .filter(|entry| &entry.value().user_id == user_id)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Delete a session
    pub fn delete_session(&self, conversation_id: &Uuid) -> bool {
        self.sessions.remove(conversation_id).is_some()
    }

    /// Get session count
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    // ==================== Session Persistence Methods ====================

    /// Resume a persistent session by ID
    ///
    /// This loads the session from the session manager and restores its messages
    /// to the conversation context.
    pub async fn resume_session(
        &self,
        session_id: &str,
    ) -> std::result::Result<AgentSession, SessionError> {
        let session_manager = self.session_manager.as_ref().ok_or_else(|| {
            SessionError::StorageError("Session manager not configured".to_string())
        })?;

        let session = session_manager.resume(session_id).await?;

        tracing::info!(
            session_id = session_id,
            message_count = session.metadata.message_count,
            "Session resumed successfully"
        );

        Ok(session)
    }

    /// Fork a persistent session, creating a new branch from an existing session
    ///
    /// This creates a new session with the same messages as the original,
    /// allowing for alternative conversation paths.
    pub async fn fork_session(
        &self,
        session_id: &str,
    ) -> std::result::Result<AgentSession, SessionError> {
        let session_manager = self.session_manager.as_ref().ok_or_else(|| {
            SessionError::StorageError("Session manager not configured".to_string())
        })?;

        let forked = session_manager.fork(session_id).await?;

        tracing::info!(
            original_session_id = session_id,
            new_session_id = forked.id(),
            "Session forked successfully"
        );

        Ok(forked)
    }

    /// Create a new persistent session
    pub async fn create_persistent_session(
        &self,
        cwd: &str,
    ) -> std::result::Result<AgentSession, SessionError> {
        let session_manager = self.session_manager.as_ref().ok_or_else(|| {
            SessionError::StorageError("Session manager not configured".to_string())
        })?;

        let session = session_manager.create(cwd).await?;

        tracing::info!(
            session_id = session.id(),
            cwd = cwd,
            "New persistent session created"
        );

        Ok(session)
    }

    /// Save a persistent session
    pub async fn save_persistent_session(
        &self,
        session: &AgentSession,
    ) -> std::result::Result<(), SessionError> {
        let session_manager = self.session_manager.as_ref().ok_or_else(|| {
            SessionError::StorageError("Session manager not configured".to_string())
        })?;

        session_manager.save(session).await?;

        tracing::debug!(session_id = session.id(), "Session saved");

        Ok(())
    }

    /// List persistent sessions
    pub async fn list_persistent_sessions(
        &self,
        limit: Option<u32>,
    ) -> std::result::Result<Vec<crate::agent::session::SessionMetadata>, SessionError> {
        let session_manager = self.session_manager.as_ref().ok_or_else(|| {
            SessionError::StorageError("Session manager not configured".to_string())
        })?;

        session_manager.list(limit).await
    }

    /// Delete a persistent session
    pub async fn delete_persistent_session(
        &self,
        session_id: &str,
    ) -> std::result::Result<(), SessionError> {
        let session_manager = self.session_manager.as_ref().ok_or_else(|| {
            SessionError::StorageError("Session manager not configured".to_string())
        })?;

        session_manager.delete(session_id).await?;

        tracing::info!(session_id = session_id, "Session deleted");

        Ok(())
    }

    // ==================== Context Compaction Methods ====================

    /// Check if a session needs context compaction
    pub fn needs_compaction(&self, messages: &[AgentMessage]) -> bool {
        messages.len() >= self.config.compaction_threshold
            || self.context_compactor.needs_compaction(messages)
    }

    /// Compact session context if needed
    ///
    /// This summarizes older messages to reduce context size while preserving
    /// recent conversation history.
    pub async fn compact_context(
        &self,
        messages: &[AgentMessage],
    ) -> std::result::Result<Vec<AgentMessage>, crate::agent::session::CompactionError> {
        if !self.needs_compaction(messages) {
            return Ok(messages.to_vec());
        }

        let trigger = if messages.len() >= self.config.compaction_threshold {
            CompactTrigger::MessageCount(messages.len())
        } else {
            CompactTrigger::TokenLimit(self.context_compactor.estimate_tokens(messages))
        };

        let result = self.context_compactor.compact(messages, trigger).await?;

        tracing::info!(
            messages_before = result.tokens_before,
            messages_after = result.tokens_after,
            messages_removed = result.messages_removed,
            "Context compacted"
        );

        Ok(result.messages)
    }

    /// Handle message with persistent session support
    ///
    /// This method integrates session persistence with the regular message handling,
    /// automatically saving messages and performing compaction when needed.
    pub async fn handle_message_with_persistence(
        &self,
        user_id: Uuid,
        conversation_id: Uuid,
        message: &str,
        stream_tx: broadcast::Sender<StreamEvent>,
        mut persistent_session: Option<&mut AgentSession>,
    ) -> Result<ChatResponse> {
        // Add message to persistent session if provided
        if let Some(ref mut session) = persistent_session {
            let user_msg = AgentMessage::User(UserMessage {
                content: MessageContent::text(message),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            });
            session.add_message(user_msg).await;
        }

        // Handle the message normally
        let response = self
            .handle_message(user_id, conversation_id, message, stream_tx)
            .await?;

        // Save to persistent session if provided
        if let Some(ref mut session) = persistent_session {
            // Add assistant response to persistent session
            let assistant_msg = AgentMessage::Assistant(crate::agent::types::AssistantMessage {
                content: vec![crate::agent::types::ContentBlock::Text {
                    text: response.content.clone(),
                }],
                model: "unknown".to_string(),
                parent_tool_use_id: None,
                error: None,
            });
            session.add_message(assistant_msg).await;

            // Save the session
            if let Err(e) = session.save().await {
                tracing::warn!(error = %e, "Failed to save persistent session");
            }

            // Check if compaction is needed
            let messages = session.messages().await;
            if self.needs_compaction(&messages) {
                match self.compact_context(&messages).await {
                    Ok(_compacted) => {
                        // Note: In a full implementation, we would update the session
                        // with the compacted messages. For now, we just log.
                        tracing::info!("Context compaction completed");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Context compaction failed");
                    }
                }
            }
        }

        Ok(response)
    }

    // ==================== Memory Methods ====================

    /// Load user memories for a session
    ///
    /// This loads the user's memories from the memory store and optionally
    /// triggers a hook to notify of loaded memories.
    pub async fn load_user_memories(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> std::result::Result<UserMemory, crate::agent::memory::MemoryError> {
        let memory_store = self.memory_store.as_ref().ok_or_else(|| {
            crate::agent::memory::MemoryError::StorageError(
                "Memory store not configured".to_string(),
            )
        })?;

        let memory = memory_store.load(user_id).await?;

        // Trigger MemoryLoaded hook
        if !memory.is_empty() {
            let hook_data = MemoryLoadedHookData {
                user_id: user_id.to_string(),
                session_id: session_id.to_string(),
                memory_count: memory.len(),
                keys: memory.entries.keys().cloned().collect(),
            };

            let hook_context = HookContext {
                session_id: session_id.to_string(),
                cwd: None,
                env: None,
                metadata: None,
            };

            let hook_executor = self.hook_executor.clone();
            tokio::spawn(async move {
                hook_executor
                    .execute(
                        HookEvent::MemoryLoaded,
                        serde_json::to_value(&hook_data).unwrap_or_default(),
                        &hook_context,
                    )
                    .await;
            });
        }

        tracing::debug!(
            user_id = user_id,
            memory_count = memory.len(),
            "User memories loaded"
        );

        Ok(memory)
    }

    /// Save user memories
    pub async fn save_user_memories(
        &self,
        memory: &UserMemory,
    ) -> std::result::Result<(), crate::agent::memory::MemoryError> {
        let memory_store = self.memory_store.as_ref().ok_or_else(|| {
            crate::agent::memory::MemoryError::StorageError(
                "Memory store not configured".to_string(),
            )
        })?;

        memory_store.save(memory).await?;

        tracing::debug!(
            user_id = memory.user_id,
            memory_count = memory.len(),
            "User memories saved"
        );

        Ok(())
    }

    /// Update a specific memory entry
    ///
    /// This updates or creates a memory entry and triggers a hook.
    pub async fn update_memory(
        &self,
        user_id: &str,
        entry: crate::agent::types::MemoryEntry,
        session_id: Option<&str>,
    ) -> std::result::Result<(), crate::agent::memory::MemoryError> {
        let memory_store = self.memory_store.as_ref().ok_or_else(|| {
            crate::agent::memory::MemoryError::StorageError(
                "Memory store not configured".to_string(),
            )
        })?;

        // Get old value for hook
        let old_value = memory_store
            .get(user_id, &entry.key)
            .await?
            .map(|e| e.value);

        // Update the entry
        memory_store.set(user_id, entry.clone()).await?;

        // Trigger MemoryUpdate hook
        let hook_data = crate::agent::types::MemoryUpdateHookData {
            user_id: user_id.to_string(),
            key: entry.key.clone(),
            old_value,
            new_value: entry.value.clone(),
            source: format!("{:?}", entry.source),
            session_id: session_id.map(|s| s.to_string()),
        };

        let hook_context = HookContext {
            session_id: session_id.unwrap_or("").to_string(),
            cwd: None,
            env: None,
            metadata: None,
        };

        let hook_executor = self.hook_executor.clone();
        tokio::spawn(async move {
            hook_executor
                .execute(
                    HookEvent::MemoryUpdate,
                    serde_json::to_value(&hook_data).unwrap_or_default(),
                    &hook_context,
                )
                .await;
        });

        tracing::debug!(user_id = user_id, key = entry.key, "Memory entry updated");

        Ok(())
    }

    /// Format user memories for inclusion in system prompt
    ///
    /// This returns a formatted string of user memories suitable for
    /// inclusion in the system prompt.
    pub async fn format_memories_for_prompt(
        &self,
        user_id: &str,
    ) -> std::result::Result<Option<String>, crate::agent::memory::MemoryError> {
        if !self.memory_available() || !self.config.include_memories_in_prompt {
            return Ok(None);
        }

        let memory_store = self.memory_store.as_ref().unwrap();
        let memory = memory_store.load(user_id).await?;

        if memory.is_empty() {
            return Ok(None);
        }

        Ok(Some(memory.format_for_prompt()))
    }

    /// Handle message with memory context
    ///
    /// This method loads user memories and includes them in the conversation context
    /// before processing the message.
    pub async fn handle_message_with_memory(
        &self,
        user_id: Uuid,
        conversation_id: Uuid,
        message: &str,
        stream_tx: broadcast::Sender<StreamEvent>,
    ) -> Result<ChatResponse> {
        // Load memories if available
        let memory_context = if self.memory_available() {
            match self.format_memories_for_prompt(&user_id.to_string()).await {
                Ok(Some(memories)) => {
                    tracing::debug!("Including {} chars of memory context", memories.len());
                    Some(memories)
                }
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to load memories, proceeding without");
                    None
                }
            }
        } else {
            None
        };

        // Prepend memory context to the user message so the LLM has
        // relevant memories available when generating a response.
        // R3-H18: Sanitize memory context to prevent prompt injection via stored memories.
        // Remove any closing tags that could break out of the memory_context block.
        let augmented_message = if let Some(memory_ctx) = memory_context {
            let sanitized_ctx = memory_ctx.replace("</memory_context>", "");
            format!(
                "<memory_context>\n{}\n</memory_context>\n\n{}",
                sanitized_ctx, message
            )
        } else {
            message.to_string()
        };

        self.handle_message(user_id, conversation_id, &augmented_message, stream_tx)
            .await
    }

    // ==================== Artifact Methods ====================

    /// Load an artifact by ID
    pub async fn load_artifact(
        &self,
        artifact_id: Uuid,
    ) -> std::result::Result<StoredArtifact, super::artifact_store::ArtifactStoreError> {
        let artifact_store = self.artifact_store.as_ref().ok_or_else(|| {
            super::artifact_store::ArtifactStoreError::StorageError(
                "Artifact store not configured".to_string(),
            )
        })?;

        artifact_store.load(artifact_id).await
    }

    /// List artifacts for a session
    pub async fn list_session_artifacts(
        &self,
        session_id: Uuid,
    ) -> std::result::Result<Vec<StoredArtifact>, super::artifact_store::ArtifactStoreError> {
        let artifact_store = self.artifact_store.as_ref().ok_or_else(|| {
            super::artifact_store::ArtifactStoreError::StorageError(
                "Artifact store not configured".to_string(),
            )
        })?;

        artifact_store.get_by_session(session_id).await
    }

    /// List artifacts for a message
    pub async fn list_message_artifacts(
        &self,
        message_id: Uuid,
    ) -> std::result::Result<Vec<StoredArtifact>, super::artifact_store::ArtifactStoreError> {
        let artifact_store = self.artifact_store.as_ref().ok_or_else(|| {
            super::artifact_store::ArtifactStoreError::StorageError(
                "Artifact store not configured".to_string(),
            )
        })?;

        artifact_store.get_by_message(message_id).await
    }

    /// Delete an artifact
    pub async fn delete_artifact(
        &self,
        artifact_id: Uuid,
    ) -> std::result::Result<(), super::artifact_store::ArtifactStoreError> {
        let artifact_store = self.artifact_store.as_ref().ok_or_else(|| {
            super::artifact_store::ArtifactStoreError::StorageError(
                "Artifact store not configured".to_string(),
            )
        })?;

        artifact_store.delete(artifact_id).await?;

        tracing::info!(artifact_id = %artifact_id, "Artifact deleted");

        Ok(())
    }

    /// Delete all artifacts for a session
    pub async fn delete_session_artifacts(
        &self,
        session_id: Uuid,
    ) -> std::result::Result<u32, super::artifact_store::ArtifactStoreError> {
        let artifact_store = self.artifact_store.as_ref().ok_or_else(|| {
            super::artifact_store::ArtifactStoreError::StorageError(
                "Artifact store not configured".to_string(),
            )
        })?;

        let count = artifact_store.delete_by_session(session_id).await?;

        tracing::info!(
            session_id = %session_id,
            deleted_count = count,
            "Session artifacts deleted"
        );

        Ok(count)
    }

    /// Handle message with artifact extraction
    ///
    /// This method automatically extracts artifacts from the response and stores them.
    pub async fn handle_message_with_artifacts(
        &self,
        user_id: Uuid,
        conversation_id: Uuid,
        message: &str,
        stream_tx: broadcast::Sender<StreamEvent>,
    ) -> Result<ChatResponse> {
        let mut response = self
            .handle_message(user_id, conversation_id, message, stream_tx)
            .await?;

        // Extract and store artifacts
        if self.artifact_extraction_available() {
            let stored_artifacts = self
                .extract_and_store_artifacts(
                    &response.content,
                    Some(conversation_id),
                    Some(response.message_id),
                )
                .await;

            // Convert StoredArtifacts to Artifact type for response
            let artifacts: Vec<Artifact> = stored_artifacts
                .into_iter()
                .map(|sa| {
                    let artifact_type = sa.artifact_type.clone();
                    let actions = sa
                        .actions
                        .iter()
                        .map(|a| super::message::ArtifactAction {
                            id: a.id.clone(),
                            label: a.label.clone(),
                            action_type: map_artifact_action_type(&a.action_type),
                            params: a.payload.clone(),
                        })
                        .collect();
                    Artifact {
                        id: sa.id,
                        artifact_type,
                        title: sa.title,
                        data: serde_json::to_value(&sa.content).unwrap_or_default(),
                        actions,
                    }
                })
                .collect();

            response.artifacts = artifacts;
        }

        Ok(response)
    }
}

/// Map stored artifact action type to message action type.
fn map_artifact_action_type(
    stored: &super::artifact::ArtifactActionType,
) -> super::message::ActionType {
    match stored {
        super::artifact::ArtifactActionType::Edit => super::message::ActionType::Edit,
        super::artifact::ArtifactActionType::Download => super::message::ActionType::Download,
        super::artifact::ArtifactActionType::Share => super::message::ActionType::Share,
        super::artifact::ArtifactActionType::OpenExternal => super::message::ActionType::Open,
        super::artifact::ArtifactActionType::Delete => super::message::ActionType::Cancel,
        _ => super::message::ActionType::Confirm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmConfig;

    #[test]
    fn test_chat_engine_creation() {
        let config = LlmConfig::default();
        let router = Arc::new(LlmRouter::new(config));
        let engine = ChatEngine::new(router);
        assert_eq!(engine.session_count(), 0);
    }

    #[test]
    fn test_get_or_create_session() {
        let config = LlmConfig::default();
        let router = Arc::new(LlmRouter::new(config));
        let engine = ChatEngine::new(router);

        let user_id = Uuid::new_v4();
        let conv_id = Uuid::new_v4();

        let session = engine.get_or_create_session(user_id, conv_id);
        assert_eq!(session.id, conv_id);
        assert_eq!(session.user_id, user_id);
    }

    #[test]
    fn test_chat_engine_with_session_manager() {
        let config = LlmConfig::default();
        let router = Arc::new(LlmRouter::new(config));
        let storage = Arc::new(MemorySessionStorage::new());
        let session_manager = Arc::new(DefaultSessionManager::new(storage));

        let engine = ChatEngine::with_session_manager(
            router,
            None,
            ChatEngineConfig::default(),
            session_manager.clone(),
        );

        assert!(engine.session_manager().is_some());
    }

    #[tokio::test]
    async fn test_create_persistent_session() {
        let config = LlmConfig::default();
        let router = Arc::new(LlmRouter::new(config));
        let storage = Arc::new(MemorySessionStorage::new());
        let session_manager = Arc::new(DefaultSessionManager::new(storage));

        let engine = ChatEngine::with_session_manager(
            router,
            None,
            ChatEngineConfig::default(),
            session_manager,
        );

        let session = engine.create_persistent_session("/tmp").await.unwrap();
        assert!(!session.id().is_empty());
    }

    #[tokio::test]
    async fn test_resume_and_fork_session() {
        let config = LlmConfig::default();
        let router = Arc::new(LlmRouter::new(config));
        let storage = Arc::new(MemorySessionStorage::new());
        let session_manager = Arc::new(DefaultSessionManager::new(storage));

        let engine = ChatEngine::with_session_manager(
            router,
            None,
            ChatEngineConfig::default(),
            session_manager,
        );

        // Create a session
        let session = engine.create_persistent_session("/tmp").await.unwrap();
        let session_id = session.id().to_string();

        // Resume the session
        let resumed = engine.resume_session(&session_id).await.unwrap();
        assert_eq!(resumed.id(), session_id);

        // Fork the session
        let forked = engine.fork_session(&session_id).await.unwrap();
        assert_ne!(forked.id(), session_id);
        assert_eq!(forked.metadata.parent_id, Some(session_id));
    }

    #[tokio::test]
    async fn test_list_and_delete_sessions() {
        let config = LlmConfig::default();
        let router = Arc::new(LlmRouter::new(config));
        let storage = Arc::new(MemorySessionStorage::new());
        let session_manager = Arc::new(DefaultSessionManager::new(storage));

        let engine = ChatEngine::with_session_manager(
            router,
            None,
            ChatEngineConfig::default(),
            session_manager,
        );

        // Create sessions
        let _s1 = engine.create_persistent_session("/tmp/1").await.unwrap();
        let s2 = engine.create_persistent_session("/tmp/2").await.unwrap();

        // List sessions
        let sessions = engine.list_persistent_sessions(None).await.unwrap();
        assert_eq!(sessions.len(), 2);

        // Delete a session
        engine.delete_persistent_session(s2.id()).await.unwrap();

        let sessions = engine.list_persistent_sessions(None).await.unwrap();
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn test_compaction_threshold() {
        let config = LlmConfig::default();
        let router = Arc::new(LlmRouter::new(config));

        let mut chat_config = ChatEngineConfig::default();
        chat_config.compaction_threshold = 5;

        let engine = ChatEngine::with_tools(router, Arc::new(McpGateway::new()), chat_config);

        // Create test messages
        let messages: Vec<AgentMessage> = (0..3)
            .map(|i| {
                AgentMessage::User(UserMessage {
                    content: MessageContent::text(format!("Message {}", i)),
                    uuid: None,
                    parent_tool_use_id: None,
                    tool_use_result: None,
                })
            })
            .collect();

        // Should not need compaction with only 3 messages
        assert!(!engine.needs_compaction(&messages));

        // Create more messages
        let many_messages: Vec<AgentMessage> = (0..10)
            .map(|i| {
                AgentMessage::User(UserMessage {
                    content: MessageContent::text(format!("Message {}", i)),
                    uuid: None,
                    parent_tool_use_id: None,
                    tool_use_result: None,
                })
            })
            .collect();

        // Should need compaction with 10 messages (threshold is 5)
        assert!(engine.needs_compaction(&many_messages));
    }

    #[test]
    fn test_chat_engine_with_memory() {
        let config = LlmConfig::default();
        let router = Arc::new(LlmRouter::new(config));
        let memory_store = Arc::new(InMemoryStore::new());

        let mut chat_config = ChatEngineConfig::default();
        chat_config.enable_memory = true;

        let engine = ChatEngine::with_memory(
            router,
            None,
            chat_config,
            Arc::new(HookExecutor::new()),
            None,
            memory_store,
        );

        assert!(engine.memory_available());
        assert!(engine.memory_store().is_some());
    }

    #[test]
    fn test_chat_engine_memory_not_available() {
        let config = LlmConfig::default();
        let router = Arc::new(LlmRouter::new(config));

        // Memory disabled in config
        let engine = ChatEngine::new(router.clone());
        assert!(!engine.memory_available());

        // Memory enabled but no store
        let mut chat_config = ChatEngineConfig::default();
        chat_config.enable_memory = true;

        let engine = ChatEngine::with_all(
            router,
            None,
            chat_config,
            Arc::new(HookExecutor::new()),
            None,
        );
        assert!(!engine.memory_available());
    }

    #[tokio::test]
    async fn test_load_and_save_memories() {
        let config = LlmConfig::default();
        let router = Arc::new(LlmRouter::new(config));
        let memory_store = Arc::new(InMemoryStore::new());

        let mut chat_config = ChatEngineConfig::default();
        chat_config.enable_memory = true;

        let engine = ChatEngine::with_memory(
            router,
            None,
            chat_config,
            Arc::new(HookExecutor::new()),
            None,
            memory_store,
        );

        // Load empty memories
        let memories = engine
            .load_user_memories("user-1", "session-1")
            .await
            .unwrap();
        assert!(memories.is_empty());

        // Save memories
        let mut memory = crate::agent::types::UserMemory::new("user-1");
        memory.set_text("name", "Alice");
        memory.set_text("language", "Rust");

        engine.save_user_memories(&memory).await.unwrap();

        // Load again
        let loaded = engine
            .load_user_memories("user-1", "session-1")
            .await
            .unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get_str("name"), Some("Alice"));
    }

    #[tokio::test]
    async fn test_update_memory() {
        let config = LlmConfig::default();
        let router = Arc::new(LlmRouter::new(config));
        let memory_store = Arc::new(InMemoryStore::new());

        let mut chat_config = ChatEngineConfig::default();
        chat_config.enable_memory = true;

        let engine = ChatEngine::with_memory(
            router,
            None,
            chat_config,
            Arc::new(HookExecutor::new()),
            None,
            memory_store,
        );

        // Update a memory entry
        let entry = crate::agent::types::MemoryEntry::text("favorite_color", "blue");
        engine
            .update_memory("user-1", entry, Some("session-1"))
            .await
            .unwrap();

        // Verify
        let memories = engine
            .load_user_memories("user-1", "session-1")
            .await
            .unwrap();
        assert_eq!(memories.get_str("favorite_color"), Some("blue"));

        // Update again
        let entry = crate::agent::types::MemoryEntry::text("favorite_color", "green");
        engine
            .update_memory("user-1", entry, Some("session-1"))
            .await
            .unwrap();

        let memories = engine
            .load_user_memories("user-1", "session-1")
            .await
            .unwrap();
        assert_eq!(memories.get_str("favorite_color"), Some("green"));
    }

    #[tokio::test]
    async fn test_format_memories_for_prompt() {
        let config = LlmConfig::default();
        let router = Arc::new(LlmRouter::new(config));
        let memory_store = Arc::new(InMemoryStore::new());

        let mut chat_config = ChatEngineConfig::default();
        chat_config.enable_memory = true;
        chat_config.include_memories_in_prompt = true;

        let engine = ChatEngine::with_memory(
            router,
            None,
            chat_config,
            Arc::new(HookExecutor::new()),
            None,
            memory_store,
        );

        // Empty memories returns None
        let formatted = engine.format_memories_for_prompt("user-1").await.unwrap();
        assert!(formatted.is_none());

        // Add some memories
        let mut memory = crate::agent::types::UserMemory::new("user-1");
        memory.set_text("name", "Alice");
        engine.save_user_memories(&memory).await.unwrap();

        // Now should return formatted string
        let formatted = engine.format_memories_for_prompt("user-1").await.unwrap();
        assert!(formatted.is_some());
        assert!(formatted.unwrap().contains("name: Alice"));
    }
}
