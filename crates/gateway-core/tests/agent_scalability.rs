//! Agent Scalability Integration Tests
//!
//! Tests across three capability dimensions:
//!   H - Concurrent session handling
//!   I - Session persistence (save/restore fidelity)
//!   J - Intent recognition accuracy and speed
//!
//! Each test exercises the agent loop with mock LLM and tool backends,
//! then reports benchmark metrics via print_bench_inline.

mod helpers;

use helpers::bench_harness::{print_bench_inline, BenchResult, BenchTimer};
use helpers::mock_llm::{MockLlmClient, MockLlmResponse};
use helpers::mock_tools::{MockToolExecutor, MockToolResult};
use helpers::scenario_builder::{
    collect_messages, extract_text_content, has_result_message, ScenarioBuilder,
};

use gateway_core::agent::intent::{IntentRecognizer, IntentType};
use gateway_core::agent::r#loop::config::{AgentConfig, CompactionConfig};
use gateway_core::agent::r#loop::runner::{AgentRunner, LlmClient, ToolExecutor};
use gateway_core::agent::session::{
    MemorySessionStorage, SessionMetadata, SessionSnapshot, SessionStorage,
};
use gateway_core::agent::types::{
    AgentMessage, AssistantMessage, ContentBlock, MessageContent, PermissionMode, SystemMessage,
    UserMessage,
};
use gateway_core::agent::AgentLoop;
use gateway_core::llm::{LlmConfig, LlmRouter};

use futures::StreamExt;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ============================================================================
// Dimension H: Concurrent Sessions
// ============================================================================

/// H.1: 5 concurrent sessions, each sending 5 prompts.
/// Verifies all 25 queries complete and no crosstalk occurs between sessions.
#[tokio::test]
async fn test_5_concurrent_sessions() {
    let mut bench = BenchResult::new("H.1: 5 concurrent sessions");
    let timer = BenchTimer::start("5_sessions_total");

    let mut handles = Vec::new();
    for session_idx in 0..5u32 {
        let handle = tokio::spawn(async move {
            let session_id = format!("session-h1-{}", session_idx);
            let mock_llm = Arc::new(MockLlmClient::new());
            mock_llm
                .set_default_response(MockLlmResponse::Text(format!(
                    "Response from session {}",
                    session_idx
                )))
                .await;

            let mock_tools = Arc::new(MockToolExecutor::new());
            mock_tools.register_filesystem_tools().await;

            // Set a distinct result for read_file so we can verify isolation
            mock_tools
                .set_result(
                    "read_file",
                    MockToolResult::Success(
                        json!({"content": format!("data_session_{}", session_idx)}),
                    ),
                )
                .await;

            let mut config = AgentConfig::default();
            config.max_turns = 50;
            config.permission_mode = PermissionMode::BypassPermissions;
            config.compaction = CompactionConfig::disabled();

            let mut agent = AgentRunner::with_session_id(config, &session_id)
                .with_llm(mock_llm.clone() as Arc<dyn LlmClient>)
                .with_tools(mock_tools.clone() as Arc<dyn ToolExecutor>);

            let session_start = Instant::now();
            let mut all_messages: Vec<Vec<AgentMessage>> = Vec::new();
            for prompt_idx in 0..5u32 {
                let prompt = format!("Session {} prompt {}", session_idx, prompt_idx);
                let stream = agent.query(&prompt).await;
                futures::pin_mut!(stream);
                let mut turn_messages = Vec::new();
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(msg) => turn_messages.push(msg),
                        Err(_) => break,
                    }
                }
                all_messages.push(turn_messages);
            }
            let session_ms = session_start.elapsed().as_secs_f64() * 1000.0;

            // Verify all 5 prompts produced messages (each turn should have at
            // least a result message).
            assert_eq!(
                all_messages.len(),
                5,
                "Session {} should have 5 turns",
                session_idx
            );
            for (i, msgs) in all_messages.iter().enumerate() {
                assert!(
                    has_result_message(msgs),
                    "Session {} turn {} missing result message",
                    session_idx,
                    i
                );
            }

            // Verify no crosstalk: text content should only reference this
            // session's index.
            for msgs in &all_messages {
                let texts = extract_text_content(msgs);
                for text in &texts {
                    assert!(
                        text.contains(&format!("session {}", session_idx))
                            || !text.starts_with("Response from session"),
                        "Session {} saw unexpected text: {}",
                        session_idx,
                        text
                    );
                }
            }

            (
                session_idx,
                session_ms,
                mock_llm.call_count(),
                mock_tools.call_count(),
            )
        });
        handles.push(handle);
    }

    let results: Vec<_> = futures::future::join_all(handles).await;
    let total_elapsed = timer.stop();
    bench.add_sample(total_elapsed);

    let mut per_session_ms_total = 0.0;
    for result in &results {
        let (idx, ms, llm_calls, _tool_calls) = result.as_ref().expect("task panicked");
        per_session_ms_total += ms;
        assert!(
            *llm_calls >= 5,
            "Session {} should have made >= 5 LLM calls but made {}",
            idx,
            llm_calls
        );
    }

    bench.add_metric("5_sessions_total_ms", total_elapsed.as_secs_f64() * 1000.0);
    bench.add_metric("per_session_avg_ms", per_session_ms_total / 5.0);
    print_bench_inline(&bench);
}

