//! Chat endpoints
//!
//! Unified chat API using AgentRunner for all requests.
//! Provides full agent capabilities (tools, hooks, permissions) through standard chat API.

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, KeepAliveStream, Sse},
    routing::{get, post},
    Json, Router,
};
use futures::stream::{Stream, StreamExt};
use gateway_core::agent::{AgentLoop, AgentMessage, ContentBlock};
use gateway_core::chat::streaming::StreamEvent;
use gateway_core::chat::{NewConversation, NewMessage};
use gateway_core::llm::{ChatResponse, Message};
use gateway_core::memory::{
    Confidence, MemoryCategory, MemoryEntry, MemoryPattern, MemorySource, PatternType,
    UnifiedMemoryStore,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

use super::settings::get_enabled_namespaces_from_settings;
use crate::{error::ApiError, middleware::auth::AuthContext, state::AppState};

/// Extract provider name from model identifier
/// Used for billing/usage tracking
fn extract_provider_from_model(model: &str) -> &'static str {
    let model_lower = model.to_lowercase();
    if model_lower.starts_with("claude") || model_lower.contains("anthropic") {
        "anthropic"
    } else if model_lower.starts_with("gpt") || model_lower.contains("openai") {
        "openai"
    } else if model_lower.starts_with("gemini") || model_lower.contains("google") {
        "google"
    } else if model_lower.starts_with("qwen") || model_lower.contains("alibaba") {
        "qwen"
    } else if model_lower.starts_with("deepseek") {
        "deepseek"
    } else if model_lower.starts_with("mistral") || model_lower.starts_with("mixtral") {
        "mistral"
    } else if model_lower.starts_with("llama") || model_lower.starts_with("meta") {
        "meta"
    } else {
        "unknown"
    }
}

/// Type alias for boxed SSE stream
type BoxedSseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

/// Create the chat routes
pub fn routes() -> Router<AppState> {
    let router = Router::new()
        .route("/", post(chat))
        .route("/completions", post(chat))
        .route("/stream", post(chat_stream))
        .route("/conversations", get(list_conversations))
        .route("/conversations", post(create_conversation))
        .route("/conversations/{id}", get(get_conversation))
        .route("/conversations/{id}/messages", get(get_messages));

    #[cfg(feature = "collaboration")]
    let router = router
        .route("/plan-approval", post(submit_plan_approval))
        .route("/clarification", post(submit_clarification))
        .route("/prd-approval", post(submit_prd_approval));

    router
}

/// Chat request wrapper for API
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ApiChatRequest {
    pub messages: Vec<ApiMessage>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub stream: bool,
    /// Profile ID for model routing (uses default if not specified)
    #[serde(default)]
    pub profile_id: Option<String>,
    /// Task type hint for task-based routing (e.g., "code", "analysis", "chat")
    #[serde(default)]
    pub task_type: Option<String>,
    /// Conversation ID for session persistence (creates new if not specified)
    #[serde(default)]
    pub conversation_id: Option<Uuid>,
    /// Collaboration mode for multi-agent execution.
    ///
    /// When set, the request is routed through graph-based collaboration
    /// (Expert, Swarm, etc.) instead of direct single-agent execution.
    /// The response is non-streaming regardless of the `stream` field.
    #[cfg(feature = "collaboration")]
    #[serde(default)]
    pub collaboration_mode: Option<gateway_core::collaboration::CollaborationMode>,
}

/// Streaming chat request
#[derive(Debug, Deserialize)]
pub struct StreamChatRequest {
    pub message: String,
    #[serde(default)]
    pub conversation_id: Option<Uuid>,
    /// Profile ID for model routing (uses default if not specified)
    #[serde(default)]
    pub profile_id: Option<String>,
    /// Task type hint for task-based routing (e.g., "code", "analysis", "chat")
    #[serde(default)]
    pub task_type: Option<String>,
    /// Collaboration mode override (e.g., "Direct", "Swarm", "PlanExecute", "Expert").
    /// When set, routes through graph-based collaboration instead of single-agent execution.
    #[serde(default)]
    pub collaboration_mode: Option<String>,
    /// Active plugin bundle names for this session.
    /// When set, the agent's system prompt and tool namespaces are augmented
    /// with the resolved bundle configuration.
    #[serde(default)]
    pub active_plugins: Option<Vec<String>>,
}

/// Plan approval request body.
#[cfg(feature = "collaboration")]
#[derive(Debug, Deserialize)]
pub struct PlanApprovalRequest {
    pub request_id: Uuid,
    pub decision: gateway_core::collaboration::approval::PlanApprovalDecision,
}

/// Plan approval response.
#[cfg(feature = "collaboration")]
#[derive(Debug, Serialize)]
pub struct PlanApprovalResponse {
    pub success: bool,
    pub message: String,
}

/// Clarification answers request body (A43).
#[cfg(feature = "collaboration")]
#[derive(Debug, Deserialize)]
pub struct ClarificationRequest {
    pub request_id: Uuid,
    pub answers: std::collections::HashMap<u32, String>,
    #[serde(default)]
    pub skip_remaining: bool,
}

/// Clarification response.
#[cfg(feature = "collaboration")]
#[derive(Debug, Serialize)]
pub struct ClarificationApiResponse {
    pub success: bool,
    pub message: String,
}

/// PRD approval request body (A43).
#[cfg(feature = "collaboration")]
#[derive(Debug, Deserialize)]
pub struct PrdApprovalRequest {
    pub request_id: Uuid,
    pub decision: gateway_core::collaboration::prd::PrdApprovalDecision,
}

/// PRD approval response.
#[cfg(feature = "collaboration")]
#[derive(Debug, Serialize)]
pub struct PrdApprovalApiResponse {
    pub success: bool,
    pub message: String,
}

/// API message format
#[derive(Debug, Deserialize, Serialize)]
pub struct ApiMessage {
    pub role: String,
    pub content: String,
}

impl From<ApiMessage> for Message {
    fn from(m: ApiMessage) -> Self {
        Message {
            role: m.role,
            content: m.content,
            ..Default::default()
        }
    }
}

impl From<Message> for ApiMessage {
    fn from(m: Message) -> Self {
        ApiMessage {
            role: m.role,
            content: m.content,
        }
    }
}

/// API chat response
#[derive(Debug, Serialize)]
pub struct ApiChatResponse {
    pub id: String,
    pub object: String,
    pub model: String,
    pub choices: Vec<ApiChoice>,
    pub usage: ApiUsage,
}

#[derive(Debug, Serialize)]
pub struct ApiChoice {
    pub index: i32,
    pub message: ApiMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize)]
pub struct ApiUsage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

impl From<ChatResponse> for ApiChatResponse {
    fn from(r: ChatResponse) -> Self {
        ApiChatResponse {
            id: r.id,
            object: "chat.completion".to_string(),
            model: r.model,
            choices: r
                .choices
                .into_iter()
                .map(|c| ApiChoice {
                    index: c.index,
                    message: c.message.into(),
                    finish_reason: c.finish_reason,
                })
                .collect(),
            usage: ApiUsage {
                prompt_tokens: r.usage.prompt_tokens,
                completion_tokens: r.usage.completion_tokens,
                total_tokens: r.usage.total_tokens,
            },
        }
    }
}

