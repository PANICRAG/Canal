//! Criterion Performance Benchmarks for Canal Agent
//!
//! Measures core agent loop performance across multiple dimensions:
//! - Conversation turns (1/5/10/20)
//! - Tool chain depth (1/3/5/10)
//! - Context compaction (50/100/200 messages)
//! - Session persistence (10/50/100 messages)
//! - Concurrent sessions (1/5/10/20)
//! - Permission checking
//! - Intent quick_check
//! - Streaming throughput

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use gateway_core::agent::r#loop::config::{AgentConfig, CompactionConfig};
use gateway_core::agent::r#loop::runner::{
    AgentRunner, LlmClient, LlmResponse, StopReason, ToolExecutor,
};
use gateway_core::agent::session::{
    CompactTrigger, ContextCompactor, MemorySessionStorage, SessionMetadata, SessionSnapshot,
    SessionStorage,
};
use gateway_core::agent::tools::ToolContext;
use gateway_core::agent::types::{
    AgentMessage, AssistantMessage, ContentBlock, MessageContent, PermissionContext,
    PermissionMode, PermissionResult, SystemMessage, Usage, UserMessage,
};
use gateway_core::agent::{AgentError, AgentLoop};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

// ──────────────────────────────────────────────────────────────────────
// Inline Mock LLM (self-contained for bench harness)
// ──────────────────────────────────────────────────────────────────────

struct BenchLlmClient {
    response_queue: Arc<Mutex<VecDeque<(Vec<ContentBlock>, StopReason)>>>,
    default_content: Vec<ContentBlock>,
}

impl BenchLlmClient {
    fn text_only() -> Self {
        Self {
            response_queue: Arc::new(Mutex::new(VecDeque::new())),
            default_content: vec![ContentBlock::text("Benchmark response")],
        }
    }

    fn with_queue(responses: Vec<(Vec<ContentBlock>, StopReason)>) -> Self {
        Self {
            response_queue: Arc::new(Mutex::new(VecDeque::from(responses))),
            default_content: vec![ContentBlock::text("Benchmark response")],
        }
    }
}

#[async_trait]
impl LlmClient for BenchLlmClient {
    async fn generate(
        &self,
        _messages: Vec<AgentMessage>,
        _tools: Vec<Value>,
    ) -> Result<LlmResponse, AgentError> {
        let mut queue = self.response_queue.lock().await;
        let (content, stop_reason) = if let Some(resp) = queue.pop_front() {
            resp
        } else {
            (self.default_content.clone(), StopReason::EndTurn)
        };

        Ok(LlmResponse {
            content,
            model: "bench-model".to_string(),
            usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            stop_reason,
        })
    }
}

// ──────────────────────────────────────────────────────────────────────
// Inline Mock Tool Executor
// ──────────────────────────────────────────────────────────────────────

struct BenchToolExecutor;

#[async_trait]
impl ToolExecutor for BenchToolExecutor {
    async fn execute(
        &self,
        _tool_name: &str,
        _tool_input: Value,
        _context: &ToolContext,
    ) -> Result<Value, AgentError> {
        Ok(serde_json::json!({"result": "ok"}))
    }

    fn get_tool_schemas(&self) -> Vec<Value> {
        vec![serde_json::json!({
            "name": "test_tool",
            "description": "A test tool",
            "input_schema": {
                "type": "object",
                "properties": { "input": {"type": "string"} }
            }
        })]
    }
}

// ──────────────────────────────────────────────────────────────────────
// Helper: run N turns of conversation
// ──────────────────────────────────────────────────────────────────────

async fn run_n_turns(n: usize, llm: Arc<dyn LlmClient>, tools: Arc<dyn ToolExecutor>) {
    let config = AgentConfig {
        max_turns: (n as u32) * 2 + 10,
        permission_mode: PermissionMode::BypassPermissions,
        system_prompt: Some("Bench agent".to_string()),
        compaction: CompactionConfig::disabled(),
        ..Default::default()
    };

    let mut runner = AgentRunner::new(config).with_llm(llm).with_tools(tools);

    for i in 0..n {
        let stream = runner.query(&format!("Turn {i}")).await;
        futures::pin_mut!(stream);
        while let Some(msg) = stream.next().await {
            let _ = msg;
        }
    }
}

// Helper: build tool chain responses (N tool calls then text)
fn build_tool_chain(depth: usize) -> Vec<(Vec<ContentBlock>, StopReason)> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut responses = Vec::with_capacity(depth + 1);
    for _ in 0..depth {
        let id = format!("bench_tool_{}", COUNTER.fetch_add(1, Ordering::SeqCst));
        responses.push((
            vec![ContentBlock::tool_use(
                id,
                "test_tool",
                serde_json::json!({"input": "bench"}),
            )],
            StopReason::ToolUse,
        ));
    }
    responses.push((vec![ContentBlock::text("Done")], StopReason::EndTurn));
    responses
}