/// H.2: 20 concurrent sessions under stress.
/// Each session sends 3 text prompts plus 1 tool-calling prompt.
/// Guarded by a 60-second timeout to detect deadlocks.
#[tokio::test]
async fn test_20_concurrent_sessions_stress() {
    let mut bench = BenchResult::new("H.2: 20 concurrent sessions stress");
    let timer = BenchTimer::start("20_sessions_total");

    let result = tokio::time::timeout(Duration::from_secs(60), async {
        let mut handles = Vec::new();
        for session_idx in 0..20u32 {
            let handle = tokio::spawn(async move {
                let session_id = format!("session-h2-{}", session_idx);
                let mock_llm = Arc::new(MockLlmClient::new());

                // Queue: 3 text responses + 1 tool use + 1 final text (after tool result)
                mock_llm
                    .queue_responses(vec![
                        MockLlmResponse::Text(format!("Reply 0 s{}", session_idx)),
                        MockLlmResponse::Text(format!("Reply 1 s{}", session_idx)),
                        MockLlmResponse::Text(format!("Reply 2 s{}", session_idx)),
                        MockLlmResponse::ToolUse {
                            name: "read_file".to_string(),
                            input: json!({"path": format!("/tmp/s{}.txt", session_idx)}),
                        },
                        MockLlmResponse::Text(format!("Tool done s{}", session_idx)),
                    ])
                    .await;
                mock_llm
                    .set_default_response(MockLlmResponse::Text("default".to_string()))
                    .await;

                let mock_tools = Arc::new(MockToolExecutor::new());
                mock_tools.register_filesystem_tools().await;

                let mut config = AgentConfig::default();
                config.max_turns = 50;
                config.permission_mode = PermissionMode::BypassPermissions;
                config.compaction = CompactionConfig::disabled();

                let mut agent = AgentRunner::with_session_id(config, &session_id)
                    .with_llm(mock_llm.clone() as Arc<dyn LlmClient>)
                    .with_tools(mock_tools.clone() as Arc<dyn ToolExecutor>);

                let start = Instant::now();

                // 3 text prompts
                for i in 0..3u32 {
                    let prompt = format!("Prompt {} for session {}", i, session_idx);
                    let stream = agent.query(&prompt).await;
                    futures::pin_mut!(stream);
                    while let Some(r) = stream.next().await {
                        if r.is_err() {
                            break;
                        }
                    }
                }

                // 1 prompt that triggers tool use
                let stream = agent.query("Read the file").await;
                futures::pin_mut!(stream);
                while let Some(r) = stream.next().await {
                    if r.is_err() {
                        break;
                    }
                }

                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                (session_idx, elapsed_ms)
            });
            handles.push(handle);
        }

        let results: Vec<_> = futures::future::join_all(handles).await;

        let mut latencies: Vec<f64> = Vec::new();
        for r in &results {
            let (_idx, ms) = r.as_ref().expect("task panicked");
            latencies.push(*ms);
        }
        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
        latencies
    })
    .await;

    let total_elapsed = timer.stop();
    bench.add_sample(total_elapsed);

    let latencies = result.expect("20 session stress test timed out after 60s");
    assert_eq!(latencies.len(), 20, "All 20 sessions should complete");

    let p50_idx = (latencies.len() as f64 * 0.50).round() as usize;
    let p95_idx = (latencies.len() as f64 * 0.95).round() as usize;
    let p50 = latencies[p50_idx.min(latencies.len() - 1)];
    let p95 = latencies[p95_idx.min(latencies.len() - 1)];

    bench.add_metric("20_sessions_total_ms", total_elapsed.as_secs_f64() * 1000.0);
    bench.add_metric("session_latency_p50", p50);
    bench.add_metric("session_latency_p95", p95);
    print_bench_inline(&bench);
}

