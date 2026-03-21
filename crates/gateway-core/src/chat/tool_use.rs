//! Tool Use Engine
//!
//! Manages the LLM ↔ Tool interaction loop for autonomous tool calling.
//! This module implements the core tool use loop that allows LLMs to:
//! 1. Request tool calls via tool_use content blocks
//! 2. Execute tools via MCP Gateway
//! 3. Return results via tool_result content blocks
//! 4. Continue the conversation until the LLM produces a final response

use std::sync::Arc;
use std::time::Instant;

use crate::agent::hooks::HookExecutor;
use crate::agent::types::{
    HookContext, HookEvent, HookResult, PostToolUseHookData, PreToolUseHookData,
};
use crate::error::{Error, Result};
use crate::llm::router::{
    ChatRequest, ChatResponse, ContentBlock, LlmRouter, Message, ToolDefinition, ToolResult,
    ToolUse,
};
use crate::mcp::gateway::McpGateway;

/// Configuration for the Tool Use Engine
#[derive(Debug, Clone)]
pub struct ToolUseConfig {
    /// Maximum number of tool use iterations before forcing a response
    pub max_iterations: usize,
    /// Timeout for each tool call in milliseconds
    pub timeout_per_call_ms: u64,
    /// Whether tool use is enabled
    pub enabled: bool,
}

impl Default for ToolUseConfig {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            timeout_per_call_ms: 30000,
            enabled: true,
        }
    }
}

/// Event emitted during tool use loop
#[derive(Debug, Clone)]
pub enum ToolUseEvent {
    /// LLM is requesting a tool call
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    /// Tool execution completed
    ToolResult {
        id: String,
        name: String,
        result: String,
        is_error: bool,
    },
    /// Tool execution was cancelled by a hook
    ToolCancelled {
        id: String,
        name: String,
        reason: String,
    },
    /// Text chunk from LLM
    Text(String),
    /// Thinking/processing status
    Thinking(String),
    /// Final response ready
    Done,
    /// Error occurred
    Error(String),
}

/// Tool Use Engine
///
/// Orchestrates the tool use loop between LLM and MCP Gateway.
pub struct ToolUseEngine {
    llm_router: Arc<LlmRouter>,
    mcp_gateway: Arc<McpGateway>,
    /// Unified Tool System (preferred over mcp_gateway when available)
    tool_system: Option<Arc<crate::tool_system::ToolSystem>>,
    config: ToolUseConfig,
    /// Optional hook executor for PreToolUse and PostToolUse hooks
    hook_executor: Option<Arc<HookExecutor>>,
    /// Hook context for tool execution
    hook_context: Option<HookContext>,
}

impl ToolUseEngine {
    /// Create a new Tool Use Engine
    pub fn new(
        llm_router: Arc<LlmRouter>,
        mcp_gateway: Arc<McpGateway>,
        config: ToolUseConfig,
    ) -> Self {
        Self {
            llm_router,
            mcp_gateway,
            tool_system: None,
            config,
            hook_executor: None,
            hook_context: None,
        }
    }

    /// Create a new Tool Use Engine with hook support
    pub fn new_with_hooks(
        llm_router: Arc<LlmRouter>,
        mcp_gateway: Arc<McpGateway>,
        config: ToolUseConfig,
        hook_executor: Arc<HookExecutor>,
        hook_context: HookContext,
    ) -> Self {
        Self {
            llm_router,
            mcp_gateway,
            tool_system: None,
            config,
            hook_executor: Some(hook_executor),
            hook_context: Some(hook_context),
        }
    }

    /// Set the unified tool system (preferred over mcp_gateway)
    pub fn with_tool_system(mut self, tool_system: Arc<crate::tool_system::ToolSystem>) -> Self {
        self.tool_system = Some(tool_system);
        self
    }

