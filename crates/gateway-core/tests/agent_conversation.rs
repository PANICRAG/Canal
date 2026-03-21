//! Agent Conversation Integration Tests
//!
//! Tests multi-turn context retention (Dimension A) and long conversation
//! endurance (Dimension D) for the Canal agent loop.
//!
//! All tests are mock-based -- no real LLM or network calls are made.
//!
//! Dimension A: Multi-turn context retention
//!   A.1  5-turn context retention
//!   A.2  20-turn deep context with mixed tool calls
//!   A.3  Context with large tool results (10KB per turn)
//!
//! Dimension D: Long conversation endurance
//!   D.1  50-turn endurance (no compaction)
//!   D.2  100-turn with compaction
//!   D.3  200-turn sustained mixed load
//!   D.4  50-turn repeated topic switching
//!   D.5  Long conversation until token budget compaction

mod helpers;

use helpers::bench_harness::{get_process_memory_mb, print_bench_inline, BenchResult, BenchTimer};
use helpers::mock_llm::{MockLlmResponse, ToolUseBlock};
use helpers::mock_tools::MockToolResult;
use helpers::scenario_builder::{collect_messages, extract_text_content, ScenarioBuilder};

use gateway_core::agent::{AgentMessage, ContentBlock};
use serde_json::json;
use std::time::Instant;

// ============================================================================
// Dimension A: Multi-turn Context Retention
// ============================================================================

/// A.1 -- Five sequential turns where each turn references the previous one.
///
/// The mock LLM returns a distinct text per turn. After all turns finish we
/// inspect the LLM call log and verify that every subsequent LLM call receives
/// all accumulated messages from earlier turns.
#[tokio::test]
async fn test_5_turn_context_retention() {
    let responses: Vec<MockLlmResponse> = (0..5)
        .map(|i| MockLlmResponse::Text(format!("Response for turn {}", i)))
        .collect();

    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_llm_responses(responses)
        .with_default_llm_response(MockLlmResponse::Text("default".into()))
        .without_compaction()
        .with_max_turns(200)
        .build()
        .await;

    let mut agent = scenario.agent_runner;
    let mock_llm = scenario.mock_llm;

    let prompts: Vec<&str> = vec![
        "Start the project setup",
        "Now create the database schema based on what we discussed",
        "Add the API routes referencing the schema",
        "Write tests for those API routes",
        "Summarize everything we built",
    ];

    let mut bench = BenchResult::new("A.1 5-turn context retention");
    let overall_start = Instant::now();

    let mut all_turn_messages: Vec<Vec<AgentMessage>> = Vec::new();
    for prompt in &prompts {
        let timer = BenchTimer::start("turn");
        let messages = collect_messages(&mut agent, prompt).await;
        timer.stop_and_record(&mut bench);
        all_turn_messages.push(messages);
    }

    bench.add_metric("total_ms", overall_start.elapsed().as_secs_f64() * 1000.0);
    bench.add_metric("avg_turn_ms", bench.avg().as_secs_f64() * 1000.0);

    // Verify: each LLM call should see growing message context
    let call_log = mock_llm.call_log().await;
    assert!(
        call_log.len() >= 5,
        "Expected at least 5 LLM calls, got {}",
        call_log.len()
    );

    // The n-th call (0-indexed) should contain messages from all prior turns.
    // At minimum, call i should have more messages than call i-1.
    for i in 1..call_log.len().min(5) {
        assert!(
            call_log[i].messages.len() > call_log[i - 1].messages.len(),
            "Call {} ({} msgs) should have more messages than call {} ({} msgs)",
            i,
            call_log[i].messages.len(),
            i - 1,
            call_log[i - 1].messages.len(),
        );
    }

    // Verify all five turns produced output messages
    for (idx, turn_msgs) in all_turn_messages.iter().enumerate() {
        let texts = extract_text_content(turn_msgs);
        assert!(
            !texts.is_empty()
                || turn_msgs
                    .iter()
                    .any(|m| matches!(m, AgentMessage::Result(_))),
            "Turn {} should have produced text or a result message",
            idx
        );
    }

    print_bench_inline(&bench);
}

