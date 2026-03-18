//! AI Chat handling module
//!
//! This module provides the ChatHandler which communicates with the Anthropic API
//! to process chat requests with tool use support.

mod anthropic;

use crate::file_ops::FileOperations;
use crate::shell::ShellExecutor;
use anthropic::{
    AnthropicClient, AnthropicConfig, AnthropicContentBlock, AnthropicMessage, AnthropicRequest,
    AnthropicTool, StreamingEvent,
};
use futures::StreamExt;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Chat handler errors
#[derive(Error, Debug)]
pub enum ChatError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("LLM request failed: {0}")]
    LlmError(String),

    #[error("Tool execution failed: {0}")]
    ToolError(String),

    #[error("Channel send error: {0}")]
    ChannelError(String),
}

/// Chat event types
#[derive(Debug, Clone)]
pub enum ChatEvent {
    Text(String),
    Thinking(String),
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
        tool_call_id: String,
        result: String,
        is_error: bool,
    },
    Complete {
        message_id: String,
        input_tokens: i32,
        output_tokens: i32,
        stop_reason: String,
    },
    Error {
        code: String,
        message: String,
        retriable: bool,
    },
}

/// Chat request
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub session_id: String,
    pub message: String,
    pub history: Vec<Message>,
    pub available_tools: Vec<Tool>,
    pub system_prompt: Option<String>,
}

/// Message in history
#[derive(Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_results: Vec<ToolResultContent>,
}

impl Message {
    pub fn user(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
        }
    }

    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCall>) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    pub fn with_tool_results(mut self, tool_results: Vec<ToolResultContent>) -> Self {
        self.tool_results = tool_results;
        self
    }
}

/// Tool call from assistant
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Tool result content
#[derive(Debug, Clone)]
pub struct ToolResultContent {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

/// Tool definition
#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: String,
}

/// Configuration for the chat handler
#[derive(Debug, Clone)]
pub struct ChatHandlerConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
    pub workspace_dir: String,
    pub max_tool_iterations: u32,
}