// Helper: build messages for compaction testing
fn build_messages(count: usize) -> Vec<AgentMessage> {
    let mut messages = Vec::with_capacity(count);
    for i in 0..count {
        if i % 2 == 0 {
            messages.push(AgentMessage::User(UserMessage {
                content: MessageContent::text(format!(
                    "Message {i}: This is a test message with some content to estimate tokens from."
                )),
                uuid: None,
                parent_tool_use_id: None,
                tool_use_result: None,
            }));
        } else {
            messages.push(AgentMessage::Assistant(AssistantMessage {
                content: vec![ContentBlock::text(format!(
                    "Response {i}: This is a response with enough text to be realistic for token counting."
                ))],
                model: "bench-model".to_string(),
                parent_tool_use_id: None,
                error: None,
            }));
        }
    }
    messages
}

// ──────────────────────────────────────────────────────────────────────
// Benchmark: Conversation Turns
// ──────────────────────────────────────────────────────────────────────

fn bench_conversation_turns(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("conversation_turns");

    for turns in [1, 5, 10, 20] {
        group.throughput(Throughput::Elements(turns as u64));
        group.bench_with_input(BenchmarkId::from_parameter(turns), &turns, |b, &turns| {
            b.to_async(&rt).iter(|| {
                let llm: Arc<dyn LlmClient> = Arc::new(BenchLlmClient::text_only());
                let tools: Arc<dyn ToolExecutor> = Arc::new(BenchToolExecutor);
                run_n_turns(turns, llm, tools)
            });
        });
    }
    group.finish();
}

// ──────────────────────────────────────────────────────────────────────
// Benchmark: Tool Chain Depth
// ──────────────────────────────────────────────────────────────────────

fn bench_tool_chain_depth(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("tool_chain_depth");

    for depth in [1, 3, 5, 10] {
        group.throughput(Throughput::Elements(depth as u64));
        group.bench_with_input(BenchmarkId::from_parameter(depth), &depth, |b, &depth| {
            b.to_async(&rt).iter(|| async move {
                let chain = build_tool_chain(depth);
                let llm: Arc<dyn LlmClient> = Arc::new(BenchLlmClient::with_queue(chain));
                let tools: Arc<dyn ToolExecutor> = Arc::new(BenchToolExecutor);

                let config = AgentConfig {
                    max_turns: (depth as u32) + 5,
                    permission_mode: PermissionMode::BypassPermissions,
                    system_prompt: Some("Bench agent".to_string()),
                    compaction: CompactionConfig::disabled(),
                    ..Default::default()
                };

                let mut runner = AgentRunner::new(config).with_llm(llm).with_tools(tools);

                let stream = runner.query("Do tool chain").await;
                futures::pin_mut!(stream);
                while let Some(msg) = stream.next().await {
                    let _ = msg;
                }
            });
        });
    }
    group.finish();
}

// ──────────────────────────────────────────────────────────────────────
// Benchmark: Context Compaction
// ──────────────────────────────────────────────────────────────────────

