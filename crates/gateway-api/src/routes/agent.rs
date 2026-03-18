//! Agent API endpoints
//!
//! Provides unified agent execution using AgentRunner with full integration
//! of LLM, tools, hooks, and permissions.
//!
//! When the `orchestration` feature is enabled, all agent queries automatically
//! use the graph-based execution engine with AutoModeSelector to choose the
//! optimal collaboration mode (Direct, Swarm, or Expert) based on task complexity.

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, KeepAliveStream, Sse},
    routing::{get, post},
    Json, Router,
};
use futures::stream::{Stream, StreamExt};
use gateway_core::agent::{
    AgentFactory, AgentLoop, AgentMessage, ContentBlock, PendingPermission, PermissionResponse,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

use crate::{error::ApiError, state::AppState};

// Orchestration imports (feature-gated)
#[cfg(feature = "orchestration")]
use gateway_core::agent::AutoModeSelector;

/// Type alias for boxed SSE stream
type BoxedSseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

/// Create the agent routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/query", post(agent_query))
        .route("/stream", post(agent_stream))
        .route("/sessions", get(list_sessions))
        .route("/sessions/{session_id}", get(get_session_info))
        .route("/sessions/{session_id}", axum::routing::delete(delete_session))
        // Permission endpoints
        .route(
            "/sessions/{session_id}/permissions",
            get(get_pending_permissions),
        )
        .route(
            "/sessions/{session_id}/permissions/respond",
            post(submit_permission_response),
        )
        .route(
            "/sessions/{session_id}/permissions/cancel",
            post(cancel_pending_permissions),
        )
}

/// Agent query request
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct AgentQueryRequest {
    /// The prompt/message to send to the agent
    pub message: String,
    /// Session ID (optional - will create new if not provided)
    #[serde(default)]
    pub session_id: Option<Uuid>,
    /// Working directory for the agent
    #[serde(default)]
    pub cwd: Option<String>,
    /// Permission mode
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Maximum turns allowed
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Model to use
    #[serde(default)]
    pub model: Option<String>,
}

/// Agent query response
#[derive(Debug, Serialize)]
pub struct AgentQueryResponse {
    /// Session ID
    pub session_id: Uuid,
    /// Messages from the agent
    pub messages: Vec<AgentMessageResponse>,
    /// Final result
    pub result: Option<ResultResponse>,
    /// Usage statistics
    pub usage: UsageResponse,
}

/// Agent message response
#[derive(Debug, Serialize)]
pub struct AgentMessageResponse {
    /// Message type
    pub r#type: String,
    /// Content (for text messages)
    pub content: Option<String>,
    /// Tool calls (for assistant messages with tools)
    pub tool_calls: Option<Vec<ToolCallResponse>>,
    /// Tool result (for tool result messages)
    pub tool_result: Option<serde_json::Value>,
}

/// Tool call response
#[derive(Debug, Serialize)]
pub struct ToolCallResponse {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// Result response
#[derive(Debug, Serialize)]
pub struct ResultResponse {
    pub success: bool,
    pub result: Option<String>,
    pub num_turns: u32,
    pub duration_ms: u64,
}

/// Usage response
#[derive(Debug, Serialize)]
pub struct UsageResponse {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

/// Extract text from MessageContent
fn message_content_to_string(content: &gateway_core::agent::MessageContent) -> String {
    match content {
        gateway_core::agent::MessageContent::Text(text) => text.clone(),
        gateway_core::agent::MessageContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join(""),
    }
}

/// Convert AgentMessage to response format
fn convert_agent_message(msg: &AgentMessage) -> AgentMessageResponse {
    match msg {
        AgentMessage::User(user_msg) => AgentMessageResponse {
            r#type: "user".to_string(),
            content: Some(message_content_to_string(&user_msg.content)),
            tool_calls: None,
            tool_result: user_msg.tool_use_result.clone(),
        },
        AgentMessage::Assistant(assistant_msg) => {
            let content: String = assistant_msg
                .content
                .iter()
                .filter_map(|b| b.as_text())
                .collect::<Vec<_>>()
                .join("");

            let tool_calls: Vec<ToolCallResponse> = assistant_msg
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => Some(ToolCallResponse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    }),
                    _ => None,
                })
                .collect();