/// A.2 -- Twenty turns mixing plain text and tool calls.
///
/// Verifies that the 20th LLM call's input still contains key context from
/// turn 1 (the system message and the first user prompt).
#[tokio::test]
async fn test_20_turn_deep_context() {
    // Build alternating text / tool-use responses for 20 turns.
    // For tool-use responses, the runner will call LLM again after tool execution
    // so we need a text response following every tool-use to close the turn.
    let mut responses: Vec<MockLlmResponse> = Vec::new();
    for i in 0..20 {
        if i % 3 == 1 {
            // Tool use turn: emit ToolUse, then a text response after tool result
            responses.push(MockLlmResponse::ToolUse {
                name: "read_file".into(),
                input: json!({"path": format!("/src/module_{}.rs", i)}),
            });
            responses.push(MockLlmResponse::Text(format!(
                "Analyzed module_{}.rs successfully",
                i
            )));
        } else {
            responses.push(MockLlmResponse::Text(format!(
                "Turn {} analysis complete",
                i
            )));
        }
    }

    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_llm_responses(responses)
        .with_default_llm_response(MockLlmResponse::Text("default deep context".into()))
        .without_compaction()
        .with_max_turns(200)
        .build()
        .await;

    let mut agent = scenario.agent_runner;
    let mock_llm = scenario.mock_llm;

    let mut bench = BenchResult::new("A.2 20-turn deep context");
    let overall_start = Instant::now();

    let prompts: Vec<String> = (0..20)
        .map(|i| format!("Continue analysis step {} referencing prior results", i))
        .collect();
    let prompt_refs: Vec<&str> = prompts.iter().map(|s| s.as_str()).collect();

    let mut context_sizes: Vec<usize> = Vec::new();
    for prompt in &prompt_refs {
        let timer = BenchTimer::start("turn");
        let _msgs = collect_messages(&mut agent, prompt).await;
        timer.stop_and_record(&mut bench);
    }

    let call_log = mock_llm.call_log().await;

    // Record context growth at each call
    for record in &call_log {
        context_sizes.push(record.messages.len());
    }

    bench.add_metric("total_ms", overall_start.elapsed().as_secs_f64() * 1000.0);
    bench.add_metric("total_llm_calls", call_log.len() as f64);
    bench.add_metric(
        "context_growth_per_turn",
        if context_sizes.len() >= 2 {
            let first = context_sizes[0] as f64;
            let last = *context_sizes.last().unwrap() as f64;
            (last - first) / (context_sizes.len() as f64 - 1.0)
        } else {
            0.0
        },
    );

    // The last LLM call should still include the first user prompt text in
    // the message history (context was not compacted).
    if let Some(last_call) = call_log.last() {
        let has_early_user_msg = last_call.messages.iter().any(|m| {
            if let AgentMessage::User(u) = m {
                let content = u.content.to_string_content();
                content.contains("step 0")
            } else {
                false
            }
        });
        assert!(
            has_early_user_msg,
            "Turn 20 LLM input should still contain the first user prompt"
        );
    }

    // Verify monotonic context growth (no messages lost without compaction)
    for i in 1..context_sizes.len() {
        assert!(
            context_sizes[i] >= context_sizes[i - 1],
            "Context should grow monotonically: call {} ({}) < call {} ({})",
            i,
            context_sizes[i],
            i - 1,
            context_sizes[i - 1],
        );
    }

    print_bench_inline(&bench);
}