/// H.3: Session isolation - no crosstalk between two concurrent sessions.
/// Session A's read_file returns "content_a", Session B's returns "content_b".
/// Verify each session only sees its own data.
#[tokio::test]
async fn test_session_isolation_no_crosstalk() {
    let mut bench = BenchResult::new("H.3: session isolation no crosstalk");
    let timer = BenchTimer::start("isolation_verification");

    let handle_a = tokio::spawn(async {
        let mock_llm = Arc::new(MockLlmClient::new());
        mock_llm
            .queue_responses(vec![
                MockLlmResponse::ToolUse {
                    name: "read_file".to_string(),
                    input: json!({"path": "/tmp/a.txt"}),
                },
                MockLlmResponse::Text("Session A done".to_string()),
            ])
            .await;
        mock_llm
            .set_default_response(MockLlmResponse::Text("A fallback".to_string()))
            .await;

        let mock_tools = Arc::new(MockToolExecutor::new());
        mock_tools.register_filesystem_tools().await;
        mock_tools
            .set_result(
                "read_file",
                MockToolResult::Success(json!({"content": "content_a"})),
            )
            .await;

        let mut config = AgentConfig::default();
        config.max_turns = 20;
        config.permission_mode = PermissionMode::BypassPermissions;
        config.compaction = CompactionConfig::disabled();

        let mut agent = AgentRunner::with_session_id(config, "session-a")
            .with_llm(mock_llm.clone() as Arc<dyn LlmClient>)
            .with_tools(mock_tools.clone() as Arc<dyn ToolExecutor>);

        let msgs = collect_messages(&mut agent, "Read file A").await;
        let tool_log = mock_tools.call_log().await;
        (msgs, tool_log)
    });

    let handle_b = tokio::spawn(async {
        let mock_llm = Arc::new(MockLlmClient::new());
        mock_llm
            .queue_responses(vec![
                MockLlmResponse::ToolUse {
                    name: "read_file".to_string(),
                    input: json!({"path": "/tmp/b.txt"}),
                },
                MockLlmResponse::Text("Session B done".to_string()),
            ])
            .await;
        mock_llm
            .set_default_response(MockLlmResponse::Text("B fallback".to_string()))
            .await;

        let mock_tools = Arc::new(MockToolExecutor::new());
        mock_tools.register_filesystem_tools().await;
        mock_tools
            .set_result(
                "read_file",
                MockToolResult::Success(json!({"content": "content_b"})),
            )
            .await;

        let mut config = AgentConfig::default();
        config.max_turns = 20;
        config.permission_mode = PermissionMode::BypassPermissions;
        config.compaction = CompactionConfig::disabled();

        let mut agent = AgentRunner::with_session_id(config, "session-b")
            .with_llm(mock_llm.clone() as Arc<dyn LlmClient>)
            .with_tools(mock_tools.clone() as Arc<dyn ToolExecutor>);

        let msgs = collect_messages(&mut agent, "Read file B").await;
        let tool_log = mock_tools.call_log().await;
        (msgs, tool_log)
    });

    let (result_a, result_b) = tokio::join!(handle_a, handle_b);
    let (msgs_a, tool_log_a) = result_a.expect("session A panicked");
    let (msgs_b, tool_log_b) = result_b.expect("session B panicked");

    // Session A: tool log should only contain path /tmp/a.txt and content_a
    for record in &tool_log_a {
        if record.tool_name == "read_file" {
            if let Ok(ref val) = record.result {
                let content_str = val.to_string();
                assert!(
                    !content_str.contains("content_b"),
                    "Session A saw content_b in tool result"
                );
            }
        }
    }

    // Session B: tool log should only contain path /tmp/b.txt and content_b
    for record in &tool_log_b {
        if record.tool_name == "read_file" {
            if let Ok(ref val) = record.result {
                let content_str = val.to_string();
                assert!(
                    !content_str.contains("content_a"),
                    "Session B saw content_a in tool result"
                );
            }
        }
    }

    // Check text content isolation
    let texts_a = extract_text_content(&msgs_a);
    let texts_b = extract_text_content(&msgs_b);

    for text in &texts_a {
        assert!(
            !text.contains("Session B done"),
            "Session A saw Session B text content"
        );
    }
    for text in &texts_b {
        assert!(
            !text.contains("Session A done"),
            "Session B saw Session A text content"
        );
    }

    let elapsed = timer.stop();
    bench.add_sample(elapsed);
    bench.add_metric("isolation_verification_ms", elapsed.as_secs_f64() * 1000.0);
    print_bench_inline(&bench);
}

