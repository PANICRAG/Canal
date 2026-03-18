//! Browser Tool Loop Integration Tests
//!
//! Tests that simulate the Tauri desktop app calling the AgentFactory
//! to execute browser automation tasks (screenshot, click, type, scroll, etc.).
//!
//! These tests verify:
//! - The agent loop correctly executes multi-turn tool calls
//! - Tool results are fed back to the LLM for continued reasoning
//! - The stream produces all expected messages (Assistant, User tool_result, Result)
//! - AgentFactory session management works correctly
//! - Error handling during tool execution doesn't break the loop
//!
//! Run with: `cargo test --package gateway-core --test browser_tool_loop_integration -- --nocapture`

mod helpers;

use futures::StreamExt;
use gateway_core::agent::r#loop::config::AgentConfig;
use gateway_core::agent::r#loop::runner::AgentRunner;
use gateway_core::agent::types::PermissionMode;
use gateway_core::agent::{AgentLoop, AgentMessage, ContentBlock};
use helpers::mock_llm::{MockLlmClient, MockLlmResponse, ToolUseBlock};
use helpers::mock_tools::{MockToolExecutor, MockToolResult};
use helpers::scenario_builder::{collect_messages, has_result_message, ScenarioBuilder};
use serde_json::json;
use std::sync::Arc;

// =============================================================================
// Helper: Browser tool schemas and mock results
// =============================================================================

fn computer_screenshot_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "include_accessibility": {"type": "boolean"},
            "interactive_only": {"type": "boolean"},
            "tab_id": {"type": "integer"},
            "quality": {"type": "integer"},
            "max_width": {"type": "integer"},
            "timeout_ms": {"type": "integer"}
        }
    })
}

fn computer_click_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "x": {"type": "integer"},
            "y": {"type": "integer"},
            "button": {"type": "string"},
            "click_type": {"type": "string"}
        },
        "required": ["x", "y"]
    })
}

fn computer_click_ref_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "ref": {"type": "string"}
        },
        "required": ["ref"]
    })
}

fn computer_type_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "text": {"type": "string"},
            "submit": {"type": "boolean"}
        },
        "required": ["text"]
    })
}

fn computer_scroll_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "x": {"type": "integer"},
            "y": {"type": "integer"},
            "direction": {"type": "string"},
            "amount": {"type": "integer"}
        },
        "required": ["x", "y", "direction"]
    })
}

fn computer_key_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "key": {"type": "string"}
        },
        "required": ["key"]
    })
}

fn browser_navigate_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "url": {"type": "string"}
        },
        "required": ["url"]
    })
}

fn mock_screenshot_result() -> serde_json::Value {
    json!({
        "success": true,
        "image_data": "iVBORw0KGgoAAAANSUhEUg==",
        "format": "jpeg",
        "image_width": 600,
        "image_height": 338,
        "viewport_width": 1920,
        "viewport_height": 1080,
        "url": "https://mail.google.com",
        "title": "Gmail - Inbox"
    })
}

fn mock_screenshot_with_a11y_result() -> serde_json::Value {
    json!({
        "success": true,
        "image_data": "iVBORw0KGgoAAAANSUhEUg==",
        "format": "jpeg",
        "image_width": 600,
        "image_height": 338,
        "viewport_width": 1920,
        "viewport_height": 1080,
        "url": "https://mail.google.com",
        "title": "Gmail - Inbox",
        "accessibility_tree": [
            {"ref": "@e1", "role": "button", "name": "Compose", "bbox": {"x": 50, "y": 100, "width": 100, "height": 40}},
            {"ref": "@e2", "role": "link", "name": "Inbox (3)", "bbox": {"x": 50, "y": 200, "width": 120, "height": 30}},
            {"ref": "@e3", "role": "link", "name": "Sent", "bbox": {"x": 50, "y": 240, "width": 60, "height": 30}}
        ],
        "accessibility_formatted": "@e1: button \"Compose\" [50,100 100x40]\n@e2: link \"Inbox (3)\" [50,200 120x30]\n@e3: link \"Sent\" [50,240 60x30]",
        "coordinate_note": "All coordinates are in IMAGE space (600x338)."
    })
}

fn mock_click_result() -> serde_json::Value {
    json!({
        "success": true,
        "clicked_at": {"x": 320, "y": 160},
        "element_info": "button: Compose"
    })
}

fn mock_click_ref_result() -> serde_json::Value {
    json!({
        "success": true,
        "ref": "@e1",
        "clicked_at": {"x": 100, "y": 120}
    })
}

fn mock_type_result() -> serde_json::Value {
    json!({
        "success": true,
        "typed_text": "Hello, this is a test message"
    })
}

fn mock_scroll_result() -> serde_json::Value {
    json!({
        "success": true,
        "scrolled": {"direction": "down", "amount": 3}
    })
}

fn mock_key_result() -> serde_json::Value {
    json!({
        "success": true,
        "key": "Enter"
    })
}

fn mock_navigate_result() -> serde_json::Value {
    json!({
        "success": true,
        "url": "https://mail.google.com",
        "title": "Gmail"
    })
}