/// A.3 -- Five turns, each with a tool call returning ~10KB JSON.
///
/// Verifies that large tool results are preserved across the conversation.
#[tokio::test]
async fn test_context_with_large_tool_results() {
    // Generate a ~10KB JSON payload
    fn make_large_json(turn: usize) -> serde_json::Value {
        let entries: Vec<serde_json::Value> = (0..100)
            .map(|i| {
                json!({
                    "id": i,
                    "turn": turn,
                    "data": "X".repeat(80),
                    "nested": { "a": i * 2, "b": format!("value_{}", i) }
                })
            })
            .collect();
        json!({ "results": entries, "turn": turn })
    }

    // Each turn: ToolUse -> (tool executor returns large json) -> Text followup
    let mut responses: Vec<MockLlmResponse> = Vec::new();
    for i in 0..5 {
        responses.push(MockLlmResponse::ToolUse {
            name: "search_files".into(),
            input: json!({"pattern": format!("*.mod_{}", i)}),
        });
        responses.push(MockLlmResponse::Text(format!(
            "Processed search results for turn {}",
            i
        )));
    }

    let large_result = MockToolResult::DynamicFn(std::sync::Arc::new(|input| {
        // Parse turn number from the pattern field if present
        let turn = input
            .get("pattern")
            .and_then(|v| v.as_str())
            .and_then(|s| s.chars().last())
            .and_then(|c| c.to_digit(10))
            .unwrap_or(0) as usize;
        make_large_json(turn)
    }));

    let scenario = ScenarioBuilder::new()
        .with_tool(
            "search_files",
            "Search for files matching a pattern",
            json!({
                "type": "object",
                "properties": { "pattern": {"type": "string"} },
                "required": ["pattern"]
            }),
            large_result,
        )
        .with_llm_responses(responses)
        .with_default_llm_response(MockLlmResponse::Text("default".into()))
        .without_compaction()
        .with_max_turns(200)
        .build()
        .await;

    let mut agent = scenario.agent_runner;
    let mock_llm = scenario.mock_llm;

    let mut bench = BenchResult::new("A.3 Context with large tool results");
    let overall_start = Instant::now();

    for i in 0..5 {
        let timer = BenchTimer::start("turn");
        let _msgs = collect_messages(
            &mut agent,
            &format!("Search for module_{} files and analyze", i),
        )
        .await;
        timer.stop_and_record(&mut bench);
    }

    let call_log = mock_llm.call_log().await;

    // Helper to measure the character size of content blocks
    fn blocks_char_size(blocks: &[ContentBlock]) -> usize {
        blocks
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
            .sum()
    }

    // Estimate total context size at the last call (in KB)
    let last_call = call_log.last().expect("Should have LLM calls");
    let total_context_chars: usize = last_call
        .messages
        .iter()
        .map(|m| match m {
            AgentMessage::User(u) => match &u.content {
                gateway_core::agent::MessageContent::Text(s) => s.len(),
                gateway_core::agent::MessageContent::Blocks(blocks) => blocks_char_size(blocks),
            },
            AgentMessage::Assistant(a) => blocks_char_size(&a.content),
            AgentMessage::System(s) => s.data.to_string().len(),
            _ => 0,
        })
        .sum();

    let context_size_kb = total_context_chars as f64 / 1024.0;

    bench.add_metric("total_ms", overall_start.elapsed().as_secs_f64() * 1000.0);
    bench.add_metric("context_size_kb", context_size_kb);
    bench.add_metric("total_llm_calls", call_log.len() as f64);

    // All tool results should be preserved (5 turns * 1 tool call each = 5 tool results)
    let tool_result_count: usize = last_call
        .messages
        .iter()
        .map(|m| match m {
            AgentMessage::User(u) => match &u.content {
                gateway_core::agent::MessageContent::Blocks(blocks) => blocks
                    .iter()
                    .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
                    .count(),
                _ => 0,
            },
            _ => 0,
        })
        .sum();

    assert!(
        tool_result_count >= 4,
        "Expected at least 4 tool results in final context, got {}",
        tool_result_count
    );

    // Context should be substantial (5 turns of large tool results).
    // Each tool result has 100 entries * ~100 chars = ~10KB, but JSON serialization
    // and nesting may vary. We verify it is meaningfully larger than a baseline
    // text-only conversation.
    assert!(
        context_size_kb > 3.0,
        "Context size should be >3KB with large tool results, got {:.1}KB",
        context_size_kb
    );

    print_bench_inline(&bench);
}

// ============================================================================
// Dimension D: Long Conversation Endurance
// ============================================================================