/// Chat endpoint handler (non-streaming)
///
/// Now uses unified AgentRunner for full agent capabilities.
/// Sessions are persisted across requests using conversation_id.
///
/// When `collaboration_mode` is set (requires `collaboration` feature),
/// the request is routed through graph-based multi-agent execution.
pub async fn chat(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<AuthContext>,
    Json(request): Json<ApiChatRequest>,
) -> Result<Json<ApiChatResponse>, ApiError> {
    // Auth context is used for billing metering when feature is enabled
    #[cfg(feature = "billing")]
    let auth = _auth;

    // Use provided conversation_id or create new one
    let session_id = request.conversation_id.unwrap_or_else(Uuid::new_v4);
    let session_id_str = session_id.to_string();

    tracing::info!(
        session_id = %session_id,
        message_count = request.messages.len(),
        model = ?request.model,
        profile_id = ?request.profile_id,
        task_type = ?request.task_type,
        is_continuing = request.conversation_id.is_some(),
        "Processing chat request via AgentRunner"
    );

    // Route through collaboration if collaboration_mode is set
    #[cfg(feature = "collaboration")]
    if let Some(collab_mode) = request.collaboration_mode {
        let prompt = build_prompt_from_messages(&request.messages);
        tracing::info!(
            session_id = %session_id,
            mode = ?collab_mode,
            "Routing chat through collaboration mode"
        );

        let agent_factory = &state.agent_factory;
        let result = agent_factory
            .execute_with_collaboration(&prompt, Some(collab_mode))
            .await
            .map_err(|e| ApiError::internal(format!("Collaboration execution failed: {}", e)))?;

        return Ok(Json(ApiChatResponse {
            id: format!("chatcmpl-{}", session_id),
            object: "chat.completion".to_string(),
            model: result
                .metadata
                .models_used
                .last()
                .cloned()
                .unwrap_or_else(|| "unknown".into()),
            choices: vec![ApiChoice {
                index: 0,
                message: ApiMessage {
                    role: "assistant".to_string(),
                    content: result.response,
                },
                finish_reason: "stop".to_string(),
            }],
            usage: ApiUsage {
                prompt_tokens: 0,
                completion_tokens: result.metadata.total_tokens as i32,
                total_tokens: result.metadata.total_tokens as i32,
            },
        }));
    }

    // Auto-route to collaboration mode (A24)
    // Priority: LLM classifier → keyword fallback → Direct
    // Only on NEW conversations (first message) — continuing conversations use AgentRunner
    #[cfg(feature = "collaboration")]
    if request.conversation_id.is_none() {
        let prompt = build_prompt_from_messages(&request.messages);

        // Step 1: Determine collaboration mode via LLM classifier or keyword fallback
        let suggested_mode = {
            #[allow(unused_mut)]
            let mut mode = gateway_core::collaboration::CollaborationMode::Direct;

            // Try LLM classifier first (Phase 3)
            if let Some(ref classifier) = state.task_classifier {
                match classifier.classify(&prompt).await {
                    Some(result) => {
                        tracing::info!(
                            session_id = %session_id,
                            category = ?result.category,
                            confidence = result.confidence,
                            reasoning = %result.reasoning,
                            "LLM task classification complete"
                        );
                        mode = gateway_core::agent::task_classifier::TaskClassifier::to_collaboration_mode(&result);
                    }
                    None => {
                        // LLM failed → fallback to keyword-based
                        tracing::warn!(
                            session_id = %session_id,
                            "LLM classifier failed, falling back to keyword detection"
                        );
                        if let Some(ref selector) = state.auto_mode_selector {
                            mode = selector.suggest_collaboration_mode_for_text(&prompt);
                        }
                    }
                }
            } else if let Some(ref selector) = state.auto_mode_selector {
                // No classifier configured → use keyword-based (Phase 1 behavior)
                mode = selector.suggest_collaboration_mode_for_text(&prompt);
            }

            mode
        };

        // Step 2: Route based on classification
        if !matches!(
            suggested_mode,
            gateway_core::collaboration::CollaborationMode::Direct
        ) {
            tracing::info!(
                session_id = %session_id,
                mode = ?suggested_mode,
                prompt_preview = %prompt.chars().take(80).collect::<String>(),
                "Auto-routing chat to collaboration mode"
            );

            let agent_factory = &state.agent_factory;
            let result = agent_factory
                .execute_with_collaboration(&prompt, Some(suggested_mode))
                .await
                .map_err(|e| {
                    ApiError::internal(format!("Auto-routed collaboration failed: {}", e))
                })?;

            return Ok(Json(ApiChatResponse {
                id: format!("chatcmpl-{}", session_id),
                object: "chat.completion".to_string(),
                model: result
                    .metadata
                    .models_used
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "unknown".into()),
                choices: vec![ApiChoice {
                    index: 0,
                    message: ApiMessage {
                        role: "assistant".to_string(),
                        content: result.response,
                    },
                    finish_reason: "stop".to_string(),
                }],
                usage: ApiUsage {
                    prompt_tokens: 0,
                    completion_tokens: result.metadata.total_tokens as i32,
                    total_tokens: result.metadata.total_tokens as i32,
                },
            }));
        } else {
            tracing::debug!(
                session_id = %session_id,
                "Auto-routing: Direct mode, using AgentRunner"
            );
        }
    }

    // Use the singleton agent factory from state (preserves sessions across requests)
    let agent_factory = &state.agent_factory;

    // Get or create agent for this session with profile-based routing
    let agent_lock = agent_factory
        .get_or_create_with_profile(
            &session_id_str,
            request.profile_id.clone(),
            request.task_type.clone(),
        )
        .await;

    // Build prompt from messages (only extracts latest user message)
    let prompt = build_prompt_from_messages(&request.messages);

    // Execute via AgentRunner
    let mut response_content = String::new();
    let mut model = String::from("claude-sonnet-4-6");
    let mut input_tokens: u32 = 0;
    let mut output_tokens: u32 = 0;

    {
        let mut stream = {
            let mut agent = agent_lock.write().await;
            agent.query(&prompt).await
            // write lock dropped — stream captures context by move (A41 fix)
        };

        while let Some(msg_result) = stream.next().await {
            match msg_result {
                Ok(msg) => {
                    match &msg {
                        AgentMessage::Assistant(assistant_msg) => {
                            // Extract text content
                            for block in &assistant_msg.content {
                                if let Some(text) = block.as_text() {
                                    response_content.push_str(text);
                                }
                            }
                            model = assistant_msg.model.clone();
                        }
                        AgentMessage::Result(result_msg) => {
                            // Extract usage from result
                            if let Some(usage) = &result_msg.usage {
                                input_tokens = usage.input_tokens;
                                output_tokens = usage.output_tokens;
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Agent query error");
                    return Err(ApiError::internal(format!("Agent error: {}", e)));
                }
            }
        }
    }

    // NOTE: We intentionally DO NOT remove the session after the request
    // This allows the agent to maintain context across multiple requests
    // with the same conversation_id

    tracing::info!(
        session_id = %session_id,
        model = %model,
        tokens = input_tokens + output_tokens,
        "Chat request completed via AgentRunner"
    );

    // Record usage to billing-core metering (feature-gated)
    #[cfg(feature = "billing")]
    if input_tokens > 0 || output_tokens > 0 {
        let user_id = auth.user_id;
        let metering_service = state.metering_service.clone();
        let model_for_billing = model.clone();
        tokio::spawn(async move {
            match metering_service
                .record_llm_usage(
                    user_id,
                    &model_for_billing,
                    input_tokens as u64,
                    output_tokens as u64,
                    Some(serde_json::json!({"session_id": session_id.to_string()})),
                )
                .await
            {
                Ok(result) => {
                    tracing::debug!(
                        user_id = %user_id,
                        model = %model_for_billing,
                        cost_mpt = result.cost_mpt,
                        balance_mpt = result.balance_mpt,
                        "Non-streaming chat metered via billing-core"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        user_id = %user_id,
                        error = %e,
                        "Non-streaming chat billing-core metering failed (non-blocking)"
                    );
                }
            }
        });
    }

    // Build OpenAI-compatible response (includes session_id for continuation)
    Ok(Json(ApiChatResponse {
        id: format!("chatcmpl-{}", session_id),
        object: "chat.completion".to_string(),
        model,
        choices: vec![ApiChoice {
            index: 0,
            message: ApiMessage {
                role: "assistant".to_string(),
                content: response_content,
            },
            finish_reason: "stop".to_string(),
        }],
        usage: ApiUsage {
            prompt_tokens: input_tokens as i32,
            completion_tokens: output_tokens as i32,
            total_tokens: (input_tokens + output_tokens) as i32,
        },
    }))
}

/// Build a prompt string from API messages
fn build_prompt_from_messages(messages: &[ApiMessage]) -> String {
    // Get the last user message as the prompt
    // Previous messages will be handled by the agent's session state
    messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .unwrap_or_default()
}

/// Convert a string collaboration mode name to a proper CollaborationMode with defaults.
/// Returns None for "Direct" or "Auto" (these use the standard AgentRunner path).
#[cfg(feature = "collaboration")]
fn parse_collaboration_mode(
    mode_str: &str,
) -> Option<gateway_core::collaboration::CollaborationMode> {
    use gateway_core::collaboration::CollaborationMode;
    match mode_str {
        "Swarm" | "swarm" => Some(CollaborationMode::Swarm {
            initial_agent: "primary".into(),
            handoff_rules: vec![],
            agent_models: std::collections::HashMap::new(),
        }),
        "PlanExecute" | "planexecute" | "Plan" | "plan" => Some(CollaborationMode::PlanExecute),
        "Expert" | "expert" => Some(CollaborationMode::Expert {
            supervisor: "coordinator".into(),
            specialists: vec!["executor".into(), "reviewer".into()],
            supervisor_model: None,
            default_specialist_model: None,
            specialist_models: std::collections::HashMap::new(),
        }),
        _ => None, // "Direct", "Auto", or unknown → use AgentRunner
    }
}

/// Submit a plan approval decision (approve, reject, revise, or approve with edits).
///
/// Called by the frontend when the user has reviewed a plan generated by PlanExecute mode.
/// The decision is delivered to the waiting approval_gate node via oneshot channel.
#[cfg(feature = "collaboration")]
async fn submit_plan_approval(
    State(state): State<AppState>,
    Json(req): Json<PlanApprovalRequest>,
) -> Json<PlanApprovalResponse> {
    tracing::info!(
        request_id = %req.request_id,
        decision = ?std::mem::discriminant(&req.decision),
        "Plan approval decision received"
    );

    match state
        .pending_plan_approvals
        .complete(&req.request_id, req.decision)
    {
        Ok(()) => Json(PlanApprovalResponse {
            success: true,
            message: "Decision delivered".into(),
        }),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to deliver plan approval");
            Json(PlanApprovalResponse {
                success: false,
                message: e,
            })
        }
    }
}

/// Submit clarification answers (A43).
///
/// Called by the frontend when the user has answered clarifying questions
/// generated by the research planner pipeline.
#[cfg(feature = "collaboration")]
async fn submit_clarification(
    State(state): State<AppState>,
    Json(req): Json<ClarificationRequest>,
) -> Json<ClarificationApiResponse> {
    tracing::info!(
        request_id = %req.request_id,
        answers = req.answers.len(),
        skip_remaining = req.skip_remaining,
        "Clarification answers received"
    );

    let response = gateway_core::collaboration::ClarificationResponse {
        answers: req.answers,
        skip_remaining: req.skip_remaining,
    };

    match state
        .pending_clarifications
        .complete(&req.request_id, response)
    {
        Ok(()) => Json(ClarificationApiResponse {
            success: true,
            message: "Answers delivered".into(),
        }),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to deliver clarification answers");
            Json(ClarificationApiResponse {
                success: false,
                message: e,
            })
        }
    }
}