fn bench_context_compaction(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("context_compaction");

    for msg_count in [50, 100, 200] {
        group.throughput(Throughput::Elements(msg_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(msg_count),
            &msg_count,
            |b, &msg_count| {
                let messages = build_messages(msg_count);
                b.to_async(&rt).iter(|| {
                    let msgs = messages.clone();
                    async move {
                        let compactor = ContextCompactor::new()
                            .max_tokens(1000) // Low threshold to force compaction
                            .target_tokens(500)
                            .keep_recent(10);

                        let _result = compactor
                            .compact(&msgs, CompactTrigger::Manual)
                            .await
                            .unwrap();
                    }
                });
            },
        );
    }
    group.finish();
}

// ──────────────────────────────────────────────────────────────────────
// Benchmark: Session Persistence
// ──────────────────────────────────────────────────────────────────────

fn bench_session_persistence(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("session_persistence");

    for msg_count in [10, 50, 100] {
        group.throughput(Throughput::Elements(msg_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(msg_count),
            &msg_count,
            |b, &msg_count| {
                let messages = build_messages(msg_count);
                b.to_async(&rt).iter(|| {
                    let msgs = messages.clone();
                    async move {
                        let storage = MemorySessionStorage::new();
                        let session_id = format!("bench-session-{msg_count}");

                        let metadata = SessionMetadata::new(&session_id, "/tmp");
                        let snapshot = SessionSnapshot::new(metadata, msgs);

                        // Save
                        storage.save(&snapshot).await.unwrap();

                        // Load
                        let _loaded = storage.load(&session_id).await.unwrap();
                    }
                });
            },
        );
    }
    group.finish();
}

// ──────────────────────────────────────────────────────────────────────
// Benchmark: Concurrent Sessions
// ──────────────────────────────────────────────────────────────────────

fn bench_concurrent_sessions(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("concurrent_sessions");

    for session_count in [1, 5, 10, 20] {
        group.throughput(Throughput::Elements(session_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(session_count),
            &session_count,
            |b, &session_count| {
                b.to_async(&rt).iter(|| async move {
                    let handles: Vec<_> = (0..session_count)
                        .map(|i| {
                            tokio::spawn(async move {
                                let llm: Arc<dyn LlmClient> = Arc::new(BenchLlmClient::text_only());
                                let tools: Arc<dyn ToolExecutor> = Arc::new(BenchToolExecutor);

                                let config = AgentConfig {
                                    max_turns: 10,
                                    permission_mode: PermissionMode::BypassPermissions,
                                    system_prompt: Some("Bench agent".to_string()),
                                    compaction: CompactionConfig::disabled(),
                                    ..Default::default()
                                };

                                let mut runner = AgentRunner::with_session_id(
                                    config,
                                    format!("bench-session-{i}"),
                                )
                                .with_llm(llm)
                                .with_tools(tools);

                                let stream = runner.query("Hello").await;
                                futures::pin_mut!(stream);
                                while let Some(msg) = stream.next().await {
                                    let _ = msg;
                                }
                            })
                        })
                        .collect();

                    for h in handles {
                        h.await.unwrap();
                    }
                });
            },
        );
    }
    group.finish();
}

// ──────────────────────────────────────────────────────────────────────
// Benchmark: Permission Check
// ──────────────────────────────────────────────────────────────────────

fn bench_permission_check(c: &mut Criterion) {
    let mut group = c.benchmark_group("permission_check");

    for count in [10, 100, 1000] {
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, &count| {
            b.iter(|| {
                let ctx = PermissionContext::default();
                for i in 0..count {
                    let _result = ctx.check_tool(
                        &format!("tool_{i}"),
                        &serde_json::json!({"path": format!("/tmp/file_{i}")}),
                    );
                }
            });
        });
    }
    group.finish();
}

// ──────────────────────────────────────────────────────────────────────
// Benchmark: Intent Quick Check
// ──────────────────────────────────────────────────────────────────────

fn bench_intent_quick_check(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("intent_quick_check");

    let messages = [
        ("short", "Hello"),
        ("medium", "Can you help me write a Python script that processes CSV files?"),
        ("long", "I need you to analyze the performance bottlenecks in my Rust application. The main issue is that the database queries are taking too long, and I suspect there might be N+1 query problems in the user listing endpoint. Can you read the source code and suggest optimizations?"),
    ];

    for (label, msg) in &messages {
        group.bench_with_input(BenchmarkId::from_parameter(label), msg, |b, &msg| {
            b.to_async(&rt).iter(|| async move {
                // Simulate quick_check pattern matching
                let _intent = if msg.starts_with('/') {
                    "SystemCommand"
                } else if msg.len() < 20
                    && (msg.to_lowercase().contains("hello") || msg.to_lowercase().contains("hi"))
                {
                    "SimpleChat"
                } else if msg.contains('?')
                    && (msg.to_lowercase().contains("what")
                        || msg.to_lowercase().contains("how")
                        || msg.to_lowercase().contains("explain"))
                {
                    "Clarification"
                } else {
                    "Task"
                };
            });
        });
    }
    group.finish();
}

// ──────────────────────────────────────────────────────────────────────
// Benchmark: Streaming Throughput
// ──────────────────────────────────────────────────────────────────────

fn bench_streaming_throughput(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("streaming_throughput");

    for event_count in [10, 50, 100] {
        group.throughput(Throughput::Elements(event_count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(event_count),
            &event_count,
            |b, &event_count| {
                b.to_async(&rt).iter(|| async move {
                    let llm: Arc<dyn LlmClient> = Arc::new(BenchLlmClient::text_only());
                    let tools: Arc<dyn ToolExecutor> = Arc::new(BenchToolExecutor);

                    let config = AgentConfig {
                        max_turns: (event_count as u32) + 5,
                        permission_mode: PermissionMode::BypassPermissions,
                        system_prompt: Some("Bench agent".to_string()),
                        compaction: CompactionConfig::disabled(),
                        ..Default::default()
                    };

                    let mut runner = AgentRunner::new(config).with_llm(llm).with_tools(tools);

                    let mut total_events = 0u64;
                    // Each query produces ~3-4 events (System + User + Assistant + Result)
                    let queries_needed = (event_count / 3) + 1;
                    for i in 0..queries_needed {
                        let stream = runner.query(&format!("Query {i}")).await;
                        futures::pin_mut!(stream);
                        while let Some(msg) = stream.next().await {
                            let _ = msg;
                            total_events += 1;
                        }
                    }
                });
            },
        );
    }
    group.finish();
}

// ──────────────────────────────────────────────────────────────────────
// Criterion Main
// ──────────────────────────────────────────────────────────────────────

criterion_group!(
    agent_benchmarks,
    bench_conversation_turns,
    bench_tool_chain_depth,
    bench_context_compaction,
    bench_session_persistence,
    bench_concurrent_sessions,
    bench_permission_check,
    bench_intent_quick_check,
    bench_streaming_throughput,
);

criterion_main!(agent_benchmarks);