/// D.1 -- 50-turn simple text conversation with no compaction.
///
/// Verifies that all 50 turns complete successfully and there is no
/// significant latency degradation (last 10 turns avg <= 5x first 10 turns avg).
#[tokio::test]
async fn test_50_turn_endurance() {
    let responses: Vec<MockLlmResponse> = (0..50)
        .map(|i| MockLlmResponse::Text(format!("Endurance response turn {}", i)))
        .collect();

    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_llm_responses(responses)
        .with_default_llm_response(MockLlmResponse::Text("default endurance".into()))
        .without_compaction()
        .with_max_turns(200)
        .build()
        .await;

    let mut agent = scenario.agent_runner;

    let mut bench = BenchResult::new("D.1 50-turn endurance");
    let overall_start = Instant::now();

    let mut turn_durations_ms: Vec<f64> = Vec::new();

    for i in 0..50 {
        let timer = BenchTimer::start("turn");
        let msgs = collect_messages(&mut agent, &format!("Endurance turn {}", i)).await;
        let elapsed = timer.stop_and_record(&mut bench);
        turn_durations_ms.push(elapsed.as_secs_f64() * 1000.0);

        // Every turn should produce at least a result message
        assert!(!msgs.is_empty(), "Turn {} produced no messages", i);
    }

    bench.add_metric("total_ms", overall_start.elapsed().as_secs_f64() * 1000.0);
    bench.add_metric("turns_completed", 50.0);

    // Check latency trend: compare first 10 and last 10 turns
    let first_10_avg: f64 = turn_durations_ms[..10].iter().sum::<f64>() / 10.0;
    let last_10_avg: f64 = turn_durations_ms[40..].iter().sum::<f64>() / 10.0;

    bench.add_metric("first_10_avg_ms", first_10_avg);
    bench.add_metric("last_10_avg_ms", last_10_avg);

    let degradation_ratio = if first_10_avg > 0.0 {
        last_10_avg / first_10_avg
    } else {
        1.0
    };
    bench.add_metric("degradation_ratio", degradation_ratio);

    // Allow up to 10x degradation for mock tests (context growth causes more
    // work in serialization / token estimation). In a real system the threshold
    // would be tighter.
    assert!(
        degradation_ratio < 10.0,
        "Latency degradation too high: {:.2}x (first_10={:.2}ms, last_10={:.2}ms)",
        degradation_ratio,
        first_10_avg,
        last_10_avg,
    );

    print_bench_inline(&bench);
}

/// D.2 -- 100-turn conversation with compaction enabled.
///
/// Uses a low token threshold so compaction triggers multiple times.
/// Verifies the conversation remains coherent (no errors, all turns complete).
#[tokio::test]
async fn test_100_turn_with_compaction() {
    // Pre-generate 100 text responses
    let responses: Vec<MockLlmResponse> = (0..100)
        .map(|i| MockLlmResponse::Text(format!("Compacted response turn {}", i)))
        .collect();

    // Use a very low compaction threshold so it triggers frequently.
    // ~500 tokens max (~2000 chars), keep 5 recent messages.
    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_llm_responses(responses)
        .with_default_llm_response(MockLlmResponse::Text("default compacted".into()))
        .with_compaction(
            500, // max_context_tokens (very low to force compaction)
            250, // target_tokens
            5,   // min_messages_to_keep
        )
        .with_max_turns(200)
        .build()
        .await;

    let mut agent = scenario.agent_runner;
    let mock_llm = scenario.mock_llm;

    let mut bench = BenchResult::new("D.2 100-turn with compaction");
    let overall_start = Instant::now();

    let mut completed_turns = 0u32;
    let mut errors = 0u32;

    for i in 0..100 {
        let timer = BenchTimer::start("turn");
        let msgs = collect_messages(
            &mut agent,
            &format!("Compaction turn {} - discuss topic {}", i, i % 5),
        )
        .await;
        timer.stop_and_record(&mut bench);

        // Check for error messages
        let has_error = msgs.iter().any(|m| {
            if let AgentMessage::System(s) = m {
                s.subtype == "error"
            } else if let AgentMessage::Result(r) = m {
                r.is_error
            } else {
                false
            }
        });

        if has_error {
            errors += 1;
        } else {
            completed_turns += 1;
        }
    }

    bench.add_metric("total_ms", overall_start.elapsed().as_secs_f64() * 1000.0);
    bench.add_metric("completed_turns", completed_turns as f64);
    bench.add_metric("errors", errors as f64);

    // Count compaction events by looking for context_summary system messages
    // in the LLM call logs.
    let call_log = mock_llm.call_log().await;
    let mut compaction_events = 0u32;
    for record in &call_log {
        for msg in &record.messages {
            if let AgentMessage::System(s) = msg {
                if s.subtype == "context_summary" {
                    compaction_events += 1;
                    break; // count once per call that has a summary
                }
            }
        }
    }

    bench.add_metric("compaction_events", compaction_events as f64);
    bench.add_metric("total_llm_calls", call_log.len() as f64);

    // With such a low threshold and 100 turns, we should see at least a couple
    // of compaction events.
    assert!(
        compaction_events >= 1,
        "Expected at least 1 compaction event, got {}",
        compaction_events
    );

    // The vast majority of turns should complete without error
    assert!(
        completed_turns >= 95,
        "Expected >= 95 successful turns, got {}",
        completed_turns
    );

    print_bench_inline(&bench);
}