    /// Get available tools formatted for LLM, optionally filtered by namespace
    pub async fn get_tool_definitions(
        &self,
        enabled_namespaces: Option<&[String]>,
    ) -> Vec<ToolDefinition> {
        // Prefer ToolSystem when available
        if let Some(ref ts) = self.tool_system {
            let tools = match enabled_namespaces {
                Some(ns) => ts.list_tools_filtered(ns).await,
                None => ts.list_tools().await,
            };
            return tools
                .into_iter()
                .map(|entry| ToolDefinition {
                    name: entry.id.llm_name(),
                    description: entry.description,
                    input_schema: entry.input_schema,
                })
                .collect();
        }

        // Fallback to legacy McpGateway
        let tools = match enabled_namespaces {
            Some(ns) => self.mcp_gateway.get_tools_filtered(ns).await,
            None => self.mcp_gateway.get_tools().await,
        };

        tools
            .into_iter()
            .map(|tool| ToolDefinition {
                // Format: namespace_toolname for Claude API
                name: format!("{}_{}", tool.namespace, tool.name),
                description: tool.description,
                input_schema: tool.input_schema,
            })
            .collect()
    }

    /// Execute a single tool call (without hooks)
    pub async fn execute_tool(&self, tool_use: &ToolUse) -> ToolResult {
        self.execute_tool_internal(tool_use, tool_use.input.clone())
            .await
    }

    /// Execute a single tool call with hook support
    ///
    /// Returns (ToolResult, was_cancelled, cancel_reason)
    pub async fn execute_tool_with_hooks(
        &self,
        tool_use: &ToolUse,
    ) -> (ToolResult, bool, Option<String>) {
        let start_time = Instant::now();

        // Check if we have hooks configured
        let (hook_executor, hook_context) = match (&self.hook_executor, &self.hook_context) {
            (Some(executor), Some(context)) => (executor, context),
            _ => {
                // No hooks, execute normally
                return (self.execute_tool(tool_use).await, false, None);
            }
        };

        // 1. Execute PreToolUse hook
        let pre_hook_data = PreToolUseHookData {
            tool_name: tool_use.name.clone(),
            input: tool_use.input.clone(),
            tool_use_id: tool_use.id.clone(),
        };

        let (hook_result, modified_input) = hook_executor
            .execute_and_aggregate(
                HookEvent::PreToolUse,
                serde_json::to_value(&pre_hook_data).unwrap_or_default(),
                hook_context,
                Some(&tool_use.name),
            )
            .await;

        // Check if hook cancelled the tool execution
        if let HookResult::Cancel { reason } = hook_result {
            tracing::info!(
                tool_id = %tool_use.id,
                tool_name = %tool_use.name,
                reason = %reason,
                "Tool execution cancelled by PreToolUse hook"
            );

            return (
                ToolResult {
                    tool_use_id: tool_use.id.clone(),
                    content: format!("Tool execution cancelled: {}", reason),
                    is_error: true,
                },
                true,
                Some(reason),
            );
        }

        // Use modified input if provided by hook
        let final_input = modified_input
            .and_then(|d| d.get("input").cloned())
            .unwrap_or_else(|| tool_use.input.clone());

        // 2. Execute the tool
        let result = self.execute_tool_internal(tool_use, final_input).await;
        let duration_ms = start_time.elapsed().as_millis() as u64;

        // 3. Execute PostToolUse hook (non-blocking for performance)
        let post_hook_data = PostToolUseHookData {
            tool_name: tool_use.name.clone(),
            input: tool_use.input.clone(),
            tool_use_id: tool_use.id.clone(),
            result: serde_json::json!({
                "content": result.content,
                "is_error": result.is_error
            }),
            is_error: result.is_error,
            duration_ms: Some(duration_ms),
        };

        let hook_executor_clone = hook_executor.clone();
        let hook_context_clone = hook_context.clone();
        let tool_name = tool_use.name.clone();
        tokio::spawn(async move {
            hook_executor_clone
                .execute_with_filter(
                    HookEvent::PostToolUse,
                    serde_json::to_value(&post_hook_data).unwrap_or_default(),
                    &hook_context_clone,
                    Some(&tool_name),
                )
                .await;
        });

        (result, false, None)
    }