/// Submit a PRD approval decision (A43).
///
/// Called by the frontend when the user has reviewed a PRD generated by the
/// research planner pipeline. Supports approve (with chosen approach index),
/// revise (with feedback), or reject.
#[cfg(feature = "collaboration")]
async fn submit_prd_approval(
    State(state): State<AppState>,
    Json(req): Json<PrdApprovalRequest>,
) -> Json<PrdApprovalApiResponse> {
    tracing::info!(
        request_id = %req.request_id,
        decision = ?std::mem::discriminant(&req.decision),
        "PRD approval decision received"
    );

    match state
        .pending_prd_approvals
        .complete(&req.request_id, req.decision)
    {
        Ok(()) => Json(PrdApprovalApiResponse {
            success: true,
            message: "Decision delivered".into(),
        }),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to deliver PRD approval");
            Json(PrdApprovalApiResponse {
                success: false,
                message: e,
            })
        }
    }
}

/// Convert a `GraphStreamEvent` into a `StreamEvent::Custom` for SSE forwarding.
///
/// Maps each graph lifecycle event to a custom SSE event with `event_type` matching
/// the frontend store action names (e.g., `graph_started`, `graph_node_entered`).
#[cfg(feature = "collaboration")]
fn convert_graph_stream_event_to_sse(event: &gateway_core::graph::GraphStreamEvent) -> StreamEvent {
    use gateway_core::graph::GraphStreamEvent;

    match event {
        GraphStreamEvent::GraphStarted { execution_id } => StreamEvent::Custom {
            event_type: "graph_started".into(),
            data: serde_json::json!({ "execution_id": execution_id }),
        },
        GraphStreamEvent::NodeEntered {
            execution_id,
            node_id,
        } => StreamEvent::Custom {
            event_type: "graph_node_entered".into(),
            data: serde_json::json!({ "execution_id": execution_id, "node_id": node_id }),
        },
        GraphStreamEvent::NodeCompleted {
            execution_id,
            node_id,
            duration_ms,
        } => StreamEvent::Custom {
            event_type: "graph_node_completed".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "node_id": node_id,
                "duration_ms": duration_ms,
            }),
        },
        GraphStreamEvent::NodeFailed {
            execution_id,
            node_id,
            error,
        } => StreamEvent::Custom {
            event_type: "graph_node_failed".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "node_id": node_id,
                "error": error,
            }),
        },
        GraphStreamEvent::EdgeTraversed {
            execution_id,
            from,
            to,
            label,
        } => StreamEvent::Custom {
            event_type: "graph_edge_traversed".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "from_node": from,
                "to_node": to,
                "label": label,
            }),
        },
        GraphStreamEvent::GraphCompleted {
            execution_id,
            total_duration_ms,
        } => StreamEvent::Custom {
            event_type: "graph_completed".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "status": "completed",
                "duration_ms": total_duration_ms,
            }),
        },
        GraphStreamEvent::ParallelPartial {
            execution_id,
            node_id,
            succeeded,
            failed,
        } => StreamEvent::Custom {
            event_type: "graph_parallel_partial".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "node_id": node_id,
                "succeeded": succeeded,
                "failed": failed,
            }),
        },
        GraphStreamEvent::ParallelBranchFailed {
            execution_id,
            node_id,
            branch_id,
            error,
        } => StreamEvent::Custom {
            event_type: "graph_parallel_branch_failed".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "node_id": node_id,
                "branch_id": branch_id,
                "error": error,
            }),
        },
        GraphStreamEvent::DagWaveStarted {
            execution_id,
            wave_index,
            node_ids,
        } => StreamEvent::Custom {
            event_type: "graph_dag_wave_started".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "wave_index": wave_index,
                "node_ids": node_ids,
            }),
        },
        GraphStreamEvent::DagWaveCompleted {
            execution_id,
            wave_index,
            duration_ms,
        } => StreamEvent::Custom {
            event_type: "graph_dag_wave_completed".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "wave_index": wave_index,
                "duration_ms": duration_ms,
            }),
        },
        GraphStreamEvent::BudgetWarning {
            execution_id,
            node_id,
        } => StreamEvent::Custom {
            event_type: "graph_budget_warning".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "node_id": node_id,
            }),
        },
        GraphStreamEvent::BudgetExceeded {
            execution_id,
            node_id,
        } => StreamEvent::Custom {
            event_type: "graph_budget_exceeded".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "node_id": node_id,
            }),
        },
        // Content streaming events — convert to native SSE Thinking/Text/ToolCall
        GraphStreamEvent::NodeThinking { content, .. } => StreamEvent::thinking(content),
        GraphStreamEvent::NodeText { content, .. } => StreamEvent::text(content),
        GraphStreamEvent::NodeToolCall {
            tool_id, tool_name, ..
        } => StreamEvent::Custom {
            event_type: "tool_call".into(),
            data: serde_json::json!({
                "tool_id": tool_id,
                "tool_name": tool_name,
            }),
        },
        GraphStreamEvent::NodeToolResult { tool_id, .. } => StreamEvent::Custom {
            event_type: "tool_result".into(),
            data: serde_json::json!({
                "tool_id": tool_id,
            }),
        },
        GraphStreamEvent::PlanApprovalRequired {
            execution_id,
            request_id,
            goal,
            steps,
            success_criteria,
            timeout_seconds,
            risk_level,
            revision_round,
            max_revisions,
        } => StreamEvent::Custom {
            event_type: "plan_approval_required".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "request_id": request_id,
                "goal": goal,
                "steps": steps,
                "success_criteria": success_criteria,
                "timeout_seconds": timeout_seconds,
                "risk_level": risk_level,
                "revision_round": revision_round,
                "max_revisions": max_revisions,
            }),
        },
        GraphStreamEvent::InstructionReceived {
            execution_id,
            job_id,
            message,
        } => StreamEvent::Custom {
            event_type: "instruction_received".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "job_id": job_id,
                "message": message,
            }),
        },
        GraphStreamEvent::HITLInputRequired {
            execution_id,
            request_id,
            job_id,
            prompt,
            input_type,
            options,
            timeout_seconds,
            context,
        } => StreamEvent::Custom {
            event_type: "hitl_input_required".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "request_id": request_id,
                "job_id": job_id,
                "prompt": prompt,
                "input_type": input_type,
                "options": options,
                "timeout_seconds": timeout_seconds,
                "context": context,
            }),
        },
        // A40: Judge evaluation events
        GraphStreamEvent::JudgeEvaluated {
            execution_id,
            step_id,
            verdict,
            reasoning,
            suggestions,
            retry_count,
        } => StreamEvent::Custom {
            event_type: "judge_evaluated".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "step_id": step_id,
                "verdict": verdict,
                "reasoning": reasoning,
                "suggestions": suggestions,
                "retry_count": retry_count,
            }),
        },
        // A43: Research planner pipeline events
        GraphStreamEvent::ResearchProgress {
            execution_id,
            phase,
            message,
        } => StreamEvent::Custom {
            event_type: "research_progress".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "phase": phase,
                "message": message,
            }),
        },
        GraphStreamEvent::ComplexityAssessed {
            execution_id,
            complexity,
            reasoning,
            will_generate_prd,
        } => StreamEvent::Custom {
            event_type: "complexity_assessed".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "complexity": complexity,
                "reasoning": reasoning,
                "will_generate_prd": will_generate_prd,
            }),
        },
        GraphStreamEvent::ClarificationRequired {
            execution_id,
            request_id,
            questions,
            task_summary,
            timeout_seconds,
        } => StreamEvent::Custom {
            event_type: "clarification_required".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "request_id": request_id,
                "questions": questions,
                "task_summary": task_summary,
                "timeout_seconds": timeout_seconds,
            }),
        },
        GraphStreamEvent::PrdReviewRequired {
            execution_id,
            request_id,
            prd,
            timeout_seconds,
            revision_round,
            max_revisions,
        } => StreamEvent::Custom {
            event_type: "prd_review_required".into(),
            data: serde_json::json!({
                "execution_id": execution_id,
                "request_id": request_id,
                "prd": prd,
                "timeout_seconds": timeout_seconds,
                "revision_round": revision_round,
                "max_revisions": max_revisions,
            }),
        },
    }
}