// ============================================================================
// Dimension I: Session Persistence
// ============================================================================

/// I.1: Save a 10-turn conversation to MemorySessionStorage, load it back,
/// and verify full fidelity (message count, types, content).
#[tokio::test]
async fn test_session_save_restore_fidelity() {
    let mut bench = BenchResult::new("I.1: session save/restore fidelity");

    // Build a 10-turn scenario
    let mut responses = Vec::new();
    for i in 0..10 {
        responses.push(MockLlmResponse::Text(format!("Turn {} response", i)));
    }

    let scenario = ScenarioBuilder::new()
        .with_session_id("persist-fidelity-1")
        .with_llm_responses(responses)
        .with_default_llm_response(MockLlmResponse::Text("extra".to_string()))
        .without_compaction()
        .with_max_turns(200)
        .build()
        .await;

    let mut agent = scenario.agent_runner;

    // Run 10-turn conversation
    let mut all_messages: Vec<AgentMessage> = Vec::new();
    for i in 0..10 {
        let prompt = format!("Turn {} prompt", i);
        let msgs = collect_messages(&mut agent, &prompt).await;
        all_messages.extend(msgs);
    }

    let original_count = all_messages.len();
    assert!(original_count > 0, "Should have collected messages");

    // Build a snapshot
    let mut metadata = SessionMetadata::new("persist-fidelity-1", "/tmp");
    metadata.message_count = original_count as u32;
    metadata.turn_count = 10;

    let snapshot = SessionSnapshot::new(metadata, all_messages.clone());

    let storage = MemorySessionStorage::new();

    // Save
    let save_timer = BenchTimer::start("save");
    storage.save(&snapshot).await.expect("save should succeed");
    let save_dur = save_timer.stop();
    bench.add_sample(save_dur);

    // Load
    let restore_timer = BenchTimer::start("restore");
    let loaded = storage
        .load("persist-fidelity-1")
        .await
        .expect("load should succeed");
    let restore_dur = restore_timer.stop();
    bench.add_sample(restore_dur);

    // Verify fidelity
    assert_eq!(
        loaded.messages.len(),
        original_count,
        "Loaded message count should match original"
    );
    assert_eq!(loaded.metadata.id, "persist-fidelity-1");

    // Verify message types match
    for (i, (original, restored)) in all_messages.iter().zip(loaded.messages.iter()).enumerate() {
        let orig_type = std::mem::discriminant(original);
        let rest_type = std::mem::discriminant(restored);
        assert_eq!(
            orig_type, rest_type,
            "Message {} type mismatch: {:?} vs {:?}",
            i, original, restored
        );
    }

    // Verify content by serializing and comparing JSON
    let original_json = serde_json::to_string(&all_messages).unwrap();
    let loaded_json = serde_json::to_string(&loaded.messages).unwrap();
    assert_eq!(
        original_json, loaded_json,
        "Serialized message content should match exactly"
    );

    // Estimate serialized size
    let serialized_size = serde_json::to_vec(&snapshot).unwrap().len();

    bench.add_metric("save_ms", save_dur.as_secs_f64() * 1000.0);
    bench.add_metric("restore_ms", restore_dur.as_secs_f64() * 1000.0);
    bench.add_metric("serialized_size_bytes", serialized_size as f64);
    bench.add_metric("message_count", original_count as f64);
    print_bench_inline(&bench);
}