impl Default for ChatHandlerConfig {
    fn default() -> Self {
        Self {
            api_key: std::env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
            model: std::env::var("ANTHROPIC_MODEL")
                .unwrap_or_else(|_| "claude-sonnet-4-6".to_string()),
            max_tokens: std::env::var("ANTHROPIC_MAX_TOKENS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(8192),
            workspace_dir: std::env::var("WORKSPACE_DIR")
                .unwrap_or_else(|_| "/workspace".to_string()),
            max_tool_iterations: std::env::var("MAX_TOOL_ITERATIONS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
        }
    }
}

/// Handles chat interactions with LLM
#[derive(Clone)]
pub struct ChatHandler {
    client: AnthropicClient,
    config: ChatHandlerConfig,
    shell_executor: ShellExecutor,
    file_ops: FileOperations,
}

impl ChatHandler {
    /// Create a new chat handler with default configuration
    pub fn new() -> Self {
        Self::with_config(ChatHandlerConfig::default())
    }

    /// Create a new chat handler with custom configuration
    pub fn with_config(config: ChatHandlerConfig) -> Self {
        let anthropic_config = AnthropicConfig {
            api_key: config.api_key.clone(),
            base_url: "https://api.anthropic.com".to_string(),
            default_model: config.model.clone(),
            api_version: "2023-06-01".to_string(),
        };

        Self {
            client: AnthropicClient::new(anthropic_config),
            shell_executor: ShellExecutor::new(&config.workspace_dir),
            file_ops: FileOperations::new(&config.workspace_dir),
            config,
        }
    }

    /// Create a new chat handler for a specific workspace
    pub fn for_workspace(workspace_dir: &str) -> Self {
        let mut config = ChatHandlerConfig::default();
        config.workspace_dir = workspace_dir.to_string();
        Self::with_config(config)
    }

    /// Handle a chat request with streaming events
    pub async fn handle_chat_stream(
        &self,
        request: ChatRequest,
        tx: mpsc::Sender<ChatEvent>,
    ) -> Result<(), ChatError> {
        info!(
            session_id = %request.session_id,
            message_len = request.message.len(),
            history_len = request.history.len(),
            tools_count = request.available_tools.len(),
            "Processing chat request"
        );

        // Validate API key
        if self.config.api_key.is_empty() {
            let err = ChatError::Config("ANTHROPIC_API_KEY not set".to_string());
            let _ = tx
                .send(ChatEvent::Error {
                    code: "CONFIG_ERROR".to_string(),
                    message: err.to_string(),
                    retriable: false,
                })
                .await;
            return Err(err);
        }

        // Build messages array from history and current message
        let mut messages = self.build_messages(&request.history);
        messages.push(AnthropicMessage::user(&request.message));

        // Convert tools to Anthropic format
        let tools = self.convert_tools(&request.available_tools);

        // Run the conversation loop with tool use
        self.run_conversation_loop(messages, tools, request.system_prompt, tx)
            .await
    }

    /// Handle a chat request and return all events (non-streaming interface)
    pub async fn handle_chat(&self, request: ChatRequest) -> Result<Vec<ChatEvent>, ChatError> {
        let (tx, mut rx) = mpsc::channel(100);

        // Spawn the streaming handler
        let handler = self.clone();
        let handle = tokio::spawn(async move { handler.handle_chat_stream(request, tx).await });

        // Collect all events
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        // Wait for the handler to complete and propagate any errors
        handle
            .await
            .map_err(|e| ChatError::LlmError(format!("Task join error: {}", e)))??;

        Ok(events)
    }

    /// Run the conversation loop handling tool calls
    async fn run_conversation_loop(
        &self,
        mut messages: Vec<AnthropicMessage>,
        tools: Vec<AnthropicTool>,
        system_prompt: Option<String>,
        tx: mpsc::Sender<ChatEvent>,
    ) -> Result<(), ChatError> {
        let mut iteration = 0;
        let mut total_input_tokens = 0i32;
        let mut total_output_tokens = 0i32;
        let mut message_id = String::new();
        let mut final_stop_reason = "end_turn".to_string();

        loop {
            iteration += 1;
            if iteration > self.config.max_tool_iterations as usize {
                warn!("Max tool iterations reached");
                let _ = tx
                    .send(ChatEvent::Error {
                        code: "MAX_ITERATIONS".to_string(),
                        message: "Maximum tool iterations reached".to_string(),
                        retriable: false,
                    })
                    .await;
                break;
            }

            debug!(iteration = iteration, "Starting LLM request iteration");

            // Build the request
            let request = AnthropicRequest {
                model: self.config.model.clone(),
                max_tokens: self.config.max_tokens,
                messages: messages.clone(),
                system: system_prompt.clone(),
                tools: tools.clone(),
                stream: true,
            };

            // Call the streaming API
            let stream_result = self.client.messages_stream(request).await;

            let mut stream = match stream_result {
                Ok(s) => s,
                Err(e) => {
                    error!(error = %e, "LLM request failed");
                    let _ = tx
                        .send(ChatEvent::Error {
                            code: "LLM_ERROR".to_string(),
                            message: e.to_string(),
                            retriable: true,
                        })
                        .await;
                    return Err(ChatError::LlmError(e.to_string()));
                }
            };

            // Process streaming events
            let mut response_text = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool_id: Option<String> = None;
            let mut current_tool_name: Option<String> = None;
            let mut current_tool_input = String::new();
            let mut stop_reason: Option<String> = None;

            while let Some(event_result) = stream.next().await {
                match event_result {
                    Ok(event) => match event {
                        StreamingEvent::MessageStart { message } => {
                            message_id = message.id;
                            total_input_tokens += message.usage.input_tokens;
                        }
                        StreamingEvent::ContentBlockStart { content_block, .. } => {
                            if let anthropic::StreamingContentBlock::ToolUse { id, name } =
                                content_block
                            {
                                current_tool_id = Some(id.clone());
                                current_tool_name = Some(name.clone());
                                current_tool_input.clear();
                            }
                        }
                        StreamingEvent::ContentBlockDelta { delta, .. } => match delta {
                            anthropic::StreamingDelta::TextDelta { text } => {
                                response_text.push_str(&text);
                                let _ = tx.send(ChatEvent::Text(text)).await;
                            }
                            anthropic::StreamingDelta::InputJsonDelta { partial_json } => {
                                current_tool_input.push_str(&partial_json);
                            }
                        },
                        StreamingEvent::ContentBlockStop { .. } => {
                            // If we were building a tool call, finalize it
                            if let (Some(id), Some(name)) =
                                (current_tool_id.take(), current_tool_name.take())
                            {
                                let tool_call = ToolCall {
                                    id: id.clone(),
                                    name: name.clone(),
                                    arguments: current_tool_input.clone(),
                                };

                                // Send tool call event
                                let _ = tx
                                    .send(ChatEvent::ToolCall {
                                        id: id.clone(),
                                        name: name.clone(),
                                        arguments: current_tool_input.clone(),
                                    })
                                    .await;

                                tool_calls.push(tool_call);
                                current_tool_input.clear();
                            }
                        }
                        StreamingEvent::MessageDelta { delta, usage } => {
                            if let Some(u) = usage {
                                total_output_tokens += u.output_tokens;
                            }
                            if let Some(reason) = delta.stop_reason {
                                stop_reason = Some(reason);
                            }
                        }
                        StreamingEvent::MessageStop => {
                            // Message complete
                        }
                        StreamingEvent::Ping => {
                            // Keep-alive, ignore
                        }
                        StreamingEvent::Error { error } => {
                            let _ = tx
                                .send(ChatEvent::Error {
                                    code: error.error_type.clone(),
                                    message: error.message.clone(),
                                    retriable: error.error_type == "overloaded_error",
                                })
                                .await;
                            return Err(ChatError::LlmError(error.message));
                        }
                    },
                    Err(e) => {
                        error!(error = %e, "Stream error");
                        let _ = tx
                            .send(ChatEvent::Error {
                                code: "STREAM_ERROR".to_string(),
                                message: e.to_string(),
                                retriable: true,
                            })
                            .await;
                        return Err(ChatError::LlmError(e.to_string()));
                    }
                }
            }

            final_stop_reason = stop_reason
                .clone()
                .unwrap_or_else(|| "end_turn".to_string());

            // Check if we need to handle tool calls
            if stop_reason.as_deref() == Some("tool_use") && !tool_calls.is_empty() {
                // Add assistant message with tool calls to history
                let assistant_msg = self.build_assistant_message(&response_text, &tool_calls);
                messages.push(assistant_msg);

                // Execute tools and collect results
                let mut tool_results = Vec::new();
                for tool_call in &tool_calls {
                    let result = self.execute_tool(tool_call).await;

                    let (content, is_error) = match result {
                        Ok(output) => (output, false),
                        Err(e) => (format!("Error: {}", e), true),
                    };

                    // Send tool result event
                    let _ = tx
                        .send(ChatEvent::ToolResult {
                            tool_call_id: tool_call.id.clone(),
                            result: content.clone(),
                            is_error,
                        })
                        .await;

                    tool_results.push(ToolResultContent {
                        tool_use_id: tool_call.id.clone(),
                        content,
                        is_error,
                    });
                }

                // Add tool results as user message
                let user_msg = self.build_tool_result_message(&tool_results);
                messages.push(user_msg);

                // Continue the loop to get the next response
                continue;
            }

            // No more tool calls, we're done
            break;
        }

        // Send completion event
        let _ = tx
            .send(ChatEvent::Complete {
                message_id,
                input_tokens: total_input_tokens,
                output_tokens: total_output_tokens,
                stop_reason: final_stop_reason,
            })
            .await;

        Ok(())
    }

    /// Build messages array from history
    fn build_messages(&self, history: &[Message]) -> Vec<AnthropicMessage> {
        history
            .iter()
            .map(|msg| {
                if msg.role == "assistant" {
                    if msg.tool_calls.is_empty() {
                        AnthropicMessage::assistant(&msg.content)
                    } else {
                        // Assistant message with tool calls
                        let mut blocks = Vec::new();
                        if !msg.content.is_empty() {
                            blocks.push(AnthropicContentBlock::Text {
                                text: msg.content.clone(),
                            });
                        }
                        for tc in &msg.tool_calls {
                            let input: serde_json::Value = serde_json::from_str(&tc.arguments)
                                .unwrap_or(serde_json::json!({}));
                            blocks.push(AnthropicContentBlock::ToolUse {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                input,
                            });
                        }
                        AnthropicMessage::with_blocks("assistant", blocks)
                    }
                } else if msg.role == "user" {
                    if msg.tool_results.is_empty() {
                        AnthropicMessage::user(&msg.content)
                    } else {
                        // User message with tool results
                        let blocks: Vec<AnthropicContentBlock> = msg
                            .tool_results
                            .iter()
                            .map(|tr| AnthropicContentBlock::ToolResult {
                                tool_use_id: tr.tool_use_id.clone(),
                                content: tr.content.clone(),
                                is_error: tr.is_error,
                            })
                            .collect();
                        AnthropicMessage::with_blocks("user", blocks)
                    }
                } else {
                    // Default to user for unknown roles
                    AnthropicMessage::user(&msg.content)
                }
            })
            .collect()
    }

    /// Build assistant message with tool calls
    fn build_assistant_message(&self, text: &str, tool_calls: &[ToolCall]) -> AnthropicMessage {
        let mut blocks = Vec::new();

        if !text.is_empty() {
            blocks.push(AnthropicContentBlock::Text {
                text: text.to_string(),
            });
        }

        for tc in tool_calls {
            let input: serde_json::Value =
                serde_json::from_str(&tc.arguments).unwrap_or(serde_json::json!({}));
            blocks.push(AnthropicContentBlock::ToolUse {
                id: tc.id.clone(),
                name: tc.name.clone(),
                input,
            });
        }

        AnthropicMessage::with_blocks("assistant", blocks)
    }

    /// Build user message with tool results
    fn build_tool_result_message(&self, results: &[ToolResultContent]) -> AnthropicMessage {
        let blocks: Vec<AnthropicContentBlock> = results
            .iter()
            .map(|r| AnthropicContentBlock::ToolResult {
                tool_use_id: r.tool_use_id.clone(),
                content: r.content.clone(),
                is_error: r.is_error,
            })
            .collect();

        AnthropicMessage::with_blocks("user", blocks)
    }

    /// Convert tools to Anthropic format
    fn convert_tools(&self, tools: &[Tool]) -> Vec<AnthropicTool> {
        tools
            .iter()
            .map(|t| {
                let schema: serde_json::Value =
                    serde_json::from_str(&t.input_schema).unwrap_or(serde_json::json!({
                        "type": "object",
                        "properties": {}
                    }));

                AnthropicTool {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: schema,
                }
            })
            .collect()
    }

    /// Execute a tool and return the result
    async fn execute_tool(&self, tool_call: &ToolCall) -> Result<String, ChatError> {
        info!(
            tool_name = %tool_call.name,
            tool_id = %tool_call.id,
            "Executing tool"
        );

        let args: serde_json::Value = serde_json::from_str(&tool_call.arguments)
            .map_err(|e| ChatError::ToolError(format!("Invalid tool arguments: {}", e)))?;

        match tool_call.name.as_str() {
            // File operations
            "read_file" | "filesystem_read_file" => {
                let path = args["path"]
                    .as_str()
                    .ok_or_else(|| ChatError::ToolError("Missing 'path' argument".to_string()))?;
                let offset = args["offset"].as_i64().unwrap_or(0);
                let limit = args["limit"].as_i64().unwrap_or(0);

                let result = self
                    .file_ops
                    .read_file(path, offset, limit)
                    .await
                    .map_err(|e| ChatError::ToolError(e.to_string()))?;

                if result.is_binary {
                    Ok(format!(
                        "Binary file: {} bytes, mime-type: {}",
                        result.size, result.mime_type
                    ))
                } else {
                    Ok(String::from_utf8_lossy(&result.content).to_string())
                }
            }

            "write_file" | "filesystem_write_file" => {
                let path = args["path"]
                    .as_str()
                    .ok_or_else(|| ChatError::ToolError("Missing 'path' argument".to_string()))?;
                let content = args["content"].as_str().ok_or_else(|| {
                    ChatError::ToolError("Missing 'content' argument".to_string())
                })?;
                let create_dirs = args["create_dirs"].as_bool().unwrap_or(true);
                let overwrite = args["overwrite"].as_bool().unwrap_or(true);

                let result = self
                    .file_ops
                    .write_file(path, content.as_bytes(), create_dirs, overwrite)
                    .await
                    .map_err(|e| ChatError::ToolError(e.to_string()))?;

                Ok(format!(
                    "Wrote {} bytes to {}{}",
                    result.bytes_written,
                    result.path,
                    if result.created { " (created)" } else { "" }
                ))
            }

            "list_directory" | "filesystem_list_directory" => {
                let path = args["path"].as_str().unwrap_or(".");
                let recursive = args["recursive"].as_bool().unwrap_or(false);
                let max_depth = args["max_depth"].as_i64().unwrap_or(1) as i32;

                let result = self
                    .file_ops
                    .list_directory(path, recursive, max_depth)
                    .await
                    .map_err(|e| ChatError::ToolError(e.to_string()))?;

                let entries: Vec<String> = result
                    .entries
                    .iter()
                    .map(|e| {
                        format!(
                            "{} {} ({})",
                            match e.entry_type {
                                crate::file_ops::EntryTypeResult::Directory => "d",
                                crate::file_ops::EntryTypeResult::File => "f",
                                crate::file_ops::EntryTypeResult::Symlink => "l",
                            },
                            e.name,
                            e.size
                        )
                    })
                    .collect();

                Ok(entries.join("\n"))
            }

            "search_files" | "filesystem_search" => {
                let path = args["path"].as_str().unwrap_or(".");
                let pattern = args["pattern"].as_str().ok_or_else(|| {
                    ChatError::ToolError("Missing 'pattern' argument".to_string())
                })?;
                let is_regex = args["is_regex"].as_bool().unwrap_or(false);
                let max_results = args["max_results"].as_i64().unwrap_or(100) as i32;

                let result = self
                    .file_ops
                    .search_files(path, pattern, is_regex, max_results)
                    .await
                    .map_err(|e| ChatError::ToolError(e.to_string()))?;

                let matches: Vec<String> = result
                    .matches
                    .iter()
                    .map(|m| format!("{}:{}: {}", m.file, m.line_number, m.line_content))
                    .collect();

                Ok(format!(
                    "Found {} matches in {} files:\n{}",
                    result.total_matches,
                    result.files_searched,
                    matches.join("\n")
                ))
            }

            "delete_file" | "filesystem_delete" => {
                let path = args["path"]
                    .as_str()
                    .ok_or_else(|| ChatError::ToolError("Missing 'path' argument".to_string()))?;
                let recursive = args["recursive"].as_bool().unwrap_or(false);

                let result = self
                    .file_ops
                    .delete_file(path, recursive)
                    .await
                    .map_err(|e| ChatError::ToolError(e.to_string()))?;

                Ok(format!(
                    "{}",
                    if result.deleted {
                        format!("Deleted {}", result.path)
                    } else {
                        format!("File not found: {}", result.path)
                    }
                ))
            }

            // Shell operations
            "execute_command" | "shell_execute" | "bash" => {
                let command = args["command"].as_str().ok_or_else(|| {
                    ChatError::ToolError("Missing 'command' argument".to_string())
                })?;
                let timeout = args["timeout"].as_i64().unwrap_or(60) as i32;

                // Execute as bash command
                let result = self
                    .shell_executor
                    .execute_command("bash", &["-c".to_string(), command.to_string()], timeout)
                    .await
                    .map_err(|e| ChatError::ToolError(e.to_string()))?;

                let mut output = String::new();
                let mut exit_code = 0;

                for event in result {
                    match event {
                        crate::shell::CommandOutputEvent::Data(data) => {
                            output.push_str(&String::from_utf8_lossy(&data));
                        }
                        crate::shell::CommandOutputEvent::Complete {
                            exit_code: code, ..
                        } => {
                            exit_code = code;
                        }
                    }
                }

                if exit_code == 0 {
                    Ok(output)
                } else {
                    Ok(format!("Exit code: {}\n{}", exit_code, output))
                }
            }

            "execute_code" | "code_execute" => {
                let code = args["code"]
                    .as_str()
                    .ok_or_else(|| ChatError::ToolError("Missing 'code' argument".to_string()))?;
                let language = args["language"].as_str().ok_or_else(|| {
                    ChatError::ToolError("Missing 'language' argument".to_string())
                })?;
                let timeout = args["timeout"].as_i64().unwrap_or(60) as i32;

                let result = self
                    .shell_executor
                    .execute_code(code, language, timeout)
                    .await
                    .map_err(|e| ChatError::ToolError(e.to_string()))?;

                let mut stdout = String::new();
                let mut stderr = String::new();
                let mut exit_code = 0;

                for event in result {
                    match event {
                        crate::shell::CodeOutputEvent::Stdout(line) => stdout.push_str(&line),
                        crate::shell::CodeOutputEvent::Stderr(line) => stderr.push_str(&line),
                        crate::shell::CodeOutputEvent::Complete {
                            exit_code: code, ..
                        } => {
                            exit_code = code;
                        }
                    }
                }

                let mut output = String::new();
                if !stdout.is_empty() {
                    output.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !output.is_empty() {
                        output.push_str("\n--- stderr ---\n");
                    }
                    output.push_str(&stderr);
                }

                if exit_code != 0 {
                    output = format!("Exit code: {}\n{}", exit_code, output);
                }

                Ok(output)
            }

            _ => Err(ChatError::ToolError(format!(
                "Unknown tool: {}",
                tool_call.name
            ))),
        }
    }
}

impl Default for ChatHandler {
    fn default() -> Self {
        Self::new()
    }
}