/// Streaming chat endpoint
///
/// Routes chat requests through container orchestration when available (K8s environments),
/// falling back to direct chat engine execution otherwise.
pub async fn chat_stream(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(request): Json<StreamChatRequest>,
) -> Result<Sse<KeepAliveStream<BoxedSseStream>>, ApiError> {
    // R4-H18: Reject excessively large messages to prevent token/memory abuse
    const MAX_MESSAGE_LENGTH: usize = 1_000_000; // 1MB
    if request.message.len() > MAX_MESSAGE_LENGTH {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Message too large: {} bytes (max {})",
                request.message.len(),
                MAX_MESSAGE_LENGTH
            ),
        ));
    }

    let session_id = request.conversation_id.unwrap_or_else(Uuid::new_v4);
    let user_id = auth.user_id;

    tracing::info!(
        session_id = %session_id,
        profile_id = ?request.profile_id,
        task_type = ?request.task_type,
        "Processing streaming chat request"
    );

    // Microservice mode: forward to agent-service via gRPC
    if let Some(ref remote_client) = state.remote_agent_client {
        tracing::info!(session_id = %session_id, "Routing to remote agent-service (microservice mode)");
        let rx = remote_client
            .chat_stream(session_id, request.message.clone(), None)
            .await
            .map_err(|e| {
                ApiError::new(
                    axum::http::StatusCode::BAD_GATEWAY,
                    format!("Agent service error: {}", e),
                )
            })?;

        let stream: BoxedSseStream = Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx));
        return Ok(Sse::new(stream).keep_alive(KeepAlive::default()));
    }

    // Ensure conversation exists in database (create if needed)
    // If this fails, we track it so we can skip the message save (avoids FK violation)
    let conversation_exists = match state
        .conversation_repository
        .ensure_exists(session_id, Some(user_id))
        .await
    {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(
                session_id = %session_id,
                error = %e,
                "Failed to ensure conversation exists in database — will skip message persistence"
            );
            false
        }
    };

    // Save user message to database only if conversation exists (avoids FK violation)
    let user_message_content = request.message.clone();
    if conversation_exists {
        let user_msg = NewMessage {
            conversation_id: session_id,
            role: "user".to_string(),
            content: user_message_content.clone(),
            artifacts: None,
            tool_calls: None,
            tool_results: None,
            tokens_used: None,
            model_used: None,
        };

        if let Err(e) = state.message_repository.save_message(user_msg).await {
            tracing::warn!(
                session_id = %session_id,
                error = %e,
                "Failed to save user message to database"
            );
        }
    } else {
        tracing::warn!(
            session_id = %session_id,
            "Skipping message save — conversation does not exist in database"
        );
    }

    // Route through collaboration mode if explicitly set (A24)
    #[cfg(feature = "collaboration")]
    if let Some(ref mode_str) = request.collaboration_mode {
        if let Some(collab_mode) = parse_collaboration_mode(mode_str) {
            tracing::info!(
                session_id = %session_id,
                mode = %mode_str,
                "Routing streaming chat through collaboration mode"
            );

            let agent_factory = state.agent_factory.clone();
            let message = request.message.clone();
            let mode_str_owned = mode_str.clone();
            #[cfg(feature = "graph")]
            let execution_store = state.execution_store.clone();

            // Create channel for SSE events
            let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(128);

            tokio::spawn(async move {
                // Send start event
                let start_event = StreamEvent::start(session_id, Uuid::new_v4());
                let json = serde_json::to_string(&start_event).unwrap_or_default();
                let _ = tx.send(Ok(Event::default().data(json))).await;

                // Send routing decision event
                let routing_event = serde_json::json!({
                    "event": "routing_decision",
                    "data": {
                        "mode": mode_str_owned,
                        "source": "explicit",
                        "category": null,
                        "confidence": null,
                        "reasoning": format!("User explicitly selected {} mode", mode_str_owned),
                    }
                });
                let _ = tx
                    .send(Ok(Event::default().data(
                        serde_json::to_string(&routing_event).unwrap_or_default(),
                    )))
                    .await;

                // Send thinking event so the client knows execution is in progress
                let thinking_event =
                    StreamEvent::thinking(format!("Executing {} collaboration...", mode_str_owned));
                let json = serde_json::to_string(&thinking_event).unwrap_or_default();
                let _ = tx.send(Ok(Event::default().data(json))).await;

                // Progress heartbeat (every 3s) so the client knows execution is alive
                let tx_progress = tx.clone();
                let mode_label = mode_str_owned.clone();
                let progress_task = tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3));
                    interval.tick().await; // skip first immediate tick
                    let mut elapsed = 0u64;
                    loop {
                        interval.tick().await;
                        elapsed += 3;
                        let msg = format!("{} mode working... ({}s)", mode_label, elapsed);
                        let evt = StreamEvent::thinking(&msg);
                        let json = serde_json::to_string(&evt).unwrap_or_default();
                        if tx_progress
                            .send(Ok(Event::default().data(json)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                });

                // --- Non-blocking execution + real-time event forwarding ---
                // Uses start_collaboration_streaming() which spawns graph execution
                // in background and returns (JoinHandle, graph_rx) immediately.
                // This fixes the PlanExecute deadlock where plan_approval_required
                // events were buffered but never forwarded to the SSE client.
                // (See A41_PLAN_APPROVAL_STREAMING_FIX.md for full root cause analysis)

                // 1. ExecutionStore tracking (caller-managed with start_collaboration_streaming)
                let exec_id = Uuid::new_v4().to_string();
                #[cfg(feature = "graph")]
                {
                    let exec_mode = match &collab_mode {
                        gateway_core::collaboration::CollaborationMode::Direct => {
                            gateway_core::graph::ExecutionMode::Direct
                        }
                        gateway_core::collaboration::CollaborationMode::PlanExecute => {
                            gateway_core::graph::ExecutionMode::PlanExecute
                        }
                        gateway_core::collaboration::CollaborationMode::Swarm { .. } => {
                            gateway_core::graph::ExecutionMode::Swarm
                        }
                        gateway_core::collaboration::CollaborationMode::Expert { .. } => {
                            gateway_core::graph::ExecutionMode::Expert
                        }
                        gateway_core::collaboration::CollaborationMode::Graph { graph_id } => {
                            gateway_core::graph::ExecutionMode::Graph(graph_id.clone())
                        }
                    };
                    execution_store.start_execution(&exec_id, exec_mode).await;
                }
                let exec_start = std::time::Instant::now();

                // 2. Non-blocking start — returns (JoinHandle, graph_rx) immediately
                let (exec_handle, mut graph_rx) = match agent_factory
                    .start_collaboration_streaming(
                        &message,
                        Some(collab_mode),
                        &exec_id,
                        uuid::Uuid::nil(),
                    )
                    .await
                {
                    Ok(pair) => pair,
                    Err(e) => {
                        progress_task.abort();
                        #[cfg(feature = "graph")]
                        execution_store
                            .fail_execution(&exec_id, &e.to_string())
                            .await;
                        let error_event =
                            StreamEvent::error(&format!("Collaboration failed: {}", e), false);
                        let json = serde_json::to_string(&error_event).unwrap_or_default();
                        let _ = tx.send(Ok(Event::default().data(json))).await;
                        let done_event = StreamEvent::done(Uuid::new_v4(), vec![]);
                        let json = serde_json::to_string(&done_event).unwrap_or_default();
                        let _ = tx.send(Ok(Event::default().data(json))).await;
                        return;
                    }
                };

                // 3. Concurrent drain loop — forward events to SSE in real-time.
                //    This is the core fix: PlanApprovalRequired events reach the
                //    client while graph execution is still running.
                let mut content_already_streamed = false;
                loop {
                    tokio::select! {
                        biased;
                        event = graph_rx.recv() => {
                            match event {
                                Some(graph_event) => {
                                    if matches!(
                                        &graph_event,
                                        gateway_core::graph::GraphStreamEvent::NodeText { .. }
                                    ) {
                                        content_already_streamed = true;
                                    }
                                    let sse_event = convert_graph_stream_event_to_sse(&graph_event);
                                    let json = serde_json::to_string(&sse_event).unwrap_or_default();
                                    if tx.send(Ok(Event::default().data(json))).await.is_err() {
                                        // Client disconnected — abort execution
                                        exec_handle.abort();
                                        return;
                                    }
                                }
                                None => break, // Channel closed, execution complete
                            }
                        }
                    }
                }

                // 4. Stop heartbeat
                progress_task.abort();

                // 5. Await final result
                match exec_handle.await {
                    Ok(Ok(result)) => {
                        tracing::info!(
                            session_id = %session_id,
                            response_len = result.response.len(),
                            content_already_streamed,
                            "Collaboration execution completed"
                        );
                        #[cfg(feature = "graph")]
                        {
                            let elapsed = exec_start.elapsed().as_millis() as u64;
                            execution_store.complete_execution(&exec_id, elapsed).await;
                        }
                        if !content_already_streamed && !result.response.is_empty() {
                            let text_event = StreamEvent::text(&result.response);
                            let json = serde_json::to_string(&text_event).unwrap_or_default();
                            let _ = tx.send(Ok(Event::default().data(json))).await;
                        }
                        let done_event = StreamEvent::done(Uuid::new_v4(), vec![]);
                        let json = serde_json::to_string(&done_event).unwrap_or_default();
                        let _ = tx.send(Ok(Event::default().data(json))).await;
                    }
                    Ok(Err(e)) => {
                        tracing::error!(session_id = %session_id, error = %e, "Collaboration failed");
                        #[cfg(feature = "graph")]
                        execution_store
                            .fail_execution(&exec_id, &e.to_string())
                            .await;
                        let error_event =
                            StreamEvent::error(&format!("Collaboration failed: {}", e), false);
                        let json = serde_json::to_string(&error_event).unwrap_or_default();
                        let _ = tx.send(Ok(Event::default().data(json))).await;
                        let done_event = StreamEvent::done(Uuid::new_v4(), vec![]);
                        let json = serde_json::to_string(&done_event).unwrap_or_default();
                        let _ = tx.send(Ok(Event::default().data(json))).await;
                    }
                    Err(join_err) => {
                        tracing::error!(session_id = %session_id, error = %join_err, "Execution panicked");
                        #[cfg(feature = "graph")]
                        execution_store
                            .fail_execution(&exec_id, &join_err.to_string())
                            .await;
                        let error_event = StreamEvent::error(
                            "Collaboration execution task failed unexpectedly",
                            false,
                        );
                        let json = serde_json::to_string(&error_event).unwrap_or_default();
                        let _ = tx.send(Ok(Event::default().data(json))).await;
                        let done_event = StreamEvent::done(Uuid::new_v4(), vec![]);
                        let json = serde_json::to_string(&done_event).unwrap_or_default();
                        let _ = tx.send(Ok(Event::default().data(json))).await;
                    }
                }
            });

            let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
            let keep_alive = axum::response::sse::KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("ping");
            return Ok(Sse::new(stream.boxed()).keep_alive(keep_alive));
        } else {
            tracing::debug!(
                session_id = %session_id,
                mode = %mode_str,
                "Collaboration mode is Direct/Auto, using AgentRunner"
            );
        }
    }

    // Unified AgentRunner path for all environments
    // K8s/Docker/Firecracker execution is transparently handled through
    // UnifiedComputerTool → Router → appropriate ExecutionStrategy
    {
        // Get enabled namespaces from settings for MCP tool filtering
        let enabled_namespaces = get_enabled_namespaces_from_settings(&state).await;
        tracing::debug!(
            session_id = %session_id,
            enabled_namespaces = ?enabled_namespaces,
            active_plugins = ?request.active_plugins,
            "Using AgentRunner for streaming chat with namespace filtering"
        );

        let session_id_str = session_id.to_string();
        let message = request.message.clone();

        // Use the singleton agent factory from state (preserves sessions across requests)
        let agent_factory = &state.agent_factory;

        // Resolve bundle context if active_plugins are specified
        let agent_lock = if let Some(ref active_plugins) = request.active_plugins {
            if !active_plugins.is_empty() {
                // Resolve bundles → extra namespaces + system prompt
                let (bundle_namespaces, bundle_prompt) = {
                    let bundle_mgr = state.bundle_manager.read().await;
                    let cat_resolver = state.category_resolver.read().await;
                    let active_set: std::collections::HashSet<String> =
                        enabled_namespaces.iter().cloned().collect();

                    match bundle_mgr.resolve_bundles(active_plugins, &cat_resolver, &active_set) {
                        Ok(merged) => {
                            if !merged.warnings.is_empty() {
                                for w in &merged.warnings {
                                    tracing::warn!(session_id = %session_id, warning = %w, "Bundle resolution warning");
                                }
                            }
                            (merged.enabled_namespaces, merged.system_prompt)
                        }
                        Err(e) => {
                            tracing::warn!(
                                session_id = %session_id,
                                error = %e,
                                "Bundle resolution failed, falling back to base namespaces"
                            );
                            (vec![], None)
                        }
                    }
                };

                agent_factory
                    .get_or_create_with_bundles(
                        &session_id_str,
                        request.profile_id.clone(),
                        request.task_type.clone(),
                        enabled_namespaces,
                        bundle_namespaces,
                        bundle_prompt,
                    )
                    .await
            } else {
                // Empty plugin list → standard path
                agent_factory
                    .get_or_create_with_profile_and_namespaces(
                        &session_id_str,
                        request.profile_id.clone(),
                        request.task_type.clone(),
                        Some(enabled_namespaces),
                    )
                    .await
            }
        } else {
            // No active_plugins → standard path
            agent_factory
                .get_or_create_with_profile_and_namespaces(
                    &session_id_str,
                    request.profile_id.clone(),
                    request.task_type.clone(),
                    Some(enabled_namespaces),
                )
                .await
        };

        // Create channel for SSE events (128 buffer size to prevent backpressure during complex agent tasks)
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(128);

        // Clone the message repository for use in the spawned task
        let message_repo = state.message_repository.clone();

        // Clone unified memory store for pattern learning
        let unified_memory = state.unified_memory.clone();

        // Clone billing service for usage tracking
        let billing_service = state.billing_service.clone();

        // Clone billing-core metering service (A37, feature-gated)
        #[cfg(feature = "billing")]
        let metering_service_v2 = state.metering_service.clone();

        // Clone user message for pattern learning
        let user_message_for_learning = user_message_content.clone();

        // Get current message count for this conversation to determine if we should trigger learning
        let message_count = state
            .message_repository
            .get_message_count(session_id)
            .await
            .unwrap_or(0) as usize;

        // Spawn task to run agent and send events
        tokio::spawn(async move {
            // Send start event
            let start_event = StreamEvent::start(session_id, Uuid::new_v4());
            let json = serde_json::to_string(&start_event).unwrap_or_default();
            let _ = tx.send(Ok(Event::default().data(json))).await;

            // Get agent, run query, then release the write lock so permission
            // response endpoints can access the agent concurrently.
            let mut stream = {
                let mut agent = agent_lock.write().await;
                agent.query(&message).await
                // write lock dropped here — stream captures context by move (A41)
            };

            // Collect assistant response for persistence and billing
            let mut assistant_response = String::new();
            let mut model_used: Option<String> = None;
            let mut input_tokens: u32 = 0;
            let mut output_tokens: u32 = 0;
            let mut sent_done = false;
            let mut message_count_in_stream: u32 = 0;

            // Stream all messages
            while let Some(msg_result) = stream.next().await {
                message_count_in_stream += 1;
                match msg_result {
                    Ok(msg) => {
                        // Log message type for debugging agent loop flow
                        match &msg {
                            AgentMessage::Assistant(a) => {
                                let tool_calls: Vec<_> = a
                                    .content
                                    .iter()
                                    .filter_map(|b| {
                                        if let ContentBlock::ToolUse { name, .. } = b {
                                            Some(name.as_str())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();
                                tracing::info!(
                                    session_id = %session_id,
                                    msg_num = message_count_in_stream,
                                    model = %a.model,
                                    tool_calls = ?tool_calls,
                                    has_text = a.content.iter().any(|b| b.as_text().is_some()),
                                    "Agent stream: AssistantMessage"
                                );
                            }
                            AgentMessage::User(u) => {
                                tracing::info!(
                                    session_id = %session_id,
                                    msg_num = message_count_in_stream,
                                    has_tool_result = u.tool_use_result.is_some(),
                                    parent_tool_id = ?u.parent_tool_use_id,
                                    "Agent stream: UserMessage"
                                );
                            }
                            AgentMessage::Result(r) => {
                                tracing::info!(
                                    session_id = %session_id,
                                    msg_num = message_count_in_stream,
                                    subtype = ?r.subtype,
                                    num_turns = r.num_turns,
                                    is_error = r.is_error,
                                    duration_ms = r.duration_ms,
                                    "Agent stream: ResultMessage"
                                );
                            }
                            _ => {
                                tracing::debug!(
                                    session_id = %session_id,
                                    msg_num = message_count_in_stream,
                                    "Agent stream: other message type"
                                );
                            }
                        }

                        // Collect text content from assistant messages
                        if let AgentMessage::Assistant(assistant_msg) = &msg {
                            for block in &assistant_msg.content {
                                if let Some(text) = block.as_text() {
                                    assistant_response.push_str(text);
                                }
                            }
                            model_used = Some(assistant_msg.model.clone());
                        }

                        let events = convert_agent_message_to_stream_events(&msg, session_id);
                        let mut disconnected = false;
                        for event in events {
                            let json = serde_json::to_string(&event).unwrap_or_default();
                            if tx.send(Ok(Event::default().data(json))).await.is_err() {
                                disconnected = true;
                                break;
                            }
                        }
                        if disconnected {
                            tracing::warn!(
                                session_id = %session_id,
                                msg_num = message_count_in_stream,
                                "Client disconnected during streaming"
                            );
                            break; // Client disconnected
                        }

                        // Check if done and capture usage
                        if let AgentMessage::Result(result_msg) = &msg {
                            if let Some(usage) = &result_msg.usage {
                                input_tokens = usage.input_tokens;
                                output_tokens = usage.output_tokens;
                            }
                            sent_done = true;
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            session_id = %session_id,
                            msg_num = message_count_in_stream,
                            error = %e,
                            "Agent stream error"
                        );
                        let error_event = StreamEvent::error(&e.to_string(), false);
                        let json = serde_json::to_string(&error_event).unwrap_or_default();
                        let _ = tx.send(Ok(Event::default().data(json))).await;
                        break;
                    }
                }
            }

            // If stream ended without a Result message (e.g., panic, unexpected termination),
            // send a done event so the frontend knows the stream is complete
            if !sent_done {
                tracing::warn!(
                    session_id = %session_id,
                    message_count = message_count_in_stream,
                    "Agent stream ended without Result message - sending done event"
                );
                let usage = gateway_core::chat::streaming::TokenUsage {
                    prompt_tokens: input_tokens as i32,
                    completion_tokens: output_tokens as i32,
                    total_tokens: (input_tokens + output_tokens) as i32,
                };
                let done_event = StreamEvent::done_with_usage(Uuid::new_v4(), vec![], usage);
                let json = serde_json::to_string(&done_event).unwrap_or_default();
                let _ = tx.send(Ok(Event::default().data(json))).await;
            }

            // Clone model_used before moving it (needed for billing after save)
            let model_used_for_billing = model_used.clone();

            // Save assistant response to database if we collected content
            if !assistant_response.is_empty() {
                // Clone assistant_response before moving it into NewMessage
                let assistant_response_for_learning = assistant_response.clone();

                let assistant_msg = NewMessage {
                    conversation_id: session_id,
                    role: "assistant".to_string(),
                    content: assistant_response,
                    artifacts: None,
                    tool_calls: None,
                    tool_results: None,
                    tokens_used: None,
                    model_used,
                };

                if let Err(e) = message_repo.save_message(assistant_msg).await {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %e,
                        "Failed to save assistant message to database"
                    );
                } else {
                    tracing::debug!(
                        session_id = %session_id,
                        "Saved assistant message to database"
                    );

                    // Trigger automatic pattern learning (async, non-blocking)
                    // Only learn every PATTERN_LEARNING_MESSAGE_THRESHOLD messages
                    // to avoid excessive processing
                    let new_message_count = message_count + 2; // +1 for user, +1 for assistant
                    if new_message_count % PATTERN_LEARNING_MESSAGE_THRESHOLD == 0 {
                        tracing::debug!(
                            session_id = %session_id,
                            message_count = new_message_count,
                            "Triggering automatic pattern learning"
                        );

                        // Spawn pattern learning in background - don't block the response
                        let memory = unified_memory.clone();
                        let user_msg = user_message_for_learning.clone();
                        let assist_msg = assistant_response_for_learning;
                        tokio::spawn(async move {
                            trigger_pattern_learning(
                                memory, user_id, session_id, user_msg, assist_msg,
                            )
                            .await;
                        });
                    }
                }
            }

            // Record usage to billing service
            if input_tokens > 0 || output_tokens > 0 {
                // billing-core metering (PigaToken-based, preferred when billing feature enabled)
                #[cfg(feature = "billing")]
                {
                    let model = model_used_for_billing
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string());
                    match metering_service_v2
                        .record_llm_usage(
                            user_id,
                            &model,
                            input_tokens as u64,
                            output_tokens as u64,
                            Some(serde_json::json!({"session_id": session_id.to_string()})),
                        )
                        .await
                    {
                        Ok(result) => {
                            tracing::debug!(
                                user_id = %user_id,
                                model = %model,
                                cost_mpt = result.cost_mpt,
                                balance_mpt = result.balance_mpt,
                                "Metered via billing-core"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                user_id = %user_id,
                                error = %e,
                                "billing-core metering failed (non-blocking)"
                            );
                        }
                    }
                }

                // Legacy billing path (used when billing feature is not enabled)
                #[cfg(not(feature = "billing"))]
                {
                    if let Some(ref billing) = billing_service {
                        let model = model_used_for_billing.unwrap_or_else(|| "unknown".to_string());
                        let usage = gateway_core::llm::router::Usage {
                            prompt_tokens: input_tokens as i32,
                            completion_tokens: output_tokens as i32,
                            total_tokens: (input_tokens + output_tokens) as i32,
                        };
                        let provider = extract_provider_from_model(&model);
                        if let Err(e) = billing
                            .record_llm_usage(
                                user_id,
                                &model,
                                Some(provider),
                                &usage,
                                Some(session_id),
                            )
                            .await
                        {
                            tracing::warn!(
                                user_id = %user_id,
                                session_id = %session_id,
                                error = %e,
                                "Failed to record billing event"
                            );
                        } else {
                            tracing::debug!(
                                user_id = %user_id,
                                model = %model,
                                input_tokens = input_tokens,
                                output_tokens = output_tokens,
                                "Recorded billing event"
                            );
                        }
                    }
                }
            }
        });

        // Convert receiver to stream
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let boxed: BoxedSseStream = Box::pin(stream);

        Ok(Sse::new(boxed).keep_alive(KeepAlive::default()))
    }
}