            AgentMessageResponse {
                r#type: "assistant".to_string(),
                content: if content.is_empty() {
                    None
                } else {
                    Some(content)
                },
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
                tool_result: None,
            }
        }
        AgentMessage::System(system_msg) => AgentMessageResponse {
            r#type: "system".to_string(),
            content: Some(system_msg.data.to_string()),
            tool_calls: None,
            tool_result: None,
        },
        AgentMessage::Result(result_msg) => AgentMessageResponse {
            r#type: "result".to_string(),
            content: result_msg.result.clone(),
            tool_calls: None,
            tool_result: None,
        },
        AgentMessage::StreamEvent(event_msg) => AgentMessageResponse {
            r#type: "stream_event".to_string(),
            content: Some(format!("{:?}", event_msg.subtype)),
            tool_calls: None,
            tool_result: None,
        },
        AgentMessage::PermissionRequest(perm_req) => AgentMessageResponse {
            r#type: "permission_request".to_string(),
            content: Some(perm_req.question.clone()),
            tool_calls: None,
            tool_result: Some(serde_json::json!({
                "request_id": perm_req.request_id,
                "tool_name": perm_req.tool_name,
                "tool_input": perm_req.tool_input,
                "options": perm_req.options,
            })),
        },
    }
}

/// Non-streaming agent query endpoint
///
/// Executes an agent query and waits for completion, returning all messages.
///
/// When the `orchestration` feature is enabled, this endpoint automatically uses
/// the graph-based execution engine with AutoModeSelector to choose the optimal
/// collaboration mode based on task complexity.
pub async fn agent_query(
    State(state): State<AppState>,
    Json(request): Json<AgentQueryRequest>,
) -> Result<Json<AgentQueryResponse>, ApiError> {
    let session_id = request.session_id.unwrap_or_else(Uuid::new_v4);

    tracing::info!(
        session_id = %session_id,
        message_len = request.message.len(),
        "Processing agent query"
    );

    // When orchestration feature is enabled, use graph-based execution
    #[cfg(feature = "orchestration")]
    {
        return execute_with_orchestration(&state, session_id, &request.message).await;
    }

    // Fallback to direct AgentRunner execution when orchestration is disabled
    #[cfg(not(feature = "orchestration"))]
    {
        execute_with_agent_runner(&state, session_id, &request.message).await
    }
}

/// Execute using graph-based orchestration with auto-selected collaboration mode
#[cfg(feature = "orchestration")]
async fn execute_with_orchestration(
    state: &AppState,
    session_id: Uuid,
    message: &str,
) -> Result<Json<AgentQueryResponse>, ApiError> {
    let agent_factory = get_agent_factory(state);

    // Use AutoModeSelector to determine the best collaboration mode based on task text
    let mode_selector = AutoModeSelector::builder().build();
    let collab_mode = mode_selector.suggest_collaboration_mode_for_text(message);

    tracing::info!(
        session_id = %session_id,
        collaboration_mode = ?collab_mode,
        "Auto-selected collaboration mode for task"
    );

    // Execute with the selected collaboration mode
    let start_time = std::time::Instant::now();
    let graph_result = agent_factory
        .execute_with_collaboration(message, Some(collab_mode))
        .await
        .map_err(|e| ApiError::internal(format!("Orchestration error: {}", e)))?;

    let duration_ms = start_time.elapsed().as_millis() as u64;

    // Convert graph result to API response format
    let messages: Vec<AgentMessageResponse> = graph_result
        .messages
        .iter()
        .map(|msg| convert_agent_message(msg))
        .collect();

    let result = Some(ResultResponse {
        success: true,
        result: Some(graph_result.response.clone()),
        num_turns: messages.len() as u32,
        duration_ms,
    });

    // Extract usage from graph state metadata
    let final_usage = UsageResponse {
        input_tokens: 0,  // Not tracked separately in StateMetadata
        output_tokens: 0, // Not tracked separately in StateMetadata
        total_tokens: graph_result.metadata.total_tokens,
    };

    Ok(Json(AgentQueryResponse {
        session_id,
        messages,
        result,
        usage: final_usage,
    }))
}