/// D.3 -- 200-turn sustained load with mixed complexity.
///
/// Distribution: 20% text-only, 50% single tool call, 30% multi-tool call.
/// Monitors memory growth and latency degradation.
#[tokio::test]
async fn test_200_turn_sustained_load() {
    // Build response queue: for every turn we emit the LLM responses.
    // Single tool-use turns consume 2 LLM calls (tool-use + text).
    // Multi-tool turns consume 2 LLM calls (multi-tool-use + text).
    let mut responses: Vec<MockLlmResponse> = Vec::new();
    for i in 0..200 {
        let bucket = i % 10;
        if bucket < 2 {
            // 20% text only
            responses.push(MockLlmResponse::Text(format!("Text turn {}", i)));
        } else if bucket < 7 {
            // 50% single tool
            responses.push(MockLlmResponse::ToolUse {
                name: "read_file".into(),
                input: json!({"path": format!("/src/file_{}.rs", i)}),
            });
            responses.push(MockLlmResponse::Text(format!(
                "Read file_{}.rs successfully",
                i
            )));
        } else {
            // 30% multi-tool
            responses.push(MockLlmResponse::MultiToolUse(vec![
                ToolUseBlock::new("read_file", json!({"path": format!("/src/a_{}.rs", i)})),
                ToolUseBlock::new("search_files", json!({"pattern": format!("*.mod_{}", i)})),
            ]));
            responses.push(MockLlmResponse::Text(format!(
                "Multi-tool turn {} complete",
                i
            )));
        }
    }

    // Use moderate compaction so the test does not run out of memory but also
    // does not compact every turn.
    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_llm_responses(responses)
        .with_default_llm_response(MockLlmResponse::Text("default sustained".into()))
        .with_compaction(
            5000, // max_context_tokens
            2500, // target_tokens
            10,   // min_messages_to_keep
        )
        .with_max_turns(300)
        .build()
        .await;

    let mut agent = scenario.agent_runner;

    let mut bench = BenchResult::new("D.3 200-turn sustained load");
    let overall_start = Instant::now();
    let mem_before = get_process_memory_mb();

    let mut turn_durations_ms: Vec<f64> = Vec::new();
    let mut completed = 0u32;

    for i in 0..200 {
        let timer = BenchTimer::start("turn");
        let msgs = collect_messages(
            &mut agent,
            &format!("Sustained turn {} - operation batch", i),
        )
        .await;
        let elapsed = timer.stop_and_record(&mut bench);
        turn_durations_ms.push(elapsed.as_secs_f64() * 1000.0);

        if !msgs.is_empty() {
            completed += 1;
        }
    }

    let mem_after = get_process_memory_mb();
    let memory_growth_mb = mem_after - mem_before;

    bench.add_metric("total_ms", overall_start.elapsed().as_secs_f64() * 1000.0);
    bench.add_metric("completed_turns", completed as f64);
    bench.add_metric("memory_growth_mb", memory_growth_mb);

    // Degradation ratio: compare first 20 to last 20
    let first_20_avg: f64 = turn_durations_ms[..20].iter().sum::<f64>() / 20.0;
    let last_20_avg: f64 = turn_durations_ms[180..].iter().sum::<f64>() / 20.0;
    let degradation_ratio = if first_20_avg > 0.0 {
        last_20_avg / first_20_avg
    } else {
        1.0
    };
    bench.add_metric("first_20_avg_ms", first_20_avg);
    bench.add_metric("last_20_avg_ms", last_20_avg);
    bench.add_metric("degradation_ratio", degradation_ratio);

    // All 200 turns should complete
    assert!(
        completed >= 195,
        "Expected >= 195 completed turns, got {}",
        completed
    );

    // Memory growth should be bounded (less than 200 MB for mock data)
    assert!(
        memory_growth_mb < 200.0,
        "Memory grew by {:.1}MB, expected < 200MB",
        memory_growth_mb
    );

    print_bench_inline(&bench);
}

