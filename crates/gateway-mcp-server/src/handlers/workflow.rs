//! Workflow namespace handler
//!
//! Handles `workflow.*` calls for listing templates and executing workflows.

use super::HandlerContext;
use crate::dispatcher::DispatchError;
use gateway_core::llm::router::{ChatRequest, ContentBlock, Message};
use canal_identity::types::AgentIdentity;

/// Handle a workflow namespace tool call
pub async fn handle(
    ctx: &HandlerContext,
    _identity: &AgentIdentity,
    tool_name: &str,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, DispatchError> {
    match tool_name {
        "workflow.list_templates" => handle_list_templates().await,
        "workflow.execute" => handle_execute(ctx, arguments).await,
        _ => Err(DispatchError::ToolNotFound(tool_name.to_string())),
    }
}

/// List available workflow templates (static catalog of known patterns)
async fn handle_list_templates() -> Result<serde_json::Value, DispatchError> {
    Ok(serde_json::json!({
        "templates": [
            {
                "id": "simple",
                "name": "Simple",
                "description": "Single agent, no verification: [Agent] → [END]",
                "pattern": "Simple"
            },
            {
                "id": "with_verification",
                "name": "WithVerification",
                "description": "Agent with verification loop: [Agent] → [Verify] → (pass) → [END] / (fail) → [Agent]",
                "pattern": "WithVerification"
            },
            {
                "id": "plan_execute",
                "name": "PlanExecute",
                "description": "Plan then execute: [Planner] → [Executor] → [Synthesizer] → [END]",
                "pattern": "PlanExecute"
            },
            {
                "id": "research",
                "name": "Research",
                "description": "Parallel research: [QueryPlanner] → [Parallel Searches] → [Merge] → [END]",
                "pattern": "Research"
            },
            {
                "id": "full",
                "name": "Full",
                "description": "Auto-select template based on task classification",
                "pattern": "Full"
            }
        ]
    }))
}

/// Get the system prompt for a given template
fn template_system_prompt(template: &str) -> Option<&'static str> {
    match template {
        "simple" => None, // No system prompt — plain chat
        "plan_execute" => Some(
            "You are executing a plan-then-execute workflow. Follow these steps:\n\
             1. PLAN: Analyze the task and create a numbered step-by-step plan.\n\
             2. EXECUTE: Work through each step sequentially, showing your work.\n\
             3. SYNTHESIZE: Summarize the results and provide the final answer.\n\
             Structure your response with clear headings: ## Plan, ## Execution, ## Result.",
        ),
        "research" => Some(
            "You are executing a research workflow. Follow these steps:\n\
             1. Break the query into 2-5 independent sub-questions.\n\
             2. For each sub-question, gather relevant information and analysis.\n\
             3. Merge findings into a coherent, comprehensive answer.\n\
             4. Cite which sub-questions contributed to each conclusion.\n\
             Structure your response with clear headings for each research strand.",
        ),
        "with_verification" => Some(
            "You are executing a task with self-verification. Follow these steps:\n\
             1. Produce your initial answer or solution.\n\
             2. VERIFY: Critically review your answer for correctness, completeness, and edge cases.\n\
             3. If verification finds issues, revise and re-verify.\n\
             4. Present the final verified result.\n\
             Structure your response with: ## Draft, ## Verification, ## Final Result.",
        ),
        "full" | "auto" => Some(
            "You are an advanced AI assistant. Analyze the task and automatically select \
             the best approach:\n\
             - For simple questions: answer directly.\n\
             - For complex tasks: plan first, then execute step by step.\n\
             - For research queries: break into sub-questions, research each, then merge.\n\
             - For code/analysis: produce a draft, verify it, then present the final result.\n\
             Choose the approach that best fits this specific task.",
        ),
        _ => None,
    }
}

/// Execute a workflow by delegating to the LLM router
async fn handle_execute(
    ctx: &HandlerContext,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, DispatchError> {
    let router = ctx
        .llm_router
        .as_ref()
        .ok_or_else(|| DispatchError::Internal("LLM Router not initialized".to_string()))?;

    let template = arguments
        .get("template")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DispatchError::HandlerError("Missing 'template' argument".to_string()))?;

    let input = arguments
        .get("input")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DispatchError::HandlerError("Missing 'input' argument".to_string()))?;

    let model = arguments
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Validate template name
    match template {
        "simple" | "plan_execute" | "research" | "with_verification" | "full" | "auto" => {}
        _ => {
            return Err(DispatchError::HandlerError(format!(
                "Unknown template: '{}'. Use one of: simple, plan_execute, research, with_verification, full",
                template
            )));
        }
    }

    // Build messages with template-specific system prompt
    let mut messages = Vec::new();
    if let Some(system_prompt) = template_system_prompt(template) {
        messages.push(Message {
            role: "system".to_string(),
            content: system_prompt.to_string(),
            content_blocks: Vec::new(),
        });
    }
    messages.push(Message {
        role: "user".to_string(),
        content: input.to_string(),
        content_blocks: Vec::new(),
    });

    let chat_req = ChatRequest {
        messages,
        model,
        max_tokens: arguments
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        temperature: None,
        stream: false,
        tools: Vec::new(),
        tool_choice: None,
        profile_id: None,
        task_type: None,
        thinking_budget: None,
    };

    let router_guard = router.read().await;
    let chat_resp = router_guard
        .route(chat_req)
        .await
        .map_err(|e| DispatchError::HandlerError(e.to_string()))?;

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
        "template": template,
        "result": text,
        "model": chat_resp.model,
        "usage": {
            "prompt_tokens": chat_resp.usage.prompt_tokens,
            "completion_tokens": chat_resp.usage.completion_tokens
        },
        "stop_reason": stop_reason
    }))
}