/// Execute using direct AgentRunner (fallback when orchestration is disabled)
#[cfg(not(feature = "orchestration"))]
async fn execute_with_agent_runner(
    state: &AppState,
    session_id: Uuid,
    message: &str,
) -> Result<Json<AgentQueryResponse>, ApiError> {
    let session_id_str = session_id.to_string();
    let agent_factory = get_agent_factory(state);

    // Get or create agent for session
    let agent_lock = agent_factory.get_or_create(&session_id_str).await;

    // Execute query and collect all messages
    let mut messages = Vec::new();
    let mut result: Option<ResultResponse> = None;
    let mut final_usage = UsageResponse {
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
    };

    // Scope the mutable borrow — release write lock after query() so permission
    // response endpoint can acquire it without deadlocking (A41 fix)
    {
        let mut stream = {
            let mut agent = agent_lock.write().await;
            agent.query(message).await
            // write lock dropped here — stream captures context by move
        };

        while let Some(msg_result) = stream.next().await {
            match msg_result {
                Ok(msg) => {
                    // Check if this is a result message
                    if let AgentMessage::Result(result_msg) = &msg {
                        result = Some(ResultResponse {
                            success: !result_msg.is_error,
                            result: result_msg.result.clone(),
                            num_turns: result_msg.num_turns,
                            duration_ms: result_msg.duration_ms,
                        });
                        // Extract usage from result message
                        if let Some(usage) = &result_msg.usage {
                            final_usage = UsageResponse {
                                input_tokens: usage.input_tokens,
                                output_tokens: usage.output_tokens,
                                total_tokens: usage.input_tokens + usage.output_tokens,
                            };
                        }
                    }
                    messages.push(convert_agent_message(&msg));
                }
                Err(e) => {
                    tracing::error!(error = %e, "Agent query error");
                    return Err(ApiError::internal(format!("Agent error: {}", e)));
                }
            }
        }
    }

    Ok(Json(AgentQueryResponse {
        session_id,
        messages,
        result,
        usage: final_usage,
    }))
}

/// Streaming agent query endpoint
///
/// Executes an agent query and streams messages as SSE events.
pub async fn agent_stream(
    State(state): State<AppState>,
    Json(request): Json<AgentQueryRequest>,
) -> Result<Sse<KeepAliveStream<BoxedSseStream>>, ApiError> {
    let session_id = request.session_id.unwrap_or_else(Uuid::new_v4);
    let session_id_str = session_id.to_string();

    tracing::info!(
        session_id = %session_id,
        "Starting agent stream"
    );

    // Get or create agent factory
    let agent_factory = get_agent_factory(&state);

    // Get or create agent for session
    let agent_lock = agent_factory.get_or_create(&session_id_str).await;
    let message = request.message.clone();

    // Use a channel-based approach to avoid borrow issues
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(32);

    // Spawn task to run agent query and send events
    tokio::spawn(async move {
        // Send start event
        let start_event = serde_json::json!({
            "type": "start",
            "session_id": session_id,
        });
        let json = serde_json::to_string(&start_event).unwrap_or_default();
        let _ = tx.send(Ok(Event::default().data(json))).await;

        // Get agent and run query — release write lock after query() to avoid
        // deadlock with permission response endpoint (A41 fix)
        let mut stream = {
            let mut agent = agent_lock.write().await;
            agent.query(&message).await
            // write lock dropped here — stream captures context by move
        };

        // Stream all messages
        while let Some(msg_result) = stream.next().await {
            match msg_result {
                Ok(msg) => {
                    let event_data = convert_to_sse_event(&msg, session_id);
                    let json = serde_json::to_string(&event_data).unwrap_or_default();
                    if tx.send(Ok(Event::default().data(json))).await.is_err() {
                        break; // Client disconnected
                    }

                    // Check if done
                    if matches!(msg, AgentMessage::Result(_)) {
                        break;
                    }
                }
                Err(e) => {
                    let error_event = serde_json::json!({
                        "type": "error",
                        "error": e.to_string(),
                    });
                    let json = serde_json::to_string(&error_event).unwrap_or_default();
                    let _ = tx.send(Ok(Event::default().data(json))).await;
                    break;
                }
            }
        }

        // Send done event
        let done_event = serde_json::json!({
            "type": "done",
            "session_id": session_id,
        });
        let json = serde_json::to_string(&done_event).unwrap_or_default();
        let _ = tx.send(Ok(Event::default().data(json))).await;
    });

    // Convert receiver to stream
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let boxed: BoxedSseStream = Box::pin(stream);
    Ok(Sse::new(boxed).keep_alive(KeepAlive::default()))
}