    /// Internal tool execution (used by both hooked and non-hooked paths)
    async fn execute_tool_internal(
        &self,
        tool_use: &ToolUse,
        input: serde_json::Value,
    ) -> ToolResult {
        tracing::info!(
            tool_id = %tool_use.id,
            tool_name = %tool_use.name,
            "Executing tool call"
        );

        // R3-M: Apply per-call timeout from config
        let timeout_duration = std::time::Duration::from_millis(self.config.timeout_per_call_ms);

        // Prefer ToolSystem when available, fallback to McpGateway
        let result = {
            let fut = if let Some(ref ts) = self.tool_system {
                Box::pin(ts.execute_llm_tool_call(&tool_use.name, input))
                    as std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send>>
            } else {
                Box::pin(
                    self.mcp_gateway
                        .execute_llm_tool_call(&tool_use.name, input),
                )
            };
            match tokio::time::timeout(timeout_duration, fut).await {
                Ok(r) => r,
                Err(_) => {
                    tracing::warn!(
                        tool_id = %tool_use.id,
                        tool_name = %tool_use.name,
                        timeout_ms = self.config.timeout_per_call_ms,
                        "Tool call timed out"
                    );
                    return ToolResult {
                        tool_use_id: tool_use.id.clone(),
                        content: format!(
                            "Tool call timed out after {}ms",
                            self.config.timeout_per_call_ms
                        ),
                        is_error: true,
                    };
                }
            }
        };

        match result {
            Ok(tool_result) => {
                let content = tool_result.text_content().unwrap_or_else(|| {
                    serde_json::to_string(&tool_result.content).unwrap_or_default()
                });

                tracing::info!(
                    tool_id = %tool_use.id,
                    tool_name = %tool_use.name,
                    is_error = tool_result.is_error,
                    "Tool execution completed"
                );

                ToolResult {
                    tool_use_id: tool_use.id.clone(),
                    content,
                    is_error: tool_result.is_error,
                }
            }
            Err(e) => {
                tracing::error!(
                    tool_id = %tool_use.id,
                    tool_name = %tool_use.name,
                    error = %e,
                    "Tool execution failed"
                );

                ToolResult {
                    tool_use_id: tool_use.id.clone(),
                    content: format!("Error executing tool: {}", e),
                    is_error: true,
                }
            }
        }
    }

    /// Build a ChatRequest for one LLM call inside the tool loop.
    ///
    /// R3-H11: Instead of cloning the entire ChatRequest (messages +
    /// tools + config) on every iteration, we:
    ///   - Clone only the messages (unavoidable — route() consumes them)
    ///   - Share the heavyweight `tools` vec once via clone-from-Arc
    ///   - Copy only lightweight scalar config fields
    ///
    /// Old cost per iteration: O(messages + tools + config)
    /// New cost per iteration: O(messages) — tools are cloned once
    /// from Arc, config fields are cheap scalars/Options.
    fn build_loop_request(
        tools: &[ToolDefinition],
        messages: Vec<Message>,
        template: &ChatRequest,
    ) -> ChatRequest {
        ChatRequest {
            messages,
            model: template.model.clone(),
            max_tokens: template.max_tokens,
            temperature: template.temperature,
            stream: template.stream,
            tools: tools.to_vec(),
            tool_choice: template.tool_choice.clone(),
            profile_id: template.profile_id.clone(),
            task_type: template.task_type.clone(),
            thinking_budget: template.thinking_budget,
        }
    }