/// Register all browser tools on a MockToolExecutor
async fn register_browser_tools(tools: &MockToolExecutor) {
    tools
        .register_tool(
            "computer_screenshot",
            "Take a screenshot of the current browser tab",
            computer_screenshot_schema(),
            MockToolResult::Success(mock_screenshot_result()),
        )
        .await;

    tools
        .register_tool(
            "computer_click",
            "Click at coordinates on the screen",
            computer_click_schema(),
            MockToolResult::Success(mock_click_result()),
        )
        .await;

    tools
        .register_tool(
            "computer_click_ref",
            "Click on an accessibility element by ref",
            computer_click_ref_schema(),
            MockToolResult::Success(mock_click_ref_result()),
        )
        .await;

    tools
        .register_tool(
            "computer_type",
            "Type text into the focused element",
            computer_type_schema(),
            MockToolResult::Success(mock_type_result()),
        )
        .await;

    tools
        .register_tool(
            "computer_scroll",
            "Scroll the page",
            computer_scroll_schema(),
            MockToolResult::Success(mock_scroll_result()),
        )
        .await;

    tools
        .register_tool(
            "computer_key",
            "Press a keyboard key",
            computer_key_schema(),
            MockToolResult::Success(mock_key_result()),
        )
        .await;

    tools
        .register_tool(
            "browser_navigate",
            "Navigate to a URL",
            browser_navigate_schema(),
            MockToolResult::Success(mock_navigate_result()),
        )
        .await;
}