/// I.2: Run 10 turns, save snapshot, create new AgentRunner, continue for 5 more turns.
/// Verifies 15 total turns of interaction complete.
#[tokio::test]
async fn test_session_restore_and_continue() {
    let mut bench = BenchResult::new("I.2: session restore and continue");

    // Phase 1: Run 10 turns
    let mock_llm_1 = Arc::new(MockLlmClient::new());
    mock_llm_1
        .set_default_response(MockLlmResponse::Text("Phase 1 response".to_string()))
        .await;

    let mock_tools_1 = Arc::new(MockToolExecutor::new());
    mock_tools_1.register_filesystem_tools().await;

    let mut config1 = AgentConfig::default();
    config1.max_turns = 200;
    config1.permission_mode = PermissionMode::BypassPermissions;
    config1.compaction = CompactionConfig::disabled();

    let mut agent1 = AgentRunner::with_session_id(config1, "persist-continue-1")
        .with_llm(mock_llm_1.clone() as Arc<dyn LlmClient>)
        .with_tools(mock_tools_1.clone() as Arc<dyn ToolExecutor>);

    let mut phase1_messages: Vec<AgentMessage> = Vec::new();
    for i in 0..10 {
        let msgs = collect_messages(&mut agent1, &format!("Phase1 turn {}", i)).await;
        phase1_messages.extend(msgs);
    }

    let phase1_count = phase1_messages.len();
    assert!(phase1_count > 0, "Phase 1 should produce messages");

    // Save snapshot
    let save_timer = BenchTimer::start("save");
    let metadata = SessionMetadata::new("persist-continue-1", "/tmp");
    let snapshot = SessionSnapshot::new(metadata, phase1_messages.clone());
    let storage = MemorySessionStorage::new();
    storage.save(&snapshot).await.expect("save should succeed");
    let save_dur = save_timer.stop();

    // Phase 2: Create new agent and continue
    let restore_timer = BenchTimer::start("restore");
    let loaded = storage
        .load("persist-continue-1")
        .await
        .expect("load should succeed");
    let restore_dur = restore_timer.stop();

    assert_eq!(loaded.messages.len(), phase1_count);

    let mock_llm_2 = Arc::new(MockLlmClient::new());
    mock_llm_2
        .set_default_response(MockLlmResponse::Text("Phase 2 response".to_string()))
        .await;

    let mock_tools_2 = Arc::new(MockToolExecutor::new());
    mock_tools_2.register_filesystem_tools().await;

    let mut config2 = AgentConfig::default();
    config2.max_turns = 200;
    config2.permission_mode = PermissionMode::BypassPermissions;
    config2.compaction = CompactionConfig::disabled();

    let mut agent2 = AgentRunner::with_session_id(config2, "persist-continue-1")
        .with_llm(mock_llm_2.clone() as Arc<dyn LlmClient>)
        .with_tools(mock_tools_2.clone() as Arc<dyn ToolExecutor>);

    // Phase 2: Run 5 more turns
    let continuation_timer = BenchTimer::start("continuation_first_turn");
    let first_continuation = collect_messages(&mut agent2, "Phase2 turn 0").await;
    let continuation_first_dur = continuation_timer.stop();
    assert!(
        has_result_message(&first_continuation),
        "First continuation turn should produce a result"
    );

    let mut phase2_messages: Vec<AgentMessage> = first_continuation;
    for i in 1..5 {
        let msgs = collect_messages(&mut agent2, &format!("Phase2 turn {}", i)).await;
        phase2_messages.extend(msgs);
    }

    let phase2_count = phase2_messages.len();
    assert!(phase2_count > 0, "Phase 2 should produce messages");

    // Verify we had 10 + 5 = 15 turns worth of interaction
    let total_llm_calls = mock_llm_1.call_count() + mock_llm_2.call_count();
    assert!(
        total_llm_calls >= 15,
        "Should have at least 15 LLM calls across both phases, got {}",
        total_llm_calls
    );

    bench.add_sample(save_dur);
    bench.add_sample(restore_dur);
    bench.add_sample(continuation_first_dur);
    bench.add_metric("save_ms", save_dur.as_secs_f64() * 1000.0);
    bench.add_metric("restore_ms", restore_dur.as_secs_f64() * 1000.0);
    bench.add_metric(
        "continuation_first_turn_ms",
        continuation_first_dur.as_secs_f64() * 1000.0,
    );
    bench.add_metric("phase1_messages", phase1_count as f64);
    bench.add_metric("phase2_messages", phase2_count as f64);
    print_bench_inline(&bench);
}

