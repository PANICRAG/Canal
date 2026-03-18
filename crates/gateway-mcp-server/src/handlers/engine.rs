//! Engine namespace handler
//!
//! Handles `engine.*` tool calls by delegating to LlmRouter directly.

use super::HandlerContext;
use crate::dispatcher::DispatchError;
use gateway_core::llm::router::{ChatRequest, ContentBlock, Message};
use canal_identity::types::AgentIdentity;
use tracing::info;

/// Handle an engine namespace tool call
pub async fn handle(
    ctx: &HandlerContext,
    _identity: &AgentIdentity,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, DispatchError> {
    match tool_name {
        "engine.chat" => handle_chat(ctx, arguments).await,
        "engine.models" => handle_models(ctx).await,
        "engine.capabilities" => handle_capabilities(ctx).await,
        _ => Err(DispatchError::ToolNotFound(tool_name.to_string())),
    }
}

async fn handle_chat(
    ctx: &HandlerContext,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, DispatchError> {
    let router = ctx
        .llm_router
        .as_ref()
        .ok_or_else(|| DispatchError::Internal("LLM Router not initialized".to_string()))?;

    let messages = parse_messages(&arguments)?;
    let model = arguments
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let max_tokens = arguments
        .get("max_tokens")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let profile_id = arguments
        .get("profile")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let chat_req = ChatRequest {
        messages,
        model,
        max_tokens,
        temperature: None,
        stream: false,
        tools: Vec::new(),
        tool_choice: None,
        profile_id: profile_id.clone(),
        task_type: None,
        thinking_budget: None,
    };

    info!("Engine chat request via MCP");

    let router_guard = router.read().await;
    let chat_resp = if let Some(ref profile) = profile_id {
        router_guard
            .route_with_profile(profile, chat_req)
            .await
            .map_err(|e| DispatchError::HandlerError(e.to_string()))?
    } else {
        router_guard
            .route(chat_req)
            .await
            .map_err(|e| DispatchError::HandlerError(e.to_string()))?
    };

    // Extract text from response
    let text = chat_resp
        .choices
        .first()
        .map(|c| {
            c.message
                .content_blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    let stop_reason = chat_resp
        .choices
        .first()
        .and_then(|c| c.stop_reason.as_ref().map(|sr| format!("{:?}", sr)))
        .unwrap_or_else(|| "unknown".to_string());

    Ok(serde_json::json!({
        "text": text,
        "model": chat_resp.model,
        "usage": {
            "prompt_tokens": chat_resp.usage.prompt_tokens,
            "completion_tokens": chat_resp.usage.completion_tokens
        },
        "stop_reason": stop_reason
    }))
}

async fn handle_models(ctx: &HandlerContext) -> Result<serde_json::Value, DispatchError> {
    let caps = &ctx.capabilities;
    Ok(serde_json::json!({
        "models": caps.get("models").cloned().unwrap_or(serde_json::json!([])),
        "execution_modes": caps.get("execution_modes").cloned().unwrap_or(serde_json::json!([])),
        "browser_automation": caps.get("browser_automation").and_then(|v| v.as_bool()).unwrap_or(false),
        "code_execution": caps.get("code_execution").and_then(|v| v.as_bool()).unwrap_or(false)
    }))
}

async fn handle_capabilities(ctx: &HandlerContext) -> Result<serde_json::Value, DispatchError> {
    Ok(ctx.capabilities.clone())
}

/// Parse messages from JSON arguments into gateway-core Messages
fn parse_messages(arguments: &serde_json::Value) -> Result<Vec<Message>, DispatchError> {
    let messages_val = arguments
        .get("messages")
        .ok_or_else(|| DispatchError::HandlerError("Missing 'messages' argument".to_string()))?;

    let messages_arr = messages_val
        .as_array()
        .ok_or_else(|| DispatchError::HandlerError("'messages' must be an array".to_string()))?;

    let mut messages = Vec::new();
    for msg in messages_arr {
        let role = msg
            .get("role")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DispatchError::HandlerError("Message missing 'role'".to_string()))?;
        let content = msg
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DispatchError::HandlerError("Message missing 'content'".to_string()))?;

        messages.push(Message {
            role: role.to_string(),
            content: content.to_string(),
            content_blocks: Vec::new(),
        });
    }

    Ok(messages)
}