/// Convert AgentMessage to SSE event data
fn convert_to_sse_event(msg: &AgentMessage, session_id: Uuid) -> serde_json::Value {
    match msg {
        AgentMessage::User(user_msg) => {
            serde_json::json!({
                "type": "user_message",
                "session_id": session_id,
                "content": message_content_to_string(&user_msg.content),
            })
        }
        AgentMessage::Assistant(assistant_msg) => {
            let text_content: String = assistant_msg
                .content
                .iter()
                .filter_map(|b| b.as_text())
                .collect::<Vec<_>>()
                .join("");

            let tool_calls: Vec<serde_json::Value> = assistant_msg
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, name, input } => Some(serde_json::json!({
                        "id": id,
                        "name": name,
                        "input": input,
                    })),
                    _ => None,
                })
                .collect();

            serde_json::json!({
                "type": "assistant_message",
                "session_id": session_id,
                "content": text_content,
                "tool_calls": tool_calls,
                "model": assistant_msg.model,
            })
        }
        AgentMessage::Result(result_msg) => {
            serde_json::json!({
                "type": "result",
                "session_id": session_id,
                "success": !result_msg.is_error,
                "result": result_msg.result,
                "num_turns": result_msg.num_turns,
                "duration_ms": result_msg.duration_ms,
                "usage": result_msg.usage,
            })
        }
        AgentMessage::System(system_msg) => {
            serde_json::json!({
                "type": "system",
                "session_id": session_id,
                "subtype": system_msg.subtype,
                "data": system_msg.data,
            })
        }
        AgentMessage::StreamEvent(event_msg) => {
            serde_json::json!({
                "type": "stream_event",
                "session_id": session_id,
                "subtype": format!("{:?}", event_msg.subtype),
            })
        }
        AgentMessage::PermissionRequest(perm_req) => {
            serde_json::json!({
                "type": "permission_request",
                "session_id": session_id,
                "request_id": perm_req.request_id,
                "tool_name": perm_req.tool_name,
                "tool_input": perm_req.tool_input,
                "question": perm_req.question,
                "options": perm_req.options,
            })
        }
    }
}

/// Session info response
#[derive(Debug, Serialize)]
pub struct SessionInfoResponse {
    pub session_id: Uuid,
    pub is_running: bool,
    pub turn_count: u32,
}

/// List active agent sessions (admin-only to prevent session enumeration — R4-H10)
pub async fn list_sessions(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<canal_auth::AuthContext>,
) -> Result<Json<Vec<String>>, ApiError> {
    if !auth.is_admin() {
        return Err(ApiError::forbidden(
            "Admin access required to list all sessions",
        ));
    }
    let agent_factory = get_agent_factory(&state);
    let sessions = agent_factory.list_sessions().await;
    Ok(Json(sessions))
}