/// Convert AgentMessage to one or more StreamEvents for SSE
/// Returns Vec to handle messages that contain both text and tool calls
fn convert_agent_message_to_stream_events(
    msg: &AgentMessage,
    _session_id: Uuid,
) -> Vec<StreamEvent> {
    match msg {
        AgentMessage::Assistant(assistant_msg) => {
            let mut events = Vec::new();

            for block in &assistant_msg.content {
                match block {
                    ContentBlock::Text { text } if !text.is_empty() => {
                        events.push(StreamEvent::text(text));
                    }
                    ContentBlock::Thinking { thinking, .. } if !thinking.is_empty() => {
                        events.push(StreamEvent::thinking(thinking));
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        events.push(StreamEvent::tool_call(id, name, input));
                    }
                    _ => {}
                }
            }

            if events.is_empty() {
                vec![StreamEvent::Heartbeat]
            } else {
                events
            }
        }
        AgentMessage::User(user_msg) => {
            // Check for tool results
            if let Some(result) = &user_msg.tool_use_result {
                if let Some(parent_id) = &user_msg.parent_tool_use_id {
                    return vec![StreamEvent::tool_result(parent_id, "", result)];
                }
            }
            vec![StreamEvent::Heartbeat]
        }
        AgentMessage::Result(result_msg) => {
            let usage = result_msg
                .usage
                .as_ref()
                .map(|u| gateway_core::chat::streaming::TokenUsage {
                    prompt_tokens: u.input_tokens as i32,
                    completion_tokens: u.output_tokens as i32,
                    total_tokens: (u.input_tokens + u.output_tokens) as i32,
                })
                .unwrap_or_else(|| gateway_core::chat::streaming::TokenUsage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    total_tokens: 0,
                });

            vec![StreamEvent::done_with_usage(Uuid::new_v4(), vec![], usage)]
        }
        AgentMessage::System(_) => vec![StreamEvent::Heartbeat],
        AgentMessage::StreamEvent(_) => vec![StreamEvent::Heartbeat],
        AgentMessage::PermissionRequest(perm_req) => {
            vec![StreamEvent::Custom {
                event_type: "permission_request".to_string(),
                data: serde_json::json!({
                    "request_id": perm_req.request_id,
                    "tool_name": perm_req.tool_name,
                    "tool_input": perm_req.tool_input,
                    "question": perm_req.question,
                    "options": perm_req.options,
                    "session_id": perm_req.session_id,
                }),
            }]
        }
    }
}