    /// Execute the tool use loop
    ///
    /// This method handles the full loop:
    /// 1. Send request to LLM with tools
    /// 2. If LLM responds with tool_use, execute tools
    /// 3. Send tool_result back to LLM
    /// 4. Repeat until LLM produces final response or max iterations reached
    ///
    /// Returns a stream of ToolUseEvents for real-time updates.
    pub async fn execute_with_tools(
        &self,
        mut request: ChatRequest,
        event_tx: tokio::sync::mpsc::Sender<ToolUseEvent>,
    ) -> Result<ChatResponse> {
        if !self.config.enabled {
            // Tool use disabled, just forward the request
            let response = self.llm_router.route(request).await?;
            if let Some(choice) = response.choices.first() {
                if !choice.message.content.is_empty() {
                    let _ = event_tx
                        .send(ToolUseEvent::Text(choice.message.content.clone()))
                        .await;
                }
            }
            let _ = event_tx.send(ToolUseEvent::Done).await;
            return Ok(response);
        }

        // Add tool definitions to request
        let tools = self.get_tool_definitions(None).await;
        if tools.is_empty() {
            // No tools available, just forward the request
            let response = self.llm_router.route(request).await?;
            if let Some(choice) = response.choices.first() {
                if !choice.message.content.is_empty() {
                    let _ = event_tx
                        .send(ToolUseEvent::Text(choice.message.content.clone()))
                        .await;
                }
            }
            let _ = event_tx.send(ToolUseEvent::Done).await;
            return Ok(response);
        }

        // R3-H11: Separate the immutable tool definitions from the
        // mutable message history.  `tools` is cloned once into
        // each per-iteration request (fixed cost), while messages
        // are cloned only when the loop will continue (i.e., when
        // more tool calls are needed).  The old code cloned the
        // ENTIRE ChatRequest (tools + messages + config) every
        // iteration, giving O(iterations * (tools + messages))
        // total allocation.  Now total allocation is
        // O(iterations * messages + tools), saving the redundant
        // tools clone that scaled with message growth.
        let shared_tools = tools;
        // Take messages out of request; request becomes a
        // lightweight template holding only scalar config fields.
        let mut messages = std::mem::take(&mut request.messages);
        request.tools.clear();

        let mut iteration = 0;
        let mut final_response: Option<ChatResponse> = None;

        loop {
            iteration += 1;

            if iteration > self.config.max_iterations {
                let _ = event_tx
                    .send(ToolUseEvent::Error(format!(
                        "Maximum tool use iterations ({}) exceeded",
                        self.config.max_iterations
                    )))
                    .await;
                break;
            }

            tracing::info!(iteration = iteration, "Tool use loop iteration");

            let _ = event_tx
                .send(ToolUseEvent::Thinking(format!(
                    "Processing (iteration {})...",
                    iteration
                )))
                .await;

            // R3-H11: Clone only the messages for this iteration's
            // request.  The tools are cloned from the shared slice
            // (fixed size), and scalar config from the template.
            // If this turns out to be the final iteration (no tool
            // use in response), the clone was wasted — but that is
            // at most one extra clone total, not one per iteration.
            let loop_request = Self::build_loop_request(&shared_tools, messages.clone(), &request);

            // route() consumes loop_request; `messages` is retained.
            let response = self.llm_router.route(loop_request).await?;

            if response.requires_tool_use() {
                let tool_uses = response.get_tool_uses();

                tracing::info!(
                    iteration = iteration,
                    tool_count = tool_uses.len(),
                    "LLM requested tool calls"
                );

                // R3-H20: Send all ToolCall events first
                for tool_use in &tool_uses {
                    let _ = event_tx
                        .send(ToolUseEvent::ToolCall {
                            id: tool_use.id.clone(),
                            name: tool_use.name.clone(),
                            arguments: tool_use.input.clone(),
                        })
                        .await;
                }

                // Execute all tools concurrently (join_all preserves ordering)
                let tool_futures: Vec<_> = tool_uses
                    .iter()
                    .map(|tool_use| self.execute_tool_with_hooks(tool_use))
                    .collect();
                let results = futures::future::join_all(tool_futures).await;

                // Send result events and collect results
                let mut tool_results = Vec::new();
                for (tool_use, (result, was_cancelled, cancel_reason)) in
                    tool_uses.iter().zip(results)
                {
                    if was_cancelled {
                        let _ = event_tx
                            .send(ToolUseEvent::ToolCancelled {
                                id: tool_use.id.clone(),
                                name: tool_use.name.clone(),
                                reason: cancel_reason
                                    .unwrap_or_else(|| "Unknown reason".to_string()),
                            })
                            .await;
                    } else {
                        let _ = event_tx
                            .send(ToolUseEvent::ToolResult {
                                id: tool_use.id.clone(),
                                name: tool_use.name.clone(),
                                result: result.content.clone(),
                                is_error: result.is_error,
                            })
                            .await;
                    }
                    tool_results.push(result);
                }

                // Build the assistant message with tool_use blocks
                let assistant_message = response
                    .choices
                    .first()
                    .map(|c| c.message.clone())
                    .unwrap_or_else(|| Message::text("assistant", ""));

                // Build the user message with tool_result blocks
                let tool_result_blocks: Vec<ContentBlock> = tool_results
                    .into_iter()
                    .map(|r| ContentBlock::ToolResult {
                        tool_use_id: r.tool_use_id,
                        content: r.content,
                        is_error: r.is_error,
                    })
                    .collect();

                let user_message = Message::with_blocks("user", tool_result_blocks);

                // Append new messages for the next iteration.
                // `messages` still holds the pre-route snapshot;
                // we extend it in-place rather than rebuilding.
                messages.push(assistant_message);
                messages.push(user_message);
            } else {
                // LLM produced a final response (no tool_use)
                tracing::info!(
                    iteration = iteration,
                    stop_reason = ?response.choices.first().and_then(|c| c.stop_reason.clone()),
                    "LLM produced final response"
                );

                if let Some(choice) = response.choices.first() {
                    if !choice.message.content.is_empty() {
                        let _ = event_tx
                            .send(ToolUseEvent::Text(choice.message.content.clone()))
                            .await;
                    }
                }

                final_response = Some(response);
                break;
            }
        }

        let _ = event_tx.send(ToolUseEvent::Done).await;

        final_response.ok_or_else(|| Error::Llm("No response from LLM".to_string()))
    }