/// Build a scenario with browser tools registered
async fn browser_scenario(
    llm_responses: Vec<MockLlmResponse>,
) -> (AgentRunner, Arc<MockLlmClient>, Arc<MockToolExecutor>) {
    let mock_llm = Arc::new(MockLlmClient::new());
    mock_llm.queue_responses(llm_responses).await;

    let mock_tools = Arc::new(MockToolExecutor::new());
    register_browser_tools(&mock_tools).await;

    let mut config = AgentConfig::default();
    config.max_turns = 20;
    config.permission_mode = PermissionMode::BypassPermissions;

    let runner = AgentRunner::new(config)
        .with_llm(mock_llm.clone() as Arc<dyn gateway_core::agent::r#loop::runner::LlmClient>)
        .with_tools(
            mock_tools.clone() as Arc<dyn gateway_core::agent::r#loop::runner::ToolExecutor>
        );

    (runner, mock_llm, mock_tools)
}

/// Helper: extract tool names from ToolUse blocks in AssistantMessages
fn extract_tool_calls(messages: &[AgentMessage]) -> Vec<String> {
    let mut names = Vec::new();
    for msg in messages {
        if let AgentMessage::Assistant(a) = msg {
            for block in &a.content {
                if let ContentBlock::ToolUse { name, .. } = block {
                    names.push(name.clone());
                }
            }
        }
    }
    names
}

/// Helper: extract tool results from UserMessages
fn extract_tool_results(messages: &[AgentMessage]) -> Vec<serde_json::Value> {
    let mut results = Vec::new();
    for msg in messages {
        if let AgentMessage::User(u) = msg {
            if let Some(ref result) = u.tool_use_result {
                results.push(result.clone());
            }
        }
    }
    results
}

/// Helper: get the ResultMessage from the stream
fn get_result(messages: &[AgentMessage]) -> Option<&gateway_core::agent::types::ResultMessage> {
    messages.iter().find_map(|m| {
        if let AgentMessage::Result(r) = m {
            Some(r)
        } else {
            None
        }
    })
}

// =============================================================================
// Test: Single screenshot call
// =============================================================================

#[tokio::test]
async fn test_single_screenshot_tool_call() {
    // LLM calls screenshot once, then provides a text summary
    let (mut runner, mock_llm, mock_tools) = browser_scenario(vec![
        MockLlmResponse::ToolUse {
            name: "computer_screenshot".into(),
            input: json!({}),
        },
        MockLlmResponse::Text("I can see the Gmail inbox with 3 unread emails.".into()),
    ])
    .await;

    let messages = collect_messages(&mut runner, "What's on screen?").await;

    // Verify the stream contains expected messages
    assert!(
        has_result_message(&messages),
        "Stream should end with a Result message"
    );

    // Check tool calls
    let tool_calls = extract_tool_calls(&messages);
    assert_eq!(
        tool_calls,
        vec!["computer_screenshot"],
        "Should call screenshot exactly once"
    );

    // Check tool results fed back
    let tool_results = extract_tool_results(&messages);
    assert_eq!(tool_results.len(), 1, "Should have 1 tool result");
    assert_eq!(tool_results[0]["success"], true);

    // LLM should be called twice: once for screenshot, once for final response
    assert_eq!(
        mock_llm.call_count(),
        2,
        "LLM called twice (tool_use + final text)"
    );
    assert_eq!(mock_tools.call_count(), 1, "Tool executed once");

    // Result should indicate success
    let result = get_result(&messages).expect("Should have Result");
    assert!(!result.is_error, "Should succeed");
    assert_eq!(result.num_turns, 2, "Should take 2 turns");
}

// =============================================================================
// Test: Screenshot → Click → Type workflow (multi-turn)
// =============================================================================

#[tokio::test]
async fn test_screenshot_click_type_multiturn() {
    // Simulates: Take screenshot → Click compose → Type message → Done
    let (mut runner, mock_llm, mock_tools) = browser_scenario(vec![
        // Turn 1: LLM sees prompt, takes screenshot
        MockLlmResponse::TextThenTool {
            text: "Let me take a screenshot first.".into(),
            tool_use: ToolUseBlock::new("computer_screenshot", json!({})),
        },
        // Turn 2: LLM sees screenshot, clicks compose button
        MockLlmResponse::TextThenTool {
            text: "I can see Gmail. Let me click the Compose button.".into(),
            tool_use: ToolUseBlock::new("computer_click", json!({"x": 100, "y": 120})),
        },
        // Turn 3: LLM sees click result, types message
        MockLlmResponse::TextThenTool {
            text: "Compose window is open. Typing the message now.".into(),
            tool_use: ToolUseBlock::new("computer_type", json!({"text": "Hello, world!"})),
        },
        // Turn 4: LLM confirms completion
        MockLlmResponse::Text("I've composed the email with 'Hello, world!' as the body.".into()),
    ])
    .await;

    let messages = collect_messages(
        &mut runner,
        "Compose a new email in Gmail saying Hello, world!",
    )
    .await;

    // Verify stream structure
    assert!(has_result_message(&messages), "Should end with Result");

    let tool_calls = extract_tool_calls(&messages);
    assert_eq!(
        tool_calls,
        vec!["computer_screenshot", "computer_click", "computer_type"],
        "Should call screenshot → click → type in order"
    );

    let tool_results = extract_tool_results(&messages);
    assert_eq!(tool_results.len(), 3, "Should have 3 tool results");

    // Verify each tool result was successful
    for (i, result) in tool_results.iter().enumerate() {
        assert_eq!(
            result["success"], true,
            "Tool result {} should be success",
            i
        );
    }

    // LLM called 4 times (3 tool_use + 1 final text)
    assert_eq!(mock_llm.call_count(), 4);
    assert_eq!(mock_tools.call_count(), 3);

    // Verify tool call inputs
    let tool_log = mock_tools.call_log().await;
    assert_eq!(tool_log[0].tool_name, "computer_screenshot");
    assert_eq!(tool_log[1].tool_name, "computer_click");
    assert_eq!(tool_log[1].input["x"], 100);
    assert_eq!(tool_log[1].input["y"], 120);
    assert_eq!(tool_log[2].tool_name, "computer_type");
    assert_eq!(tool_log[2].input["text"], "Hello, world!");
}

// =============================================================================
// Test: Screenshot with accessibility → Click ref workflow
// =============================================================================

#[tokio::test]
async fn test_screenshot_accessibility_click_ref() {
    let mock_tools = Arc::new(MockToolExecutor::new());

    // Register screenshot with a dynamic handler that returns accessibility data
    mock_tools
        .register_tool(
            "computer_screenshot",
            "Take screenshot",
            computer_screenshot_schema(),
            MockToolResult::DynamicFn(Arc::new(|input| {
                if input.get("include_accessibility").and_then(|v| v.as_bool()) == Some(true) {
                    mock_screenshot_with_a11y_result()
                } else {
                    mock_screenshot_result()
                }
            })),
        )
        .await;

    mock_tools
        .register_tool(
            "computer_click_ref",
            "Click element by ref",
            computer_click_ref_schema(),
            MockToolResult::Success(mock_click_ref_result()),
        )
        .await;

    let mock_llm = Arc::new(MockLlmClient::new());
    mock_llm
        .queue_responses(vec![
            // Turn 1: Take screenshot with accessibility tree
            MockLlmResponse::ToolUse {
                name: "computer_screenshot".into(),
                input: json!({"include_accessibility": true}),
            },
            // Turn 2: Click the Compose button using ref
            MockLlmResponse::TextThenTool {
                text: "I see the Gmail inbox. The Compose button is @e1.".into(),
                tool_use: ToolUseBlock::new("computer_click_ref", json!({"ref": "@e1"})),
            },
            // Turn 3: Done
            MockLlmResponse::Text("Clicked Compose. The compose window should open.".into()),
        ])
        .await;

    let mut config = AgentConfig::default();
    config.max_turns = 20;
    config.permission_mode = PermissionMode::BypassPermissions;

    let mut runner = AgentRunner::new(config)
        .with_llm(mock_llm.clone() as Arc<dyn gateway_core::agent::r#loop::runner::LlmClient>)
        .with_tools(
            mock_tools.clone() as Arc<dyn gateway_core::agent::r#loop::runner::ToolExecutor>
        );

    let messages = collect_messages(&mut runner, "Click the Compose button in Gmail").await;

    assert!(has_result_message(&messages));

    let tool_calls = extract_tool_calls(&messages);
    assert_eq!(
        tool_calls,
        vec!["computer_screenshot", "computer_click_ref"]
    );

    // Verify screenshot was called with accessibility=true
    let tool_log = mock_tools.call_log().await;
    assert_eq!(tool_log[0].input["include_accessibility"], true);

    // Verify click_ref was called with correct ref
    assert_eq!(tool_log[1].input["ref"], "@e1");

    // Verify the screenshot result included accessibility data
    let tool_results = extract_tool_results(&messages);
    assert!(
        tool_results[0].get("accessibility_tree").is_some(),
        "Screenshot result should include accessibility tree"
    );
}

// =============================================================================
// Test: Navigate → Screenshot → Scroll → Screenshot workflow
// =============================================================================

#[tokio::test]
async fn test_navigate_screenshot_scroll() {
    let (mut runner, _, _mock_tools) = browser_scenario(vec![
        // Turn 1: Navigate to URL
        MockLlmResponse::ToolUse {
            name: "browser_navigate".into(),
            input: json!({"url": "https://example.com"}),
        },
        // Turn 2: Take screenshot
        MockLlmResponse::ToolUse {
            name: "computer_screenshot".into(),
            input: json!({}),
        },
        // Turn 3: Scroll down
        MockLlmResponse::ToolUse {
            name: "computer_scroll".into(),
            input: json!({"x": 300, "y": 300, "direction": "down", "amount": 3}),
        },
        // Turn 4: Take another screenshot
        MockLlmResponse::ToolUse {
            name: "computer_screenshot".into(),
            input: json!({}),
        },
        // Turn 5: Report findings
        MockLlmResponse::Text(
            "I navigated to example.com and scrolled down. The page shows...".into(),
        ),
    ])
    .await;

    let messages = collect_messages(&mut runner, "Go to example.com and scroll down").await;

    assert!(has_result_message(&messages));

    let tool_calls = extract_tool_calls(&messages);
    assert_eq!(
        tool_calls,
        vec![
            "browser_navigate",
            "computer_screenshot",
            "computer_scroll",
            "computer_screenshot"
        ]
    );

    // Verify all 4 tool results were fed back
    let tool_results = extract_tool_results(&messages);
    assert_eq!(tool_results.len(), 4);

    // 5 turns total
    let result = get_result(&messages).unwrap();
    assert_eq!(result.num_turns, 5);
}

// =============================================================================
// Test: Tool execution error doesn't break the loop
// =============================================================================

#[tokio::test]
async fn test_tool_error_continues_loop() {
    let mock_tools = Arc::new(MockToolExecutor::new());

    // Screenshot works fine
    mock_tools
        .register_tool(
            "computer_screenshot",
            "Take screenshot",
            computer_screenshot_schema(),
            MockToolResult::Success(mock_screenshot_result()),
        )
        .await;

    // Click fails
    mock_tools
        .register_tool(
            "computer_click",
            "Click at coordinates",
            computer_click_schema(),
            MockToolResult::Error("Element not found at coordinates".into()),
        )
        .await;

    let mock_llm = Arc::new(MockLlmClient::new());
    mock_llm.queue_responses(vec![
        // Turn 1: Screenshot
        MockLlmResponse::ToolUse {
            name: "computer_screenshot".into(),
            input: json!({}),
        },
        // Turn 2: Click (will fail)
        MockLlmResponse::ToolUse {
            name: "computer_click".into(),
            input: json!({"x": 999, "y": 999}),
        },
        // Turn 3: LLM recovers from error and tries again or gives up
        MockLlmResponse::Text("The click failed because the element wasn't at those coordinates. Let me try another approach.".into()),
    ]).await;

    let mut config = AgentConfig::default();
    config.max_turns = 20;
    config.permission_mode = PermissionMode::BypassPermissions;

    let mut runner = AgentRunner::new(config)
        .with_llm(mock_llm.clone() as Arc<dyn gateway_core::agent::r#loop::runner::LlmClient>)
        .with_tools(
            mock_tools.clone() as Arc<dyn gateway_core::agent::r#loop::runner::ToolExecutor>
        );

    let messages = collect_messages(&mut runner, "Click at position 999,999").await;

    // Stream should still complete with Result
    assert!(
        has_result_message(&messages),
        "Should complete despite tool error"
    );

    let tool_calls = extract_tool_calls(&messages);
    assert_eq!(tool_calls, vec!["computer_screenshot", "computer_click"]);

    // The click tool result should indicate error
    let tool_results = extract_tool_results(&messages);
    assert_eq!(tool_results.len(), 2);
    // First result (screenshot) should succeed
    assert_eq!(tool_results[0]["success"], true);
    // Second result (click) should have error
    assert!(
        tool_results[1].get("error").is_some(),
        "Click result should contain error"
    );

    // LLM gets 3 calls: screenshot tool_use, click tool_use (error fed back), final text
    assert_eq!(mock_llm.call_count(), 3);

    let result = get_result(&messages).unwrap();
    assert!(
        !result.is_error,
        "Overall result should succeed (LLM recovered)"
    );
}

// =============================================================================
// Test: Multiple tools in one LLM response (parallel execution)
// =============================================================================

#[tokio::test]
async fn test_parallel_tool_execution() {
    let (mut runner, mock_llm, mock_tools) = browser_scenario(vec![
        // Turn 1: LLM requests both screenshot and navigate simultaneously
        MockLlmResponse::MultiToolUse(vec![
            ToolUseBlock::new("computer_screenshot", json!({})),
            ToolUseBlock::new("browser_navigate", json!({"url": "https://example.com"})),
        ]),
        // Turn 2: Final response
        MockLlmResponse::Text("I took a screenshot and navigated to example.com.".into()),
    ])
    .await;

    let messages = collect_messages(&mut runner, "Screenshot and navigate").await;

    assert!(has_result_message(&messages));

    let tool_calls = extract_tool_calls(&messages);
    assert_eq!(tool_calls.len(), 2, "Should have 2 tool calls");
    assert!(tool_calls.contains(&"computer_screenshot".to_string()));
    assert!(tool_calls.contains(&"browser_navigate".to_string()));

    // Both tool results should be fed back
    let tool_results = extract_tool_results(&messages);
    assert_eq!(tool_results.len(), 2);

    assert_eq!(mock_llm.call_count(), 2);
    assert_eq!(mock_tools.call_count(), 2);
}

// =============================================================================
// Test: Max turns exceeded during browser task
// =============================================================================

#[tokio::test]
async fn test_max_turns_exceeded() {
    let mock_llm = Arc::new(MockLlmClient::new());
    // LLM always requests screenshot (infinite loop)
    mock_llm
        .set_default_response(MockLlmResponse::ToolUse {
            name: "computer_screenshot".into(),
            input: json!({}),
        })
        .await;

    let mock_tools = Arc::new(MockToolExecutor::new());
    register_browser_tools(&mock_tools).await;

    let mut config = AgentConfig::default();
    config.max_turns = 3; // Very low limit
    config.permission_mode = PermissionMode::BypassPermissions;

    let mut runner = AgentRunner::new(config)
        .with_llm(mock_llm.clone() as Arc<dyn gateway_core::agent::r#loop::runner::LlmClient>)
        .with_tools(
            mock_tools.clone() as Arc<dyn gateway_core::agent::r#loop::runner::ToolExecutor>
        );

    let messages = collect_messages(&mut runner, "Keep taking screenshots forever").await;

    // Should still have a Result
    assert!(has_result_message(&messages));

    let result = get_result(&messages).unwrap();
    assert!(result.is_error, "Should be error due to max turns");
    assert!(
        matches!(
            result.subtype,
            gateway_core::agent::types::ResultSubtype::ErrorMaxTurns
        ),
        "Should be ErrorMaxTurns"
    );

    // Should have exactly 3 tool calls (one per turn before max turns hit)
    assert_eq!(
        mock_llm.call_count(),
        3,
        "LLM called exactly max_turns times"
    );
}

// =============================================================================
// Test: LLM error during multi-turn browser task
// =============================================================================

#[tokio::test]
async fn test_llm_error_during_tool_loop() {
    let mock_llm = Arc::new(MockLlmClient::new());
    mock_llm
        .queue_responses(vec![
            // Turn 1: Screenshot (succeeds)
            MockLlmResponse::ToolUse {
                name: "computer_screenshot".into(),
                input: json!({}),
            },
            // Turn 2: LLM API error
            MockLlmResponse::Error("Rate limit exceeded".into()),
        ])
        .await;

    let mock_tools = Arc::new(MockToolExecutor::new());
    register_browser_tools(&mock_tools).await;

    let mut config = AgentConfig::default();
    config.max_turns = 20;
    config.permission_mode = PermissionMode::BypassPermissions;

    let mut runner = AgentRunner::new(config)
        .with_llm(mock_llm.clone() as Arc<dyn gateway_core::agent::r#loop::runner::LlmClient>)
        .with_tools(
            mock_tools.clone() as Arc<dyn gateway_core::agent::r#loop::runner::ToolExecutor>
        );

    let messages = collect_messages(&mut runner, "What's on screen?").await;

    // The first screenshot should have executed
    assert_eq!(
        mock_tools.call_count(),
        1,
        "Screenshot should execute before LLM error"
    );

    // Stream should contain an error (system message from collect_messages helper)
    let has_error = messages.iter().any(|m| {
        if let AgentMessage::System(s) = m {
            s.subtype == "error"
        } else {
            false
        }
    });
    assert!(has_error, "Should have error message from LLM failure");
}

// =============================================================================
// Test: Two consecutive screenshots (the exact failing scenario)
// =============================================================================

#[tokio::test]
async fn test_two_consecutive_screenshots_with_accessibility() {
    // This test reproduces the exact scenario that was failing:
    // screenshot → text → screenshot(accessibility) → should get tool_result → text
    let mock_tools = Arc::new(MockToolExecutor::new());

    // Use dynamic handler to return different results based on input
    mock_tools
        .register_tool(
            "computer_screenshot",
            "Take screenshot",
            computer_screenshot_schema(),
            MockToolResult::DynamicFn(Arc::new(|input| {
                if input.get("include_accessibility").and_then(|v| v.as_bool()) == Some(true) {
                    mock_screenshot_with_a11y_result()
                } else {
                    mock_screenshot_result()
                }
            })),
        )
        .await;

    let mock_llm = Arc::new(MockLlmClient::new());
    mock_llm.queue_responses(vec![
        // Turn 1: First screenshot (no accessibility)
        MockLlmResponse::ToolUse {
            name: "computer_screenshot".into(),
            input: json!({}),
        },
        // Turn 2: Second screenshot WITH accessibility
        MockLlmResponse::TextThenTool {
            text: "I can see the Gmail inbox. Let me get the accessibility tree for precise clicking.".into(),
            tool_use: ToolUseBlock::new(
                "computer_screenshot",
                json!({"include_accessibility": true}),
            ),
        },
        // Turn 3: Final response using accessibility info
        MockLlmResponse::Text("I can see the Compose button (@e1), Inbox (@e2), and Sent (@e3).".into()),
    ]).await;

    let mut config = AgentConfig::default();
    config.max_turns = 20;
    config.permission_mode = PermissionMode::BypassPermissions;

    let mut runner = AgentRunner::new(config)
        .with_llm(mock_llm.clone() as Arc<dyn gateway_core::agent::r#loop::runner::LlmClient>)
        .with_tools(
            mock_tools.clone() as Arc<dyn gateway_core::agent::r#loop::runner::ToolExecutor>
        );

    let messages = collect_messages(
        &mut runner,
        "Analyze the Gmail inbox with accessibility data",
    )
    .await;

    // CRITICAL: Stream must complete with a Result
    assert!(
        has_result_message(&messages),
        "Stream MUST end with Result (not terminate early)"
    );

    // Both screenshots should have been called
    let tool_calls = extract_tool_calls(&messages);
    assert_eq!(
        tool_calls,
        vec!["computer_screenshot", "computer_screenshot"],
        "Should have exactly 2 screenshot calls"
    );

    // Both tool results should be present
    let tool_results = extract_tool_results(&messages);
    assert_eq!(
        tool_results.len(),
        2,
        "MUST have 2 tool results (one per screenshot)"
    );

    // First result: basic screenshot
    assert_eq!(tool_results[0]["success"], true);
    assert!(
        tool_results[0].get("accessibility_tree").is_none(),
        "First screenshot should NOT have accessibility tree"
    );

    // Second result: screenshot with accessibility
    assert_eq!(tool_results[1]["success"], true);
    assert!(
        tool_results[1].get("accessibility_tree").is_some(),
        "Second screenshot MUST have accessibility tree"
    );

    // LLM should be called 3 times
    assert_eq!(
        mock_llm.call_count(),
        3,
        "LLM called 3 times (2 screenshots + 1 final text)"
    );
    assert_eq!(mock_tools.call_count(), 2, "Tool executed 2 times");

    // Result should indicate success
    let result = get_result(&messages).unwrap();
    assert!(!result.is_error, "Should succeed");
    assert_eq!(result.num_turns, 3);
}

// =============================================================================
// Test: Agent factory session persistence across queries
// =============================================================================

#[tokio::test]
async fn test_agent_factory_session_reuse() {
    use gateway_core::agent::AgentFactory;
    use gateway_core::llm::{LlmConfig, LlmRouter};

    let llm_config = LlmConfig::default();
    let llm_router = LlmRouter::new(llm_config);

    let factory = AgentFactory::new(Arc::new(llm_router))
        .with_max_turns(20)
        .with_permission_mode(PermissionMode::BypassPermissions);

    let session_id = "test-session-001";

    // Get or create agent for session
    let agent1 = factory.get_or_create(session_id).await;
    let agent2 = factory.get_or_create(session_id).await;

    // Should return the same Arc (same session)
    assert!(
        Arc::ptr_eq(&agent1, &agent2),
        "Same session_id should return same agent"
    );

    // Different session should be different
    let agent3 = factory.get_or_create("different-session").await;
    assert!(
        !Arc::ptr_eq(&agent1, &agent3),
        "Different session_id should return different agent"
    );
}

// =============================================================================
// Test: Stream event ordering (message types in correct order)
// =============================================================================

#[tokio::test]
async fn test_stream_message_ordering() {
    let (mut runner, _, _) = browser_scenario(vec![
        // Turn 1: Tool call
        MockLlmResponse::TextThenTool {
            text: "Taking screenshot.".into(),
            tool_use: ToolUseBlock::new("computer_screenshot", json!({})),
        },
        // Turn 2: Final text
        MockLlmResponse::Text("Done.".into()),
    ])
    .await;

    let messages = collect_messages(&mut runner, "Screenshot please").await;

    // Expected message order:
    // 1. System (agent_instructions) - first query only
    // 2. User (the user's prompt)
    // 3. Assistant (text + tool_use) - Turn 1 LLM response
    // 4. User (tool_result) - Tool execution result
    // 5. Assistant (text) - Turn 2 LLM response
    // 6. Result (done)

    // Verify ordering
    let mut found_system = false;
    let mut found_user_prompt = false;
    let mut found_assistant_tool = false;
    let mut found_user_result = false;
    let mut found_assistant_final = false;
    let mut found_result = false;

    for msg in &messages {
        match msg {
            AgentMessage::System(_) if !found_system => {
                found_system = true;
                assert!(!found_user_prompt, "System must come before user prompt");
            }
            AgentMessage::User(u) if !found_user_prompt && u.tool_use_result.is_none() => {
                found_user_prompt = true;
                assert!(found_system, "User prompt must come after system");
            }
            AgentMessage::Assistant(a) if !found_assistant_tool => {
                found_assistant_tool = true;
                assert!(found_user_prompt, "Assistant must come after user prompt");
                let has_tool = a.content.iter().any(|b| b.is_tool_use());
                assert!(has_tool, "First assistant message should have tool_use");
            }
            AgentMessage::User(u) if u.tool_use_result.is_some() => {
                found_user_result = true;
                assert!(
                    found_assistant_tool,
                    "Tool result must come after assistant tool_use"
                );
            }
            AgentMessage::Assistant(_) if found_user_result && !found_assistant_final => {
                found_assistant_final = true;
            }
            AgentMessage::Result(_) => {
                found_result = true;
            }
            _ => {}
        }
    }

    assert!(found_system, "Should have system message");
    assert!(found_user_prompt, "Should have user prompt");
    assert!(found_assistant_tool, "Should have assistant with tool_use");
    assert!(found_user_result, "Should have user with tool_result");
    assert!(found_assistant_final, "Should have final assistant message");
    assert!(found_result, "Should have Result");
}

// =============================================================================
// Test: Keyboard shortcut workflow (key press tool)
// =============================================================================

#[tokio::test]
async fn test_keyboard_shortcut_workflow() {
    let (mut runner, _, mock_tools) = browser_scenario(vec![
        // Turn 1: Press Ctrl+A to select all
        MockLlmResponse::ToolUse {
            name: "computer_key".into(),
            input: json!({"key": "ctrl+a"}),
        },
        // Turn 2: Press Delete
        MockLlmResponse::ToolUse {
            name: "computer_key".into(),
            input: json!({"key": "Delete"}),
        },
        // Turn 3: Type new text
        MockLlmResponse::ToolUse {
            name: "computer_type".into(),
            input: json!({"text": "New content", "submit": false}),
        },
        // Turn 4: Done
        MockLlmResponse::Text("I've replaced the text with 'New content'.".into()),
    ])
    .await;

    let messages = collect_messages(
        &mut runner,
        "Select all text and replace it with 'New content'",
    )
    .await;

    assert!(has_result_message(&messages));

    let tool_calls = extract_tool_calls(&messages);
    assert_eq!(
        tool_calls,
        vec!["computer_key", "computer_key", "computer_type"]
    );
    assert_eq!(mock_tools.call_count(), 3);

    let result = get_result(&messages).unwrap();
    assert!(!result.is_error);
    assert_eq!(result.num_turns, 4);
}

// =============================================================================
// Test: ScenarioBuilder with browser tools
// =============================================================================

#[tokio::test]
async fn test_scenario_builder_browser_tools() {
    let mut scenario = ScenarioBuilder::new()
        .with_tool(
            "computer_screenshot",
            "Take screenshot",
            computer_screenshot_schema(),
            MockToolResult::Success(mock_screenshot_result()),
        )
        .with_tool(
            "computer_click",
            "Click at coordinates",
            computer_click_schema(),
            MockToolResult::Success(mock_click_result()),
        )
        .with_llm_responses(vec![
            MockLlmResponse::ToolUse {
                name: "computer_screenshot".into(),
                input: json!({}),
            },
            MockLlmResponse::ToolUse {
                name: "computer_click".into(),
                input: json!({"x": 100, "y": 200}),
            },
            MockLlmResponse::Text("Clicked the target element.".into()),
        ])
        .with_max_turns(10)
        .build()
        .await;

    let _messages = collect_messages(&mut scenario.agent_runner, "Click something").await;

    // Verify via scenario's mock references
    assert_eq!(scenario.mock_llm.call_count(), 3);
    assert_eq!(scenario.mock_tools.call_count(), 2);
}

// =============================================================================
// Test: Tool result content is properly structured for LLM feedback
// =============================================================================

#[tokio::test]
async fn test_tool_result_fed_back_to_llm() {
    let mock_llm = Arc::new(MockLlmClient::new());
    mock_llm
        .queue_responses(vec![
            MockLlmResponse::ToolUse {
                name: "computer_screenshot".into(),
                input: json!({}),
            },
            MockLlmResponse::Text("I see the page.".into()),
        ])
        .await;

    let mock_tools = Arc::new(MockToolExecutor::new());
    register_browser_tools(&mock_tools).await;

    let mut config = AgentConfig::default();
    config.max_turns = 20;
    config.permission_mode = PermissionMode::BypassPermissions;

    let mut runner = AgentRunner::new(config)
        .with_llm(mock_llm.clone() as Arc<dyn gateway_core::agent::r#loop::runner::LlmClient>)
        .with_tools(
            mock_tools.clone() as Arc<dyn gateway_core::agent::r#loop::runner::ToolExecutor>
        );

    let _ = collect_messages(&mut runner, "Take a screenshot").await;

    // Inspect the second LLM call - it should contain the tool result
    let call_log = mock_llm.call_log().await;
    assert_eq!(call_log.len(), 2);

    // Second call should include the previous messages with tool result
    let second_call_messages = &call_log[1].messages;
    let has_tool_result = second_call_messages.iter().any(|m| {
        if let AgentMessage::User(u) = m {
            u.tool_use_result.is_some()
        } else {
            false
        }
    });
    assert!(
        has_tool_result,
        "LLM's second call should include tool result in messages"
    );

    // The tool result should contain the screenshot data
    let tool_result_msg = second_call_messages
        .iter()
        .find_map(|m| {
            if let AgentMessage::User(u) = m {
                u.tool_use_result.as_ref()
            } else {
                None
            }
        })
        .expect("Should find tool result in messages");

    assert_eq!(tool_result_msg["success"], true);
    assert!(
        tool_result_msg.get("image_data").is_some(),
        "Should have image_data"
    );
}

// =============================================================================
// Test: Complete Gmail workflow simulation (5+ turns)
// =============================================================================

#[tokio::test]
async fn test_complete_gmail_compose_workflow() {
    let (mut runner, mock_llm, mock_tools) = browser_scenario(vec![
        // Turn 1: Navigate to Gmail
        MockLlmResponse::ToolUse {
            name: "browser_navigate".into(),
            input: json!({"url": "https://mail.google.com"}),
        },
        // Turn 2: Take screenshot to see the page
        MockLlmResponse::ToolUse {
            name: "computer_screenshot".into(),
            input: json!({"include_accessibility": true}),
        },
        // Turn 3: Click Compose button
        MockLlmResponse::ToolUse {
            name: "computer_click".into(),
            input: json!({"x": 100, "y": 120}),
        },
        // Turn 4: Take another screenshot to verify compose opened
        MockLlmResponse::ToolUse {
            name: "computer_screenshot".into(),
            input: json!({}),
        },
        // Turn 5: Type the recipient email
        MockLlmResponse::ToolUse {
            name: "computer_type".into(),
            input: json!({"text": "test@example.com"}),
        },
        // Turn 6: Press Tab to move to subject
        MockLlmResponse::ToolUse {
            name: "computer_key".into(),
            input: json!({"key": "Tab"}),
        },
        // Turn 7: Type subject
        MockLlmResponse::ToolUse {
            name: "computer_type".into(),
            input: json!({"text": "Test Email"}),
        },
        // Turn 8: Press Tab to move to body
        MockLlmResponse::ToolUse {
            name: "computer_key".into(),
            input: json!({"key": "Tab"}),
        },
        // Turn 9: Type body
        MockLlmResponse::ToolUse {
            name: "computer_type".into(),
            input: json!({"text": "Hello, this is a test email from the agent."}),
        },
        // Turn 10: Confirm completion
        MockLlmResponse::Text(
            "I've composed an email to test@example.com with subject 'Test Email' and body text. \
             The email is ready to send."
                .into(),
        ),
    ])
    .await;

    let messages = collect_messages(
        &mut runner,
        "Compose an email to test@example.com with subject 'Test Email'",
    )
    .await;

    assert!(
        has_result_message(&messages),
        "Should complete successfully"
    );

    let tool_calls = extract_tool_calls(&messages);
    assert_eq!(tool_calls.len(), 9, "Should have 9 tool calls");
    assert_eq!(tool_calls[0], "browser_navigate");
    assert_eq!(tool_calls[1], "computer_screenshot");
    assert_eq!(tool_calls[2], "computer_click");

    let tool_results = extract_tool_results(&messages);
    assert_eq!(
        tool_results.len(),
        9,
        "All 9 tool results should be present"
    );
    for (i, result) in tool_results.iter().enumerate() {
        assert_eq!(result["success"], true, "Tool result {} should succeed", i);
    }

    assert_eq!(
        mock_llm.call_count(),
        10,
        "LLM called 10 times (9 tool_use + 1 final)"
    );
    assert_eq!(mock_tools.call_count(), 9);

    let result = get_result(&messages).unwrap();
    assert!(!result.is_error);
    assert_eq!(result.num_turns, 10);
}

// =============================================================================
// Test: Interrupt during browser task
// =============================================================================

#[tokio::test]
async fn test_interrupt_during_browser_task() {
    let mock_llm = Arc::new(MockLlmClient::new().with_latency_ms(50));
    mock_llm
        .set_default_response(MockLlmResponse::ToolUse {
            name: "computer_screenshot".into(),
            input: json!({}),
        })
        .await;

    let mock_tools = Arc::new(MockToolExecutor::new());
    register_browser_tools(&mock_tools).await;
    // Add tool latency
    mock_tools.set_latency("computer_screenshot", 50).await;

    let mut config = AgentConfig::default();
    config.max_turns = 100;
    config.permission_mode = PermissionMode::BypassPermissions;

    let mut runner = AgentRunner::new(config)
        .with_llm(mock_llm.clone() as Arc<dyn gateway_core::agent::r#loop::runner::LlmClient>)
        .with_tools(
            mock_tools.clone() as Arc<dyn gateway_core::agent::r#loop::runner::ToolExecutor>
        );

    // Clone the state Arc before query borrows runner mutably.
    // AgentState::interrupt() takes &self (uses AtomicBool), so no mutable borrow needed.
    let state_handle = runner.state().clone();

    let stream = runner.query("Keep taking screenshots forever").await;
    futures::pin_mut!(stream);

    let mut messages = Vec::new();
    let mut count = 0;

    while let Some(result) = stream.next().await {
        match result {
            Ok(msg) => {
                messages.push(msg);
                count += 1;
                // Interrupt after processing a few messages
                if count >= 6 {
                    state_handle.interrupt();
                }
            }
            Err(_) => break,
        }
    }

    // Should have stopped before max_turns (100)
    assert!(
        mock_llm.call_count() < 100,
        "Should stop before max_turns due to interrupt"
    );

    // Should have some messages
    assert!(!messages.is_empty(), "Should have collected some messages");
}

// =============================================================================
// Test: No LLM configured returns placeholder
// =============================================================================

#[tokio::test]
async fn test_no_llm_configured() {
    let mock_tools = Arc::new(MockToolExecutor::new());
    register_browser_tools(&mock_tools).await;

    let mut config = AgentConfig::default();
    config.permission_mode = PermissionMode::BypassPermissions;

    // Runner without LLM
    let mut runner = AgentRunner::new(config).with_tools(
        mock_tools.clone() as Arc<dyn gateway_core::agent::r#loop::runner::ToolExecutor>
    );

    let messages = collect_messages(&mut runner, "Take a screenshot").await;

    assert!(has_result_message(&messages));

    // No tools should be called (LLM not configured to request them)
    assert_eq!(
        mock_tools.call_count(),
        0,
        "No tools should be called without LLM"
    );

    let result = get_result(&messages).unwrap();
    assert_eq!(result.result.as_deref(), Some("LLM not configured"));
}