/// Conversation summary
#[derive(Debug, Serialize)]
pub struct ConversationSummary {
    pub id: Uuid,
    pub title: Option<String>,
    pub message_count: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// List conversations for the authenticated user
pub async fn list_conversations(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<Vec<ConversationSummary>>, ApiError> {
    let user_id = auth.user_id;

    // Get conversations for the authenticated user only
    match state
        .conversation_repository
        .get_user_conversations(user_id, 100)
        .await
    {
        Ok(db_conversations) => {
            let mut summaries: Vec<ConversationSummary> = Vec::new();

            for conv in db_conversations {
                // Get message count from database
                let message_count = state
                    .message_repository
                    .get_message_count(conv.id)
                    .await
                    .unwrap_or(0) as usize;

                summaries.push(ConversationSummary {
                    id: conv.id,
                    title: conv.title,
                    message_count,
                    created_at: conv.created_at,
                    updated_at: conv.updated_at,
                });
            }

            Ok(Json(summaries))
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to list conversations from database, falling back to in-memory");

            // Fallback to in-memory sessions
            let sessions = state.chat_engine.get_user_sessions(&user_id);

            let summaries: Vec<ConversationSummary> = sessions
                .iter()
                .map(|s| ConversationSummary {
                    id: s.id,
                    title: s.title.clone(),
                    message_count: s.message_count(),
                    created_at: s.created_at,
                    updated_at: s.updated_at,
                })
                .collect();

            Ok(Json(summaries))
        }
    }
}

/// Create conversation request
#[derive(Debug, Deserialize)]
pub struct CreateConversationRequest {
    pub title: Option<String>,
}

/// Create a new conversation
pub async fn create_conversation(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(request): Json<CreateConversationRequest>,
) -> Result<Json<ConversationSummary>, ApiError> {
    let user_id = auth.user_id;

    // Create conversation in database
    let new_conv = NewConversation {
        id: None, // Let database generate the ID
        user_id: Some(user_id),
        organization_id: None,
        title: request.title.clone(),
        metadata: None,
    };

    match state
        .conversation_repository
        .save_conversation(new_conv)
        .await
    {
        Ok(stored_conv) => {
            tracing::info!(
                conversation_id = %stored_conv.id,
                title = ?stored_conv.title,
                "Created new conversation in database"
            );

            Ok(Json(ConversationSummary {
                id: stored_conv.id,
                title: stored_conv.title,
                message_count: 0,
                created_at: stored_conv.created_at,
                updated_at: stored_conv.updated_at,
            }))
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to create conversation in database");
            Err(ApiError::internal(format!(
                "Failed to create conversation: {}",
                e
            )))
        }
    }
}

/// Get a specific conversation (with user ownership verification)
pub async fn get_conversation(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<ConversationSummary>, ApiError> {
    let user_id = auth.user_id;

    // First try to get from database with user verification
    if let Ok(Some(conv)) = state
        .conversation_repository
        .get_conversation_for_user(id, user_id)
        .await
    {
        let message_count = state
            .message_repository
            .get_message_count(id)
            .await
            .unwrap_or(0) as usize;

        return Ok(Json(ConversationSummary {
            id: conv.id,
            title: conv.title,
            message_count,
            created_at: conv.created_at,
            updated_at: conv.updated_at,
        }));
    }

    // Fallback to in-memory session (also check ownership)
    let session = state
        .chat_engine
        .get_session(&id)
        .ok_or_else(|| ApiError::not_found(format!("Conversation {} not found", id)))?;

    // Note: In-memory sessions don't have explicit user_id, so we allow access
    // In production, you may want to add user_id to ChatSession as well

    Ok(Json(ConversationSummary {
        id: session.id,
        title: session.title.clone(),
        message_count: session.message_count(),
        created_at: session.created_at,
        updated_at: session.updated_at,
    }))
}

/// Message response
#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Get messages for a conversation (with user ownership verification)
pub async fn get_messages(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<MessageResponse>>, ApiError> {
    let user_id = auth.user_id;

    // First verify user owns this conversation
    let owns_conversation = state
        .conversation_repository
        .user_owns_conversation(id, user_id)
        .await
        .unwrap_or(false);

    if !owns_conversation {
        // Check if conversation exists at all
        let exists = state
            .conversation_repository
            .exists(id)
            .await
            .unwrap_or(false);

        if exists {
            // Conversation exists but user doesn't own it
            return Err(ApiError::forbidden(
                "You don't have access to this conversation",
            ));
        }
    }

    // Get messages with user verification
    if let Ok(db_messages) = state
        .message_repository
        .get_messages_for_user(id, user_id)
        .await
    {
        if !db_messages.is_empty() {
            let messages: Vec<MessageResponse> = db_messages
                .iter()
                .map(|m| MessageResponse {
                    id: m.id,
                    role: m.role.clone(),
                    content: m.content.clone(),
                    created_at: m.created_at,
                })
                .collect();

            return Ok(Json(messages));
        }
    }

    // Fallback to in-memory session
    if let Some(session) = state.chat_engine.get_session(&id) {
        let messages: Vec<MessageResponse> = session
            .get_messages()
            .iter()
            .map(|m| MessageResponse {
                id: m.id,
                role: m.role.to_string(),
                content: m.content.clone(),
                created_at: m.created_at,
            })
            .collect();

        return Ok(Json(messages));
    }

    // If no messages found, return empty list
    Ok(Json(vec![]))
}

// ============================================
// Pattern Learning Module
// ============================================

/// Configuration for automatic pattern learning
const PATTERN_LEARNING_MESSAGE_THRESHOLD: usize = 5;

/// Trigger automatic pattern learning from conversation history.
/// This function is called asynchronously after saving assistant messages.
///
/// Learning is triggered every N messages (configured by PATTERN_LEARNING_MESSAGE_THRESHOLD)
/// to avoid excessive processing while still capturing useful patterns.
async fn trigger_pattern_learning(
    unified_memory: Arc<UnifiedMemoryStore>,
    user_id: Uuid,
    conversation_id: Uuid,
    user_message: String,
    assistant_response: String,
) {
    // Extract and record patterns from the conversation
    if let Err(e) = learn_patterns_from_exchange(
        &unified_memory,
        user_id,
        conversation_id,
        &user_message,
        &assistant_response,
    )
    .await
    {
        tracing::warn!(
            user_id = %user_id,
            conversation_id = %conversation_id,
            error = %e,
            "Failed to learn patterns from conversation"
        );
    }
}

/// Learn patterns from a user-assistant exchange.
/// Extracts:
/// - Tool usage patterns (what tools are used for what tasks)
/// - Communication style (formal/informal, language preferences)
/// - Common operations (file operations, code patterns, etc.)
async fn learn_patterns_from_exchange(
    unified_memory: &Arc<UnifiedMemoryStore>,
    user_id: Uuid,
    conversation_id: Uuid,
    user_message: &str,
    assistant_response: &str,
) -> Result<(), gateway_core::error::Error> {
    let now = chrono::Utc::now();

    // 1. Detect tool usage patterns from assistant response
    // Look for common tool invocation patterns in the response
    let tool_patterns = extract_tool_patterns(assistant_response);
    for (tool_name, context) in tool_patterns {
        let pattern = MemoryPattern {
            id: Uuid::new_v4(),
            pattern_type: PatternType::ToolUsage,
            description: format!("Uses '{}' tool for: {}", tool_name, context),
            confidence: 0.6, // Initial confidence, increases with repetition
            examples: vec![user_message.chars().take(200).collect()],
            occurrence_count: 1,
            last_seen: now,
        };
        unified_memory.record_pattern(user_id, pattern).await?;

        tracing::debug!(
            user_id = %user_id,
            tool = %tool_name,
            "Recorded tool usage pattern"
        );
    }

    // 2. Detect communication style patterns
    if let Some(style_pattern) = detect_communication_style(user_message) {
        let pattern = MemoryPattern {
            id: Uuid::new_v4(),
            pattern_type: PatternType::Communication,
            description: style_pattern.clone(),
            confidence: 0.5,
            examples: vec![user_message.chars().take(100).collect()],
            occurrence_count: 1,
            last_seen: now,
        };
        unified_memory.record_pattern(user_id, pattern).await?;

        tracing::debug!(
            user_id = %user_id,
            style = %style_pattern,
            "Recorded communication style pattern"
        );
    }

    // 3. Detect workflow patterns (common sequences of operations)
    if let Some(workflow_pattern) = detect_workflow_pattern(user_message, assistant_response) {
        let pattern = MemoryPattern {
            id: Uuid::new_v4(),
            pattern_type: PatternType::Workflow,
            description: workflow_pattern.clone(),
            confidence: 0.5,
            examples: vec![user_message.chars().take(150).collect()],
            occurrence_count: 1,
            last_seen: now,
        };
        unified_memory.record_pattern(user_id, pattern).await?;

        tracing::debug!(
            user_id = %user_id,
            workflow = %workflow_pattern,
            "Recorded workflow pattern"
        );
    }

    // 4. Store conversation summary for context
    let summary_key = format!("conv_{}_{}", conversation_id, now.timestamp());
    let summary_content = format!(
        "User asked: {}\nAssistant provided: {}",
        user_message.chars().take(200).collect::<String>(),
        summarize_response(assistant_response)
    );

    let entry = MemoryEntry::new(summary_key, MemoryCategory::Conversation, summary_content)
        .with_title(format!(
            "Conversation exchange {}",
            now.format("%Y-%m-%d %H:%M")
        ))
        .with_source(MemorySource::Inferred)
        .with_confidence(Confidence::Medium)
        .with_tags(vec![
            format!("conversation:{}", conversation_id),
            "auto-learned".to_string(),
        ]);

    unified_memory.store(user_id, entry).await?;

    tracing::info!(
        user_id = %user_id,
        conversation_id = %conversation_id,
        "Pattern learning completed for conversation exchange"
    );

    Ok(())
}

/// Extract tool usage patterns from assistant response.
/// Returns a list of (tool_name, context) tuples.
fn extract_tool_patterns(response: &str) -> Vec<(String, String)> {
    let mut patterns = Vec::new();
    let response_lower = response.to_lowercase();

    // Common tool patterns to detect
    let tool_indicators = [
        (
            "bash",
            vec!["running command", "executing", "terminal", "shell"],
        ),
        ("read", vec!["reading file", "file contents", "opened"]),
        ("write", vec!["writing to", "created file", "saved"]),
        ("edit", vec!["editing", "modified", "updated file"]),
        ("grep", vec!["searching", "found matches", "search results"]),
        (
            "glob",
            vec!["finding files", "file pattern", "matching files"],
        ),
        (
            "browser",
            vec!["navigating", "webpage", "clicking", "screenshot"],
        ),
        (
            "code_execution",
            vec!["executing code", "running script", "output"],
        ),
    ];

    for (tool, indicators) in tool_indicators {
        for indicator in indicators {
            if response_lower.contains(indicator) {
                // Extract context around the indicator
                if let Some(pos) = response_lower.find(indicator) {
                    let start = pos.saturating_sub(50);
                    let end = (pos + indicator.len() + 100).min(response.len());
                    let context = response[start..end]
                        .trim()
                        .chars()
                        .take(100)
                        .collect::<String>();
                    patterns.push((tool.to_string(), context));
                    break; // Only record once per tool
                }
            }
        }
    }

    patterns
}

/// Detect communication style from user message.
/// Returns a description of the detected style.
fn detect_communication_style(message: &str) -> Option<String> {
    let message_lower = message.to_lowercase();

    // Detect language preference
    let has_chinese = message.chars().any(|c| {
        let code = c as u32;
        (0x4E00..=0x9FFF).contains(&code) || (0x3400..=0x4DBF).contains(&code)
    });

    // Detect formality
    let formal_indicators = ["please", "kindly", "would you", "could you", "thank you"];
    let informal_indicators = ["hey", "yo", "gimme", "gonna", "wanna", "asap"];

    let formal_count = formal_indicators
        .iter()
        .filter(|&ind| message_lower.contains(ind))
        .count();
    let informal_count = informal_indicators
        .iter()
        .filter(|&ind| message_lower.contains(ind))
        .count();

    // Detect technical vs non-technical
    let technical_indicators = [
        "code",
        "function",
        "api",
        "debug",
        "error",
        "compile",
        "deploy",
        "git",
        "docker",
        "kubernetes",
        "database",
        "sql",
    ];
    let technical_count = technical_indicators
        .iter()
        .filter(|&ind| message_lower.contains(ind))
        .count();

    let mut styles = Vec::new();

    if has_chinese {
        styles.push("Prefers Chinese communication");
    }

    if formal_count > informal_count {
        styles.push("Formal communication style");
    } else if informal_count > formal_count {
        styles.push("Informal/casual communication style");
    }

    if technical_count >= 2 {
        styles.push("Technical/developer focus");
    }

    if message.len() > 500 {
        styles.push("Provides detailed context");
    } else if message.len() < 50 {
        styles.push("Concise/brief requests");
    }

    if styles.is_empty() {
        None
    } else {
        Some(styles.join("; "))
    }
}

/// Detect workflow patterns from the exchange.
/// Identifies common task sequences like "read -> edit -> test".
fn detect_workflow_pattern(user_message: &str, assistant_response: &str) -> Option<String> {
    let combined = format!("{} {}", user_message, assistant_response).to_lowercase();

    // Common workflow patterns
    let workflows = [
        (
            vec!["read", "edit", "save"],
            "File modification workflow: read -> edit -> save",
        ),
        (
            vec!["search", "find", "open"],
            "Search and navigate workflow: search -> find -> open",
        ),
        (
            vec!["test", "fix", "test"],
            "Test-driven development: test -> fix -> verify",
        ),
        (
            vec!["git", "commit", "push"],
            "Git workflow: stage -> commit -> push",
        ),
        (
            vec!["create", "write", "run"],
            "Script creation workflow: create -> write -> execute",
        ),
        (
            vec!["debug", "log", "trace"],
            "Debugging workflow: investigate -> log -> trace",
        ),
        (
            vec!["browse", "click", "screenshot"],
            "Browser automation: navigate -> interact -> capture",
        ),
    ];

    for (indicators, description) in workflows {
        let match_count = indicators
            .iter()
            .filter(|&ind| combined.contains(ind))
            .count();

        // If most indicators match, record this workflow
        if match_count >= indicators.len() - 1 {
            return Some(description.to_string());
        }
    }

    None
}

/// Create a brief summary of the assistant response.
fn summarize_response(response: &str) -> String {
    // Take first 200 chars or first sentence, whichever is shorter
    let first_sentence_end = response
        .find(|c| c == '.' || c == '!' || c == '?')
        .map(|pos| pos + 1)
        .unwrap_or(response.len());

    let summary_end = first_sentence_end.min(200);
    let mut summary: String = response.chars().take(summary_end).collect();

    if summary.len() < response.len() {
        summary.push_str("...");
    }

    summary
}