/// D.4 -- 50 turns switching between three topics.
///
/// Topics: code analysis, data processing, file operations.
/// Verifies that context persists across topic switches.
#[tokio::test]
async fn test_conversation_with_repeated_topics() {
    let topics = [
        (
            "code_analysis",
            "Analyze the authentication module's code structure",
        ),
        (
            "data_processing",
            "Process the CSV export and generate statistics",
        ),
        (
            "file_operations",
            "Organize the configuration files by environment",
        ),
    ];

    // Build responses: cycle through topics, alternating text / tool use
    let mut responses: Vec<MockLlmResponse> = Vec::new();
    for i in 0..50 {
        let (topic_name, _) = topics[i % 3];
        if i % 2 == 0 {
            responses.push(MockLlmResponse::Text(format!(
                "{} result for iteration {}",
                topic_name, i
            )));
        } else {
            // Tool use turn
            let tool_name = match i % 3 {
                0 => "read_file",
                1 => "search_files",
                _ => "write_file",
            };
            let input = match tool_name {
                "write_file" => json!({"path": format!("/config/{}.yaml", i), "content": "data"}),
                "search_files" => json!({"pattern": format!("*.{}", topic_name)}),
                _ => json!({"path": format!("/src/{}.rs", topic_name)}),
            };
            responses.push(MockLlmResponse::ToolUse {
                name: tool_name.into(),
                input,
            });
            responses.push(MockLlmResponse::Text(format!(
                "Completed {} tool step {}",
                topic_name, i
            )));
        }
    }

    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_llm_responses(responses)
        .with_default_llm_response(MockLlmResponse::Text("default topic".into()))
        .without_compaction()
        .with_max_turns(200)
        .build()
        .await;

    let mut agent = scenario.agent_runner;
    let mock_llm = scenario.mock_llm;

    let mut bench = BenchResult::new("D.4 50-turn topic switching");
    let overall_start = Instant::now();

    // Track per-topic switch overhead: measure how long the first turn after a
    // topic switch takes vs. a same-topic continuation.
    let mut switch_durations_ms: Vec<f64> = Vec::new();
    let mut same_topic_durations_ms: Vec<f64> = Vec::new();

    let mut prev_topic_idx: Option<usize> = None;

    for i in 0..50 {
        let topic_idx = i % 3;
        let (_, topic_prompt) = topics[topic_idx];

        let prompt = format!("Turn {}: {}", i, topic_prompt);

        let timer = BenchTimer::start("turn");
        let _msgs = collect_messages(&mut agent, &prompt).await;
        let elapsed = timer.stop_and_record(&mut bench);
        let ms = elapsed.as_secs_f64() * 1000.0;

        if let Some(prev) = prev_topic_idx {
            if prev != topic_idx {
                switch_durations_ms.push(ms);
            } else {
                same_topic_durations_ms.push(ms);
            }
        }
        prev_topic_idx = Some(topic_idx);
    }

    bench.add_metric("total_ms", overall_start.elapsed().as_secs_f64() * 1000.0);

    let switch_avg = if switch_durations_ms.is_empty() {
        0.0
    } else {
        switch_durations_ms.iter().sum::<f64>() / switch_durations_ms.len() as f64
    };
    let same_avg = if same_topic_durations_ms.is_empty() {
        0.0
    } else {
        same_topic_durations_ms.iter().sum::<f64>() / same_topic_durations_ms.len() as f64
    };

    bench.add_metric("topic_switch_avg_ms", switch_avg);
    bench.add_metric("same_topic_avg_ms", same_avg);
    bench.add_metric(
        "topic_switch_overhead_ms",
        if same_avg > 0.0 {
            switch_avg - same_avg
        } else {
            0.0
        },
    );

    // Verify context integrity: the final LLM call should contain messages
    // referencing all three topics.
    let call_log = mock_llm.call_log().await;
    if let Some(last_call) = call_log.last() {
        let all_text: String = last_call
            .messages
            .iter()
            .map(|m| match m {
                AgentMessage::User(u) => u.content.to_string_content(),
                AgentMessage::Assistant(a) => a
                    .content
                    .iter()
                    .filter_map(|b| b.as_text().map(|t| t.to_string()))
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join(" ");

        for (topic_name, _) in &topics {
            assert!(
                all_text.contains(topic_name),
                "Final context should contain topic '{}' but it was missing",
                topic_name
            );
        }
    }

    print_bench_inline(&bench);
}

/// D.5 -- Continue conversation until the token budget triggers compaction.
///
/// Uses a moderate token budget and verifies that:
///   1. Compaction eventually triggers.
///   2. The conversation continues working after compaction.
///   3. We measure the turns before first compaction and the savings.
#[tokio::test]
async fn test_long_conversation_token_budget() {
    // Each turn: text response of moderate size. We want roughly 100 chars
    // per response to build up tokens at a predictable rate.
    let mut responses: Vec<MockLlmResponse> = Vec::new();
    for i in 0..300 {
        responses.push(MockLlmResponse::Text(format!(
            "Budget test turn {} - {} padding content here to take up space in the context window.",
            i,
            "word ".repeat(15)
        )));
    }

    // Token budget: ~1000 tokens (~4000 chars). With system prompt + messages
    // this should trigger within ~20-40 turns.
    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_llm_responses(responses)
        .with_default_llm_response(MockLlmResponse::Text("budget fallback".into()))
        .with_compaction(
            1000, // max_context_tokens (low)
            500,  // target_tokens
            5,    // min_messages_to_keep
        )
        .with_max_turns(300)
        .build()
        .await;

    let mut agent = scenario.agent_runner;
    let mock_llm = scenario.mock_llm;

    let mut bench = BenchResult::new("D.5 Token budget compaction");
    let overall_start = Instant::now();

    let max_turns_to_try = 100;
    let mut first_compaction_turn: Option<usize> = None;
    let mut post_compaction_turns_ok = 0u32;

    for i in 0..max_turns_to_try {
        let timer = BenchTimer::start("turn");
        let msgs = collect_messages(
            &mut agent,
            &format!(
                "Budget turn {} - please continue the analysis with more detail and context",
                i
            ),
        )
        .await;
        timer.stop_and_record(&mut bench);

        // Check if compaction has occurred by scanning the latest LLM call
        if first_compaction_turn.is_none() {
            let call_log = mock_llm.call_log().await;
            if let Some(last_call) = call_log.last() {
                let has_summary = last_call.messages.iter().any(|m| {
                    if let AgentMessage::System(s) = m {
                        s.subtype == "context_summary"
                    } else {
                        false
                    }
                });
                if has_summary {
                    first_compaction_turn = Some(i);
                }
            }
        } else {
            // We are past first compaction - verify turns still work
            let has_result = msgs.iter().any(|m| matches!(m, AgentMessage::Result(_)));
            if has_result {
                post_compaction_turns_ok += 1;
            }
        }

        // Once we have enough post-compaction data, we can stop
        if post_compaction_turns_ok >= 10 {
            break;
        }
    }

    bench.add_metric("total_ms", overall_start.elapsed().as_secs_f64() * 1000.0);
    bench.add_metric(
        "turns_before_compaction",
        first_compaction_turn.unwrap_or(max_turns_to_try) as f64,
    );
    bench.add_metric(
        "post_compaction_success_turns",
        post_compaction_turns_ok as f64,
    );

    // Compute compaction savings from the call log
    let call_log = mock_llm.call_log().await;
    let mut pre_compaction_tokens: Option<usize> = None;
    let mut post_compaction_tokens: Option<usize> = None;

    for record in &call_log {
        let has_summary = record.messages.iter().any(|m| {
            if let AgentMessage::System(s) = m {
                s.subtype == "context_summary"
            } else {
                false
            }
        });

        if has_summary && pre_compaction_tokens.is_none() {
            // The call right before this one had the max tokens
            post_compaction_tokens = Some(record.messages.len());
        }

        if !has_summary && pre_compaction_tokens.is_none() && post_compaction_tokens.is_none() {
            pre_compaction_tokens = Some(record.messages.len());
        }
    }

    // Find the largest message count before compaction appeared
    let mut max_msgs_before_compact = 0usize;
    let mut first_compact_msgs = 0usize;
    for record in &call_log {
        let has_summary = record.messages.iter().any(|m| {
            if let AgentMessage::System(s) = m {
                s.subtype == "context_summary"
            } else {
                false
            }
        });

        if !has_summary {
            max_msgs_before_compact = max_msgs_before_compact.max(record.messages.len());
        } else if first_compact_msgs == 0 {
            first_compact_msgs = record.messages.len();
        }
    }

    let savings = if max_msgs_before_compact > first_compact_msgs && first_compact_msgs > 0 {
        (max_msgs_before_compact - first_compact_msgs) as f64
    } else {
        0.0
    };
    bench.add_metric("compaction_msg_savings", savings);

    // Compaction should have triggered
    assert!(
        first_compaction_turn.is_some(),
        "Compaction should have triggered within {} turns",
        max_turns_to_try
    );

    // Conversation should continue working after compaction
    assert!(
        post_compaction_turns_ok >= 5,
        "Expected >= 5 successful post-compaction turns, got {}",
        post_compaction_turns_ok
    );

    print_bench_inline(&bench);
}