/// I.3: Create 10 sessions, each with 20 messages. Save all concurrently,
/// then load all concurrently. Verify all 10 restored correctly.
#[tokio::test]
async fn test_10_sessions_concurrent_persistence() {
    let mut bench = BenchResult::new("I.3: 10 sessions concurrent persistence");

    let storage = Arc::new(MemorySessionStorage::new());

    // Build 10 snapshots, each with 20 messages
    let mut snapshots: Vec<SessionSnapshot> = Vec::new();
    for session_idx in 0..10u32 {
        let session_id = format!("concurrent-persist-{}", session_idx);
        let mut messages: Vec<AgentMessage> = Vec::new();
        for msg_idx in 0..20u32 {
            if msg_idx % 3 == 0 {
                messages.push(AgentMessage::User(UserMessage {
                    content: MessageContent::text(format!(
                        "Session {} message {}",
                        session_idx, msg_idx
                    )),
                    uuid: Some(uuid::Uuid::new_v4()),
                    parent_tool_use_id: None,
                    tool_use_result: None,
                }));
            } else if msg_idx % 3 == 1 {
                messages.push(AgentMessage::Assistant(AssistantMessage {
                    content: vec![ContentBlock::text(format!(
                        "Reply {} for session {}",
                        msg_idx, session_idx
                    ))],
                    model: "mock-model".to_string(),
                    parent_tool_use_id: None,
                    error: None,
                }));
            } else {
                messages.push(AgentMessage::System(SystemMessage {
                    subtype: "info".to_string(),
                    data: json!({"session": session_idx, "msg": msg_idx}),
                }));
            }
        }

        let mut metadata = SessionMetadata::new(&session_id, "/tmp");
        metadata.message_count = 20;
        metadata.turn_count = 7;
        snapshots.push(SessionSnapshot::new(metadata, messages));
    }

    // Save all concurrently
    let save_timer = BenchTimer::start("concurrent_save");
    let save_handles: Vec<_> = snapshots
        .iter()
        .map(|snap| {
            let storage = storage.clone();
            let snapshot = snap.clone();
            tokio::spawn(async move {
                storage.save(&snapshot).await.expect("save should succeed");
            })
        })
        .collect();
    futures::future::join_all(save_handles).await;
    let save_dur = save_timer.stop();

    // Load all concurrently
    let load_timer = BenchTimer::start("concurrent_load");
    let load_handles: Vec<_> = (0..10u32)
        .map(|idx| {
            let storage = storage.clone();
            let session_id = format!("concurrent-persist-{}", idx);
            tokio::spawn(async move {
                storage
                    .load(&session_id)
                    .await
                    .expect("load should succeed")
            })
        })
        .collect();
    let loaded_results: Vec<_> = futures::future::join_all(load_handles).await;
    let load_dur = load_timer.stop();

    // Verify all 10 restored correctly
    for (idx, result) in loaded_results.iter().enumerate() {
        let loaded = result.as_ref().expect("load task panicked");
        assert_eq!(
            loaded.metadata.id,
            format!("concurrent-persist-{}", idx),
            "Session ID mismatch at index {}",
            idx
        );
        assert_eq!(
            loaded.messages.len(),
            20,
            "Session {} should have 20 messages, got {}",
            idx,
            loaded.messages.len()
        );

        // Verify content matches original
        let original_json = serde_json::to_string(&snapshots[idx].messages).unwrap();
        let loaded_json = serde_json::to_string(&loaded.messages).unwrap();
        assert_eq!(
            original_json, loaded_json,
            "Session {} content mismatch after restore",
            idx
        );
    }

    bench.add_sample(save_dur);
    bench.add_sample(load_dur);
    bench.add_metric("concurrent_save_10_ms", save_dur.as_secs_f64() * 1000.0);
    bench.add_metric("concurrent_load_10_ms", load_dur.as_secs_f64() * 1000.0);
    bench.add_metric(
        "per_session_save_avg_ms",
        save_dur.as_secs_f64() * 1000.0 / 10.0,
    );
    bench.add_metric(
        "per_session_load_avg_ms",
        load_dur.as_secs_f64() * 1000.0 / 10.0,
    );
    print_bench_inline(&bench);
}