/// Get session info (R4-H10: AuthContext extracted for audit trail)
pub async fn get_session_info(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<canal_auth::AuthContext>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionInfoResponse>, ApiError> {
    let agent_factory = get_agent_factory(&state);
    let agents = agent_factory.list_sessions().await;

    if !agents.contains(&session_id) {
        return Err(ApiError::not_found(format!(
            "Session {} not found",
            session_id
        )));
    }

    let agent_lock = agent_factory.get_or_create(&session_id).await;
    let agent = agent_lock.read().await;

    Ok(Json(SessionInfoResponse {
        session_id: Uuid::parse_str(&session_id).unwrap_or_default(),
        is_running: agent.is_running(),
        turn_count: 0, // Would need state access
    }))
}

/// Delete a session (R4-H10: AuthContext extracted for audit trail)
pub async fn delete_session(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<canal_auth::AuthContext>,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let agent_factory = get_agent_factory(&state);
    tracing::info!(
        session_id = %session_id,
        user_id = %auth.user_id,
        "User deleting agent session"
    );

    if agent_factory.remove(&session_id).await.is_some() {
        Ok(Json(serde_json::json!({
            "success": true,
            "session_id": session_id,
        })))
    } else {
        Err(ApiError::not_found(format!(
            "Session {} not found",
            session_id
        )))
    }
}

/// Get agent factory from app state
///
/// Returns the pre-configured AgentFactory from AppState which includes:
/// - LLM router with all providers
/// - MCP gateway
/// - Browser router (for browser automation tools)
/// - Code router (for code execution)
/// - Worker manager (for orchestration)
fn get_agent_factory(state: &AppState) -> Arc<AgentFactory> {
    state.agent_factory.clone()
}

// ============================================================================
// Permission Handling Endpoints
// ============================================================================

/// Pending permission response
#[derive(Debug, Serialize)]
pub struct PendingPermissionResponse {
    pub request_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub question: String,
    pub options: Vec<PermissionOptionResponse>,
    pub session_id: String,
    pub tool_use_id: Option<String>,
    pub state: String,
    pub created_at: Option<String>,
}

/// Permission option response
#[derive(Debug, Serialize)]
pub struct PermissionOptionResponse {
    pub label: String,
    pub value: String,
    pub is_default: bool,
    pub description: Option<String>,
}

impl From<PendingPermission> for PendingPermissionResponse {
    fn from(pending: PendingPermission) -> Self {
        let state_str = match pending.state {
            gateway_core::agent::PendingPermissionState::Pending => "pending",
            gateway_core::agent::PendingPermissionState::Granted { .. } => "granted",
            gateway_core::agent::PendingPermissionState::Denied => "denied",
            gateway_core::agent::PendingPermissionState::TimedOut => "timed_out",
            gateway_core::agent::PendingPermissionState::Cancelled => "cancelled",
        };

        Self {
            request_id: pending.request.request_id,
            tool_name: pending.request.tool_name,
            tool_input: pending.request.tool_input,
            question: pending.request.question,
            options: pending
                .request
                .options
                .into_iter()
                .map(|opt| PermissionOptionResponse {
                    label: opt.label,
                    value: opt.value,
                    is_default: opt.is_default,
                    description: opt.description,
                })
                .collect(),
            session_id: pending.request.session_id,
            tool_use_id: pending.request.tool_use_id,
            state: state_str.to_string(),
            created_at: pending.request.created_at.map(|dt| dt.to_rfc3339()),
        }
    }
}

/// Get pending permission requests for a session (R4-H10: AuthContext extracted)
pub async fn get_pending_permissions(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<canal_auth::AuthContext>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<PendingPermissionResponse>>, ApiError> {
    let agent_factory = get_agent_factory(&state);
    let agents = agent_factory.list_sessions().await;

    if !agents.contains(&session_id) {
        return Err(ApiError::not_found(format!(
            "Session {} not found",
            session_id
        )));
    }

    let agent_lock = agent_factory.get_or_create(&session_id).await;
    let agent = agent_lock.read().await;

    let pending = agent.get_pending_permissions().await;
    let responses: Vec<PendingPermissionResponse> = pending.into_iter().map(Into::into).collect();

    Ok(Json(responses))
}

/// Permission response request body
#[derive(Debug, Deserialize)]
pub struct SubmitPermissionRequest {
    /// Request ID being responded to
    pub request_id: String,
    /// Whether permission is granted
    pub granted: bool,
    /// Selected option value (e.g., "allow", "deny", "always_allow")
    #[serde(default)]
    pub selected_option: Option<String>,
    /// Optional modified input
    #[serde(default)]
    pub modified_input: Option<serde_json::Value>,
}

/// Permission response result
#[derive(Debug, Serialize)]
pub struct SubmitPermissionResult {
    pub success: bool,
    pub request_id: String,
    pub session_id: String,
    pub message: String,
}

/// Submit a permission response for a session (R4-H10: AuthContext extracted)
pub async fn submit_permission_response(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<canal_auth::AuthContext>,
    Path(session_id): Path<String>,
    Json(request): Json<SubmitPermissionRequest>,
) -> Result<Json<SubmitPermissionResult>, ApiError> {
    tracing::info!(
        session_id = %session_id,
        request_id = %request.request_id,
        granted = request.granted,
        "Submitting permission response"
    );

    let agent_factory = get_agent_factory(&state);
    let agents = agent_factory.list_sessions().await;

    if !agents.contains(&session_id) {
        return Err(ApiError::not_found(format!(
            "Session {} not found",
            session_id
        )));
    }

    let agent_lock = agent_factory.get_or_create(&session_id).await;

    // Create the permission response
    let response = PermissionResponse {
        request_id: request.request_id.clone(),
        session_id: session_id.clone(),
        granted: request.granted,
        selected_option: request.selected_option.clone(),
        modified_input: request.modified_input.clone(),
    };

    // Check if "always allow/deny" requires a write lock (modifies permission rules)
    let needs_write = request.selected_option.as_deref() == Some("always_allow")
        || request.selected_option.as_deref() == Some("always_deny");

    if needs_write {
        // Write lock path: process_permission_response adds permission rules
        let mut agent = agent_lock.write().await;
        agent
            .process_permission_response(response)
            .await
            .map_err(|e| {
                tracing::error!(
                    session_id = %session_id,
                    request_id = %request.request_id,
                    error = %e,
                    "Failed to process permission response (write path)"
                );
                ApiError::internal(format!("Failed to process permission response: {}", e))
            })?;
    } else {
        // Read lock path: just submit to the state channel (no runner mutation needed)
        let agent = agent_lock.read().await;
        let agent_state = agent.state();

        // Check pending permission exists
        let pending = agent_state
            .get_pending_permission(&request.request_id)
            .await;
        if pending.is_none() {
            tracing::warn!(
                session_id = %session_id,
                request_id = %request.request_id,
                "Permission request not found — agent may not be waiting for permissions"
            );
            return Err(ApiError::bad_request(format!(
                "Permission request {} not found. The agent may not be waiting for approval \
                 (check that CANAL_ENV=production is set to enable permission mode).",
                request.request_id
            )));
        }

        // Submit through the channel to the waiting stream
        agent_state
            .submit_permission_response(response)
            .await
            .map_err(|e| {
                tracing::error!(
                    session_id = %session_id,
                    request_id = %request.request_id,
                    error = %e,
                    "Failed to send permission response through channel"
                );
                ApiError::internal(format!(
                    "Failed to send permission response: {}. The agent stream may have ended.",
                    e
                ))
            })?;
    }

    let message = if request.granted {
        "Permission granted"
    } else {
        "Permission denied"
    };

    Ok(Json(SubmitPermissionResult {
        success: true,
        request_id: request.request_id,
        session_id,
        message: message.to_string(),
    }))
}

/// Cancel all pending permissions result
#[derive(Debug, Serialize)]
pub struct CancelPermissionsResult {
    pub success: bool,
    pub session_id: String,
    pub cancelled_count: usize,
}

/// Cancel all pending permission requests for a session (R4-H10: AuthContext extracted)
pub async fn cancel_pending_permissions(
    State(state): State<AppState>,
    axum::Extension(_auth): axum::Extension<canal_auth::AuthContext>,
    Path(session_id): Path<String>,
) -> Result<Json<CancelPermissionsResult>, ApiError> {
    tracing::info!(
        session_id = %session_id,
        "Cancelling all pending permissions"
    );

    let agent_factory = get_agent_factory(&state);
    let agents = agent_factory.list_sessions().await;

    if !agents.contains(&session_id) {
        return Err(ApiError::not_found(format!(
            "Session {} not found",
            session_id
        )));
    }

    let agent_lock = agent_factory.get_or_create(&session_id).await;
    let agent = agent_lock.read().await;

    // Get count before cancelling
    let pending = agent.get_pending_permissions().await;
    let pending_count = pending
        .iter()
        .filter(|p| {
            matches!(
                p.state,
                gateway_core::agent::PendingPermissionState::Pending
            )
        })
        .count();

    // Cancel all pending permissions
    agent.cancel_pending_permissions().await;

    Ok(Json(CancelPermissionsResult {
        success: true,
        session_id,
        cancelled_count: pending_count,
    }))
}