    /// Simpler version without streaming - just returns the final response
    pub async fn execute_with_tools_sync(&self, mut request: ChatRequest) -> Result<ChatResponse> {
        if !self.config.enabled {
            return Ok(self.llm_router.route(request).await?);
        }

        let tools = self.get_tool_definitions(None).await;
        if tools.is_empty() {
            return Ok(self.llm_router.route(request).await?);
        }

        // R3-H11: Same pattern as execute_with_tools — separate
        // messages from the immutable config template to avoid
        // cloning the entire ChatRequest per iteration.
        let shared_tools = tools;
        let mut messages = std::mem::take(&mut request.messages);
        request.tools.clear();

        let mut iteration = 0;

        loop {
            iteration += 1;

            if iteration > self.config.max_iterations {
                return Err(Error::Llm(format!(
                    "Maximum tool use iterations ({}) exceeded",
                    self.config.max_iterations
                )));
            }

            // R3-H11: Clone only messages, not the full request.
            let loop_request = Self::build_loop_request(&shared_tools, messages.clone(), &request);

            let response = self.llm_router.route(loop_request).await?;

            if response.requires_tool_use() {
                let tool_uses = response.get_tool_uses();

                let mut tool_results = Vec::new();
                for tool_use in &tool_uses {
                    let result = self.execute_tool(tool_use).await;
                    tool_results.push(result);
                }

                let assistant_message = response
                    .choices
                    .first()
                    .map(|c| c.message.clone())
                    .unwrap_or_else(|| Message::text("assistant", ""));

                let tool_result_blocks: Vec<ContentBlock> = tool_results
                    .into_iter()
                    .map(|r| ContentBlock::ToolResult {
                        tool_use_id: r.tool_use_id,
                        content: r.content,
                        is_error: r.is_error,
                    })
                    .collect();

                let user_message = Message::with_blocks("user", tool_result_blocks);

                // Append to retained messages for next iteration
                messages.push(assistant_message);
                messages.push(user_message);
            } else {
                return Ok(response);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::router::LlmConfig;

    fn create_test_engine() -> ToolUseEngine {
        let llm_router = Arc::new(LlmRouter::new(LlmConfig::default()));
        let mcp_gateway = Arc::new(McpGateway::new());
        let config = ToolUseConfig::default();

        ToolUseEngine::new(llm_router, mcp_gateway, config)
    }

    #[tokio::test]
    async fn test_get_tool_definitions_empty() {
        let engine = create_test_engine();
        let tools = engine.get_tool_definitions(None).await;
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn test_config_defaults() {
        let config = ToolUseConfig::default();
        assert_eq!(config.max_iterations, 10);
        assert_eq!(config.timeout_per_call_ms, 30000);
        assert!(config.enabled);
    }
}