// ============================================================================
// Dimension J: Intent Recognition Accuracy
// ============================================================================

/// J.1: Test intent recognition with 10 messages covering different intent types
/// using the quick_check pattern-based approach.
/// Also sends each through the full agent loop to verify appropriate handling.
#[tokio::test]
async fn test_intent_recognition_10_types() {
    let mut bench = BenchResult::new("J.1: intent recognition 10 types");

    // Create an IntentRecognizer (requires LlmRouter; we create one with
    // default config - quick_check does not hit the LLM).
    let llm_router = Arc::new(LlmRouter::new(LlmConfig::default()));
    let recognizer = IntentRecognizer::new(llm_router);

    // Define test cases: (message, expected IntentType)
    let test_cases: Vec<(&str, IntentType)> = vec![
        // Greetings -> SimpleChat
        ("Hello", IntentType::SimpleChat),
        ("Hi there!", IntentType::SimpleChat),
        // Tasks -> Task
        ("Create a Python script to sort files", IntentType::Task),
        ("Analyze this code for bugs", IntentType::Task),
        ("Write a function to parse CSV files", IntentType::Task),
        ("Generate a report of sales data", IntentType::Task),
        // Short questions -> SimpleChat (via length heuristic)
        ("How?", IntentType::SimpleChat),
        ("Why?", IntentType::SimpleChat),
        // Tasks with clear keywords
        (
            "Search for all Python files in the project",
            IntentType::Task,
        ),
        ("Delete the temporary files from /tmp", IntentType::Task),
    ];

    let mut correct = 0u32;
    let total = test_cases.len() as u32;

    let total_timer = BenchTimer::start("total_intent");
    let mut per_intent_durations = Vec::new();

    for (message, expected_type) in &test_cases {
        let intent_timer = Instant::now();
        let detected = recognizer.quick_check(message);
        let intent_dur = intent_timer.elapsed();
        per_intent_durations.push(intent_dur);

        if detected == *expected_type {
            correct += 1;
        } else {
            println!(
                "  [INTENT] Mismatch: \"{}\" expected {:?}, got {:?}",
                message, expected_type, detected
            );
        }
    }
    let total_dur = total_timer.stop();

    // Also run each message through the agent loop to verify it responds
    for (message, _) in &test_cases {
        let scenario = ScenarioBuilder::new()
            .with_default_llm_response(MockLlmResponse::Text(format!("Response to: {}", message)))
            .without_compaction()
            .with_max_turns(5)
            .build()
            .await;

        let mut agent = scenario.agent_runner;
        let msgs = collect_messages(&mut agent, message).await;
        assert!(
            has_result_message(&msgs),
            "Agent should produce a result for: {}",
            message
        );
    }

    let accuracy = (correct as f64) / (total as f64) * 100.0;
    assert!(
        accuracy >= 70.0,
        "Intent recognition accuracy should be >= 70%, got {:.1}%",
        accuracy
    );

    let avg_per_intent_ns: f64 = per_intent_durations
        .iter()
        .map(|d| d.as_nanos() as f64)
        .sum::<f64>()
        / per_intent_durations.len() as f64;

    bench.add_sample(total_dur);
    bench.add_metric("accuracy_percent", accuracy);
    bench.add_metric("correct_count", correct as f64);
    bench.add_metric("total_count", total as f64);
    bench.add_metric("per_intent_avg_ns", avg_per_intent_ns);
    bench.add_metric("total_ms", total_dur.as_secs_f64() * 1000.0);
    print_bench_inline(&bench);
}

/// J.2: Compare response speed for simple vs complex messages through the
/// full agent loop. Sends 10 simple and 10 complex messages.
#[tokio::test]
async fn test_intent_speed_vs_accuracy() {
    let mut bench = BenchResult::new("J.2: intent speed simple vs complex");

    let simple_messages = [
        "Hello", "Hi", "Thanks", "Ok", "Yes", "No", "Hey", "Bye", "Sure", "Good",
    ];

    let complex_messages = [
        "Create a comprehensive Python data pipeline that reads CSV files from S3, transforms the data using pandas, and writes to PostgreSQL",
        "Analyze the performance bottlenecks in our Kubernetes cluster by examining pod resource usage, network latency, and disk I/O metrics",
        "Generate a full-stack web application with React frontend, Express backend, and MongoDB database including authentication",
        "Write a machine learning model to predict customer churn using gradient boosting with feature engineering and cross-validation",
        "Build a CI/CD pipeline using GitHub Actions that includes linting, testing, building Docker images, and deploying to AWS ECS",
        "Create an automated monitoring system that tracks API response times, error rates, and sends Slack alerts when thresholds are breached",
        "Design and implement a distributed cache layer using Redis with automatic failover, connection pooling, and TTL management",
        "Write a comprehensive test suite for our microservices architecture including unit tests, integration tests, and contract tests",
        "Implement a real-time data streaming pipeline using Apache Kafka with exactly-once semantics and dead letter queues",
        "Create a security audit tool that scans our codebase for vulnerabilities, checks dependency versions, and generates compliance reports",
    ];

    // Run simple messages
    let mut simple_durations = Vec::new();
    for msg in &simple_messages {
        let scenario = ScenarioBuilder::new()
            .with_default_llm_response(MockLlmResponse::Text("Simple reply".to_string()))
            .without_compaction()
            .with_max_turns(5)
            .build()
            .await;

        let mut agent = scenario.agent_runner;
        let start = Instant::now();
        let msgs = collect_messages(&mut agent, msg).await;
        let dur = start.elapsed();
        simple_durations.push(dur);

        assert!(
            has_result_message(&msgs),
            "Simple message should produce result: {}",
            msg
        );
    }

    // Run complex messages
    let mut complex_durations = Vec::new();
    for msg in &complex_messages {
        let scenario = ScenarioBuilder::new()
            .with_default_llm_response(MockLlmResponse::Text("Complex reply".to_string()))
            .without_compaction()
            .with_max_turns(5)
            .build()
            .await;

        let mut agent = scenario.agent_runner;
        let start = Instant::now();
        let msgs = collect_messages(&mut agent, msg).await;
        let dur = start.elapsed();
        complex_durations.push(dur);

        assert!(
            has_result_message(&msgs),
            "Complex message should produce result"
        );
    }

    let simple_avg_ms: f64 = simple_durations
        .iter()
        .map(|d| d.as_secs_f64() * 1000.0)
        .sum::<f64>()
        / simple_durations.len() as f64;

    let complex_avg_ms: f64 = complex_durations
        .iter()
        .map(|d| d.as_secs_f64() * 1000.0)
        .sum::<f64>()
        / complex_durations.len() as f64;

    // The speedup ratio shows how much faster simple messages are processed.
    // With mock LLM, latencies are similar, so ratio should be close to 1.
    // We simply verify both complete and report the ratio.
    let speedup_ratio = if simple_avg_ms > 0.0 {
        complex_avg_ms / simple_avg_ms
    } else {
        1.0
    };

    // Record all samples
    for d in &simple_durations {
        bench.add_sample(*d);
    }
    for d in &complex_durations {
        bench.add_sample(*d);
    }

    bench.add_metric("simple_avg_ms", simple_avg_ms);
    bench.add_metric("complex_avg_ms", complex_avg_ms);
    bench.add_metric("speedup_ratio", speedup_ratio);
    bench.add_metric("simple_count", simple_messages.len() as f64);
    bench.add_metric("complex_count", complex_messages.len() as f64);
    print_bench_inline(&bench);
}
