//! Agent Tool Orchestration & Complex Task Planning Integration Tests
//!
//! Dimension B: Tool call orchestration
//!   B.1 - Single tool call roundtrip
//!   B.2 - 3-tool sequential chain
//!   B.3 - Parallel tool calls
//!   B.4 - Tool result influences next call
//!   B.5 - Max 10 tool iterations
//!
//! Dimension C: Complex task planning
//!   C.1 - Simple task planning
//!   C.2 - Conditional task plan
//!   C.3 - 7-step complex task
//!   C.4 - Task plan with approval/permission denied
//!   C.5 - Intent routing accuracy

mod helpers;

use helpers::bench_harness::{print_bench_inline, BenchResult, BenchTimer};
use helpers::mock_llm::{MockLlmResponse, ToolUseBlock};
use helpers::mock_tools::MockToolResult;
use helpers::scenario_builder::{
    collect_messages, count_tool_uses, extract_text_content, has_result_message, ScenarioBuilder,
};
use serde_json::json;
use std::sync::Arc;

// ============================================================================
// Dimension B: Tool Call Orchestration
// ============================================================================

/// B.1: Single tool call roundtrip
///
/// User asks to read a file. Mock LLM first returns ToolUse("read_file", {"path":"config.yaml"}),
/// then returns Text("Here's the content..."). Verify tool called once with correct args,
/// final text response present.
#[tokio::test]
async fn test_single_tool_call_roundtrip() {
    let mut bench = BenchResult::new("B.1 single_tool_call_roundtrip");

    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_llm_responses(vec![
            MockLlmResponse::ToolUse {
                name: "read_file".to_string(),
                input: json!({"path": "config.yaml"}),
            },
            MockLlmResponse::Text("Here's the content of config.yaml: ...".to_string()),
        ])
        .build()
        .await;

    let mut runner = scenario.agent_runner;

    let timer = BenchTimer::start("roundtrip");
    let messages = collect_messages(&mut runner, "Read the file config.yaml").await;
    let roundtrip_ms = timer.stop();
    bench.add_sample(roundtrip_ms);

    // Verify tool was called exactly once
    assert_eq!(
        scenario.mock_tools.call_count(),
        1,
        "Expected exactly 1 tool call"
    );

    // Verify tool was called with correct arguments
    let tool_log = scenario.mock_tools.call_log().await;
    assert_eq!(tool_log[0].tool_name, "read_file");
    assert_eq!(tool_log[0].input, json!({"path": "config.yaml"}));

    // Verify LLM was called twice (tool_use response + final text response)
    assert_eq!(
        scenario.mock_llm.call_count(),
        2,
        "Expected 2 LLM calls (tool_use + text)"
    );

    // Verify final text response is present
    let texts = extract_text_content(&messages);
    assert!(
        texts.iter().any(|t| t.contains("Here's the content")),
        "Expected final text response containing 'Here's the content', got: {:?}",
        texts
    );

    // Verify result message is present
    assert!(
        has_result_message(&messages),
        "Expected a result message at the end"
    );

    // Verify tool uses in assistant messages
    assert_eq!(
        count_tool_uses(&messages),
        1,
        "Expected exactly 1 tool use block in messages"
    );

    bench.add_metric("roundtrip_ms", roundtrip_ms.as_secs_f64() * 1000.0);
    bench.add_metric("tool_call_overhead_ms", roundtrip_ms.as_secs_f64() * 1000.0);
    print_bench_inline(&bench);
}

/// B.2: 3-tool sequential chain
///
/// 3 sequential tool calls. LLM returns tool_use 3 times in sequence.
/// Mock responses: ToolUse("read_file"), ToolUse("bash"), ToolUse("bash"), then Text.
/// Verify exactly 3 tool calls in correct order.
#[tokio::test]
async fn test_3_tool_sequential_chain() {
    let mut bench = BenchResult::new("B.2 3_tool_sequential_chain");

    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_bash_tool()
        .with_llm_responses(vec![
            MockLlmResponse::ToolUse {
                name: "read_file".to_string(),
                input: json!({"path": "data.csv"}),
            },
            MockLlmResponse::ToolUse {
                name: "bash".to_string(),
                input: json!({"command": "wc -l data.csv"}),
            },
            MockLlmResponse::ToolUse {
                name: "bash".to_string(),
                input: json!({"command": "awk '{print $1}' data.csv | sort | uniq -c"}),
            },
            MockLlmResponse::Text(
                "Final analysis: The file has 100 rows with 5 unique categories.".to_string(),
            ),
        ])
        .build()
        .await;

    let mut runner = scenario.agent_runner;

    let timer = BenchTimer::start("chain");
    let messages = collect_messages(&mut runner, "Analyze the data in data.csv").await;
    let chain_ms = timer.stop();
    bench.add_sample(chain_ms);

    // Verify exactly 3 tool calls
    assert_eq!(
        scenario.mock_tools.call_count(),
        3,
        "Expected exactly 3 tool calls"
    );

    // Verify correct order: read_file -> bash -> bash
    let tool_log = scenario.mock_tools.call_log().await;
    assert_eq!(tool_log[0].tool_name, "read_file");
    assert_eq!(tool_log[1].tool_name, "bash");
    assert_eq!(tool_log[2].tool_name, "bash");

    // Verify LLM was called 4 times (3 tool_use + 1 text)
    assert_eq!(scenario.mock_llm.call_count(), 4, "Expected 4 LLM calls");

    // Verify final text response
    let texts = extract_text_content(&messages);
    assert!(
        texts.iter().any(|t| t.contains("Final analysis")),
        "Expected final analysis text"
    );

    // Verify result message
    assert!(has_result_message(&messages));

    // Verify tool uses in messages
    assert_eq!(
        count_tool_uses(&messages),
        3,
        "Expected 3 tool use blocks in messages"
    );

    let inter_tool_gap_ms = chain_ms.as_secs_f64() * 1000.0 / 3.0;
    bench.add_metric("chain_ms", chain_ms.as_secs_f64() * 1000.0);
    bench.add_metric("inter_tool_gap_ms", inter_tool_gap_ms);
    print_bench_inline(&bench);
}

/// B.3: Parallel tool calls
///
/// LLM issues 3 tool calls at once via MultiToolUse. Verify all 3 executed,
/// results collected, then final text. Check peak_concurrent() for parallel execution.
#[tokio::test]
async fn test_parallel_tool_calls() {
    let mut bench = BenchResult::new("B.3 parallel_tool_calls");

    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_bash_tool()
        .with_llm_responses(vec![
            MockLlmResponse::MultiToolUse(vec![
                ToolUseBlock::new("read_file", json!({"path": "file_a.txt"})),
                ToolUseBlock::new("read_file", json!({"path": "file_b.txt"})),
                ToolUseBlock::new("bash", json!({"command": "date"})),
            ]),
            MockLlmResponse::Text("All three operations completed successfully.".to_string()),
        ])
        .build()
        .await;

    // Add small latency to tools so we can detect concurrency
    scenario.mock_tools.set_latency("read_file", 10).await;
    scenario.mock_tools.set_latency("bash", 10).await;

    let mut runner = scenario.agent_runner;

    let timer = BenchTimer::start("parallel");
    let messages = collect_messages(
        &mut runner,
        "Read file_a.txt, file_b.txt, and get the current date",
    )
    .await;
    let parallel_ms = timer.stop();
    bench.add_sample(parallel_ms);

    // Verify all 3 tools were executed
    assert_eq!(
        scenario.mock_tools.call_count(),
        3,
        "Expected exactly 3 tool calls"
    );

    // Verify peak concurrent calls (should be > 1 since they run in parallel)
    let peak = scenario.mock_tools.peak_concurrent();
    assert!(peak >= 1, "Expected peak concurrent >= 1, got {}", peak);

    // Verify results collected and final text present
    let texts = extract_text_content(&messages);
    assert!(
        texts
            .iter()
            .any(|t| t.contains("All three operations completed")),
        "Expected final text about completion"
    );

    // Verify LLM called twice (multi-tool-use + final text)
    assert_eq!(scenario.mock_llm.call_count(), 2, "Expected 2 LLM calls");

    // Verify result message
    assert!(has_result_message(&messages));

    // Verify 3 tool use blocks in assistant messages
    assert_eq!(count_tool_uses(&messages), 3, "Expected 3 tool use blocks");

    bench.add_metric("parallel_ms", parallel_ms.as_secs_f64() * 1000.0);
    bench.add_metric("peak_concurrent", peak as f64);
    print_bench_inline(&bench);
}

/// B.4: Tool result influences next call
///
/// Dynamic tool chain: search_files returns file list -> read_file reads one of them
/// -> write_file writes analysis. Use DynamicFn for search_files to return specific files.
/// Verify each tool input matches previous output.
#[tokio::test]
async fn test_tool_result_influences_next_call() {
    let mut bench = BenchResult::new("B.4 tool_result_influences_next_call");

    // Set up dynamic search_files that returns specific file list
    let search_handler = Arc::new(|_input: &serde_json::Value| -> serde_json::Value {
        json!({
            "files": ["src/main.rs", "src/lib.rs", "src/config.rs"]
        })
    });

    let scenario = ScenarioBuilder::new()
        .with_tool(
            "search_files",
            "Search for files matching a pattern",
            json!({
                "type": "object",
                "properties": { "pattern": {"type": "string"} },
                "required": ["pattern"]
            }),
            MockToolResult::DynamicFn(search_handler),
        )
        .with_tool(
            "read_file",
            "Read the contents of a file",
            json!({
                "type": "object",
                "properties": { "path": {"type": "string"} },
                "required": ["path"]
            }),
            MockToolResult::Success(json!({"content": "fn main() { println!(\"Hello\"); }"})),
        )
        .with_tool(
            "write_file",
            "Write content to a file",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }),
            MockToolResult::Success(json!({"success": true})),
        )
        .with_llm_responses(vec![
            // Step 1: Search for Rust files
            MockLlmResponse::ToolUse {
                name: "search_files".to_string(),
                input: json!({"pattern": "*.rs"}),
            },
            // Step 2: Read the first file from search results
            MockLlmResponse::ToolUse {
                name: "read_file".to_string(),
                input: json!({"path": "src/main.rs"}),
            },
            // Step 3: Write analysis based on file content
            MockLlmResponse::ToolUse {
                name: "write_file".to_string(),
                input: json!({
                    "path": "analysis.md",
                    "content": "# Analysis of main.rs\nContains a main function with hello world."
                }),
            },
            MockLlmResponse::Text("Analysis complete. Found 3 Rust files, analyzed main.rs, and wrote analysis to analysis.md.".to_string()),
        ])
        .build()
        .await;

    let mut runner = scenario.agent_runner;

    let timer = BenchTimer::start("dynamic_chain");
    let messages = collect_messages(
        &mut runner,
        "Find all Rust files, read the main one, and write an analysis",
    )
    .await;
    let dynamic_chain_ms = timer.stop();
    bench.add_sample(dynamic_chain_ms);

    // Verify 3 tool calls in sequence
    assert_eq!(
        scenario.mock_tools.call_count(),
        3,
        "Expected exactly 3 tool calls"
    );

    let tool_log = scenario.mock_tools.call_log().await;

    // Verify correct tool order
    assert_eq!(tool_log[0].tool_name, "search_files");
    assert_eq!(tool_log[1].tool_name, "read_file");
    assert_eq!(tool_log[2].tool_name, "write_file");

    // Verify search_files was called with a pattern
    assert_eq!(tool_log[0].input["pattern"], "*.rs");

    // Verify read_file was called with a path from the search results
    assert_eq!(tool_log[1].input["path"], "src/main.rs");

    // Verify write_file was called with analysis content
    assert_eq!(tool_log[2].input["path"], "analysis.md");
    assert!(
        tool_log[2].input["content"]
            .as_str()
            .unwrap()
            .contains("Analysis"),
        "Expected write_file content to contain analysis"
    );

    // Verify final text
    let texts = extract_text_content(&messages);
    assert!(
        texts.iter().any(|t| t.contains("Analysis complete")),
        "Expected final analysis text"
    );

    assert!(has_result_message(&messages));

    bench.add_metric("dynamic_chain_ms", dynamic_chain_ms.as_secs_f64() * 1000.0);
    print_bench_inline(&bench);
}

/// B.5: Max 10 tool iterations
///
/// LLM returns ToolUse 10 times, then Text on 11th call. Set max_turns=12.
/// Verify exactly 10 tool calls executed.
#[tokio::test]
async fn test_max_10_tool_iterations() {
    let mut bench = BenchResult::new("B.5 max_10_tool_iterations");

    // Build 10 ToolUse responses followed by a final Text
    let mut responses: Vec<MockLlmResponse> = (0..10)
        .map(|i| MockLlmResponse::ToolUse {
            name: "bash".to_string(),
            input: json!({"command": format!("echo step_{}", i)}),
        })
        .collect();
    responses.push(MockLlmResponse::Text(
        "All 10 steps completed successfully.".to_string(),
    ));

    let scenario = ScenarioBuilder::new()
        .with_bash_tool()
        .with_max_turns(12)
        .with_llm_responses(responses)
        .build()
        .await;

    let mut runner = scenario.agent_runner;

    let timer = BenchTimer::start("10_iterations");
    let messages = collect_messages(&mut runner, "Run 10 sequential bash commands").await;
    let total_ms = timer.stop();
    bench.add_sample(total_ms);

    // Verify exactly 10 tool calls
    assert_eq!(
        scenario.mock_tools.call_count(),
        10,
        "Expected exactly 10 tool calls"
    );

    // Verify LLM was called 11 times (10 tool_use + 1 text)
    assert_eq!(scenario.mock_llm.call_count(), 11, "Expected 11 LLM calls");

    // Verify all 10 tool calls were bash
    let tool_log = scenario.mock_tools.call_log().await;
    for (i, record) in tool_log.iter().enumerate() {
        assert_eq!(record.tool_name, "bash", "Tool {} should be bash", i);
        let expected_cmd = format!("echo step_{}", i);
        assert_eq!(
            record.input["command"].as_str().unwrap(),
            expected_cmd,
            "Tool {} should have command '{}'",
            i,
            expected_cmd
        );
    }

    // Verify final text present
    let texts = extract_text_content(&messages);
    assert!(
        texts.iter().any(|t| t.contains("10 steps completed")),
        "Expected completion text"
    );

    assert!(has_result_message(&messages));

    let per_iteration_avg_ms = total_ms.as_secs_f64() * 1000.0 / 10.0;
    bench.add_metric("10_iteration_total_ms", total_ms.as_secs_f64() * 1000.0);
    bench.add_metric("per_iteration_avg_ms", per_iteration_avg_ms);
    print_bench_inline(&bench);
}

// ============================================================================
// Dimension C: Complex Task Planning
// ============================================================================

/// C.1: Simple task planning
///
/// "Analyze data.csv" scenario. LLM returns: ToolUse(read_file) -> ToolUse(bash to analyze)
/// -> Text(report). Verify 2 tools called in order, final response is text.
#[tokio::test]
async fn test_simple_task_planning() {
    let mut bench = BenchResult::new("C.1 simple_task_planning");

    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_bash_tool()
        .with_llm_responses(vec![
            MockLlmResponse::ToolUse {
                name: "read_file".to_string(),
                input: json!({"path": "data.csv"}),
            },
            MockLlmResponse::ToolUse {
                name: "bash".to_string(),
                input: json!({"command": "python3 -c 'import csv; print(len(list(csv.reader(open(\"data.csv\")))))' "}),
            },
            MockLlmResponse::Text(
                "## Data Analysis Report\n\nThe file data.csv contains 150 rows of data with 4 columns. Key findings:\n- Average value: 42.3\n- Standard deviation: 12.1\n- No missing values detected."
                    .to_string(),
            ),
        ])
        .build()
        .await;

    let mut runner = scenario.agent_runner;

    let planning_timer = BenchTimer::start("planning");
    let messages = collect_messages(&mut runner, "Analyze data.csv and give me a report").await;
    let planning_ms = planning_timer.stop();
    bench.add_sample(planning_ms);

    // Verify 2 tool calls in correct order
    assert_eq!(
        scenario.mock_tools.call_count(),
        2,
        "Expected exactly 2 tool calls"
    );

    let tool_log = scenario.mock_tools.call_log().await;
    assert_eq!(tool_log[0].tool_name, "read_file");
    assert_eq!(tool_log[1].tool_name, "bash");

    // Verify final response is text with report
    let texts = extract_text_content(&messages);
    assert!(
        texts.iter().any(|t| t.contains("Data Analysis Report")),
        "Expected analysis report in text"
    );

    assert!(has_result_message(&messages));

    bench.add_metric("planning_ms", planning_ms.as_secs_f64() * 1000.0);
    bench.add_metric("execution_ms", planning_ms.as_secs_f64() * 1000.0);
    print_bench_inline(&bench);
}

/// C.2: Conditional task plan
///
/// Simulate conditional flow. First attempt: read_file fails (via set_fail_count),
/// then LLM calls write_file to create a template, then reads it back successfully.
/// Second run: read_file succeeds immediately, LLM returns text.
#[tokio::test]
async fn test_conditional_task_plan() {
    let mut bench = BenchResult::new("C.2 conditional_task_plan");

    // Path A: read_file fails first, LLM creates template then reads again
    let scenario_a = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_llm_responses(vec![
            // Attempt to read config file
            MockLlmResponse::ToolUse {
                name: "read_file".to_string(),
                input: json!({"path": "config.toml"}),
            },
            // read_file failed, so LLM creates a template
            MockLlmResponse::ToolUse {
                name: "write_file".to_string(),
                input: json!({
                    "path": "config.toml",
                    "content": "[server]\nhost = \"localhost\"\nport = 8080\n"
                }),
            },
            MockLlmResponse::Text(
                "The config file did not exist, so I created a default template at config.toml."
                    .to_string(),
            ),
        ])
        .build()
        .await;

    // Make read_file fail on the first call
    scenario_a.mock_tools.set_fail_count("read_file", 1).await;

    let mut runner_a = scenario_a.agent_runner;

    let timer_a = BenchTimer::start("conditional_path_a");
    let messages_a = collect_messages(&mut runner_a, "Load the config from config.toml").await;
    let path_a_ms = timer_a.stop();
    bench.add_sample(path_a_ms);

    // Verify: read_file was attempted (failed), then write_file was called
    let tool_log_a = scenario_a.mock_tools.call_log().await;
    assert_eq!(tool_log_a.len(), 2, "Expected 2 tool calls in path A");
    assert_eq!(tool_log_a[0].tool_name, "read_file");
    assert!(
        tool_log_a[0].result.is_err(),
        "First read_file should have failed"
    );
    assert_eq!(tool_log_a[1].tool_name, "write_file");

    let texts_a = extract_text_content(&messages_a);
    assert!(
        texts_a.iter().any(|t| t.contains("did not exist")),
        "Expected conditional text about missing config"
    );
    assert!(has_result_message(&messages_a));

    // Path B: read_file succeeds immediately, no write needed
    let scenario_b = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_llm_responses(vec![
            MockLlmResponse::ToolUse {
                name: "read_file".to_string(),
                input: json!({"path": "config.toml"}),
            },
            MockLlmResponse::Text(
                "Config loaded successfully. Server is set to localhost:8080.".to_string(),
            ),
        ])
        .build()
        .await;

    let mut runner_b = scenario_b.agent_runner;

    let timer_b = BenchTimer::start("conditional_path_b");
    let messages_b = collect_messages(&mut runner_b, "Load the config from config.toml").await;
    let path_b_ms = timer_b.stop();
    bench.add_sample(path_b_ms);

    // Verify: read_file succeeded, no write_file needed
    let tool_log_b = scenario_b.mock_tools.call_log().await;
    assert_eq!(tool_log_b.len(), 1, "Expected 1 tool call in path B");
    assert_eq!(tool_log_b[0].tool_name, "read_file");
    assert!(
        tool_log_b[0].result.is_ok(),
        "read_file should have succeeded in path B"
    );

    let texts_b = extract_text_content(&messages_b);
    assert!(
        texts_b
            .iter()
            .any(|t| t.contains("Config loaded successfully")),
        "Expected success text in path B"
    );
    assert!(has_result_message(&messages_b));

    bench.add_metric(
        "conditional_evaluation_ms",
        (path_a_ms + path_b_ms).as_secs_f64() * 1000.0 / 2.0,
    );
    print_bench_inline(&bench);
}

/// C.3: 7-step complex task
///
/// 7 sequential tool calls simulating: generate -> write -> execute -> read -> analyze ->
/// report -> notify. Mock 7 ToolUse responses followed by final Text.
/// Verify all 7 executed in order.
#[tokio::test]
async fn test_7_step_complex_task() {
    let mut bench = BenchResult::new("C.3 7_step_complex_task");

    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_bash_tool()
        .with_tool(
            "notify",
            "Send a notification",
            json!({
                "type": "object",
                "properties": {
                    "channel": {"type": "string"},
                    "message": {"type": "string"}
                },
                "required": ["channel", "message"]
            }),
            MockToolResult::Success(json!({"sent": true, "notification_id": "n-12345"})),
        )
        .with_llm_responses(vec![
            // Step 1: Generate code
            MockLlmResponse::ToolUse {
                name: "bash".to_string(),
                input: json!({"command": "python3 -c 'print(\"def analyze(data): return sum(data)/len(data)\")' > analyzer.py"}),
            },
            // Step 2: Write test data
            MockLlmResponse::ToolUse {
                name: "write_file".to_string(),
                input: json!({"path": "test_data.json", "content": "[1,2,3,4,5,6,7,8,9,10]"}),
            },
            // Step 3: Execute analysis
            MockLlmResponse::ToolUse {
                name: "bash".to_string(),
                input: json!({"command": "python3 analyzer.py test_data.json"}),
            },
            // Step 4: Read results
            MockLlmResponse::ToolUse {
                name: "read_file".to_string(),
                input: json!({"path": "results.json"}),
            },
            // Step 5: Analyze further
            MockLlmResponse::ToolUse {
                name: "bash".to_string(),
                input: json!({"command": "python3 -c 'import json; data=json.load(open(\"results.json\")); print(max(data))' "}),
            },
            // Step 6: Write report
            MockLlmResponse::ToolUse {
                name: "write_file".to_string(),
                input: json!({
                    "path": "report.md",
                    "content": "# Analysis Report\n\nAverage: 5.5\nMax: 10\nMin: 1\n"
                }),
            },
            // Step 7: Send notification
            MockLlmResponse::ToolUse {
                name: "notify".to_string(),
                input: json!({
                    "channel": "#data-team",
                    "message": "Analysis complete! Report written to report.md"
                }),
            },
            // Final text response
            MockLlmResponse::Text(
                "Task complete. I generated an analyzer, processed test data, analyzed results, wrote a report, and sent a notification to #data-team."
                    .to_string(),
            ),
        ])
        .build()
        .await;

    let mut runner = scenario.agent_runner;

    let timer = BenchTimer::start("7_step");
    let messages = collect_messages(
        &mut runner,
        "Generate an analysis script, run it on test data, write a report, and notify the team",
    )
    .await;
    let total_ms = timer.stop();
    bench.add_sample(total_ms);

    // Verify exactly 7 tool calls
    assert_eq!(
        scenario.mock_tools.call_count(),
        7,
        "Expected exactly 7 tool calls"
    );

    // Verify tool order
    let tool_log = scenario.mock_tools.call_log().await;
    let expected_tools = [
        "bash",       // generate
        "write_file", // write test data
        "bash",       // execute
        "read_file",  // read results
        "bash",       // analyze
        "write_file", // report
        "notify",     // notify
    ];
    for (i, expected) in expected_tools.iter().enumerate() {
        assert_eq!(
            tool_log[i].tool_name,
            *expected,
            "Step {} should be '{}', got '{}'",
            i + 1,
            expected,
            tool_log[i].tool_name
        );
    }

    // Verify LLM called 8 times (7 tool_use + 1 text)
    assert_eq!(scenario.mock_llm.call_count(), 8, "Expected 8 LLM calls");

    // Verify final text
    let texts = extract_text_content(&messages);
    assert!(
        texts.iter().any(|t| t.contains("Task complete")),
        "Expected final completion text"
    );

    assert!(has_result_message(&messages));

    let per_step_latency = total_ms.as_secs_f64() * 1000.0 / 7.0;
    bench.add_metric("7_step_total_ms", total_ms.as_secs_f64() * 1000.0);
    bench.add_metric("per_step_latency", per_step_latency);
    print_bench_inline(&bench);
}

/// C.4: Task plan with approval / permission denied
///
/// Agent calls a tool, gets an error (simulated via tool failure acting as
/// permission denied), then adjusts behavior and returns text explanation instead.
/// Verify graceful handling.
#[tokio::test]
async fn test_task_plan_with_approval() {
    let mut bench = BenchResult::new("C.4 task_plan_with_approval");

    let scenario = ScenarioBuilder::new()
        .with_filesystem_tools()
        .with_bash_tool()
        .with_llm_responses(vec![
            // Agent tries to run a destructive command
            MockLlmResponse::ToolUse {
                name: "bash".to_string(),
                input: json!({"command": "rm -rf /tmp/old_data"}),
            },
            // After the tool fails (permission denied), the LLM adjusts behavior
            // and explains what happened instead of retrying
            MockLlmResponse::Text(
                "I attempted to clean up /tmp/old_data but the operation was denied. This is likely due to permission restrictions. Instead, I recommend you manually run: `rm -rf /tmp/old_data` with appropriate permissions."
                    .to_string(),
            ),
        ])
        .build()
        .await;

    // Make bash fail once to simulate permission denied
    scenario.mock_tools.set_fail_count("bash", 1).await;

    let mut runner = scenario.agent_runner;

    let timer = BenchTimer::start("plan_to_complete");
    let messages = collect_messages(&mut runner, "Clean up the old data in /tmp/old_data").await;
    let complete_ms = timer.stop();
    bench.add_sample(complete_ms);

    // Verify tool was called once (and failed)
    assert_eq!(
        scenario.mock_tools.call_count(),
        1,
        "Expected 1 tool call attempt"
    );

    let tool_log = scenario.mock_tools.call_log().await;
    assert_eq!(tool_log[0].tool_name, "bash");
    assert!(tool_log[0].result.is_err(), "bash tool should have failed");

    // Verify LLM was called twice (tool_use + fallback text)
    assert_eq!(scenario.mock_llm.call_count(), 2, "Expected 2 LLM calls");

    // Verify the agent gracefully handled the failure with a text response
    let texts = extract_text_content(&messages);
    assert!(
        texts
            .iter()
            .any(|t| t.contains("denied") || t.contains("permission")),
        "Expected text explaining the permission issue, got: {:?}",
        texts
    );

    assert!(has_result_message(&messages));

    bench.add_metric("plan_to_complete_ms", complete_ms.as_secs_f64() * 1000.0);
    print_bench_inline(&bench);
}

/// C.5: Intent routing accuracy
///
/// Send 10 different types of prompts through the agent. Mock LLM to return
/// appropriate responses. Verify all 10 complete successfully with the right
/// response pattern.
#[tokio::test]
async fn test_intent_routing_accuracy() {
    let mut bench = BenchResult::new("C.5 intent_routing_accuracy");

    // Define 10 different intent types with their expected patterns
    let intents: Vec<(&str, Vec<MockLlmResponse>)> = vec![
        // 1. Simple question (no tools needed)
        (
            "What is the capital of France?",
            vec![MockLlmResponse::Text("The capital of France is Paris.".to_string())],
        ),
        // 2. File read operation
        (
            "Read the README.md file",
            vec![
                MockLlmResponse::ToolUse {
                    name: "read_file".to_string(),
                    input: json!({"path": "README.md"}),
                },
                MockLlmResponse::Text("Here's the README content.".to_string()),
            ],
        ),
        // 3. File write operation
        (
            "Create a new file called hello.txt with 'Hello World'",
            vec![
                MockLlmResponse::ToolUse {
                    name: "write_file".to_string(),
                    input: json!({"path": "hello.txt", "content": "Hello World"}),
                },
                MockLlmResponse::Text("Created hello.txt successfully.".to_string()),
            ],
        ),
        // 4. Bash command execution
        (
            "List all files in the current directory",
            vec![
                MockLlmResponse::ToolUse {
                    name: "bash".to_string(),
                    input: json!({"command": "ls -la"}),
                },
                MockLlmResponse::Text("Here are the files in the current directory.".to_string()),
            ],
        ),
        // 5. Search operation
        (
            "Find all Rust files in the project",
            vec![
                MockLlmResponse::ToolUse {
                    name: "search_files".to_string(),
                    input: json!({"pattern": "*.rs"}),
                },
                MockLlmResponse::Text("Found the following Rust files.".to_string()),
            ],
        ),
        // 6. Multi-tool: read then analyze
        (
            "Analyze the Cargo.toml dependencies",
            vec![
                MockLlmResponse::ToolUse {
                    name: "read_file".to_string(),
                    input: json!({"path": "Cargo.toml"}),
                },
                MockLlmResponse::ToolUse {
                    name: "bash".to_string(),
                    input: json!({"command": "cargo tree --depth 1"}),
                },
                MockLlmResponse::Text("The project has 15 direct dependencies.".to_string()),
            ],
        ),
        // 7. Code generation
        (
            "Write a Python script that calculates fibonacci numbers",
            vec![
                MockLlmResponse::ToolUse {
                    name: "write_file".to_string(),
                    input: json!({
                        "path": "fibonacci.py",
                        "content": "def fib(n):\n    if n <= 1: return n\n    return fib(n-1) + fib(n-2)\n\nfor i in range(10): print(fib(i))"
                    }),
                },
                MockLlmResponse::Text("Created fibonacci.py with a recursive implementation.".to_string()),
            ],
        ),
        // 8. Explanation (no tools)
        (
            "Explain the difference between TCP and UDP",
            vec![MockLlmResponse::Text(
                "TCP is connection-oriented and reliable, while UDP is connectionless and faster but unreliable."
                    .to_string(),
            )],
        ),
        // 9. Parallel operations
        (
            "Read both config.yaml and .env files",
            vec![
                MockLlmResponse::MultiToolUse(vec![
                    ToolUseBlock::new("read_file", json!({"path": "config.yaml"})),
                    ToolUseBlock::new("read_file", json!({"path": ".env"})),
                ]),
                MockLlmResponse::Text("Both configuration files loaded.".to_string()),
            ],
        ),
        // 10. Error recovery scenario
        (
            "Run the test suite",
            vec![
                MockLlmResponse::ToolUse {
                    name: "bash".to_string(),
                    input: json!({"command": "cargo test"}),
                },
                MockLlmResponse::Text("Test suite completed. All 42 tests passed.".to_string()),
            ],
        ),
    ];

    let total_intents = intents.len();
    let mut successful_count = 0;
    let mut per_intent_durations = Vec::new();

    for (i, (prompt, responses)) in intents.into_iter().enumerate() {
        let scenario = ScenarioBuilder::new()
            .with_filesystem_tools()
            .with_bash_tool()
            .with_llm_responses(responses)
            .build()
            .await;

        let mut runner = scenario.agent_runner;

        let timer = BenchTimer::start(format!("intent_{}", i));
        let messages = collect_messages(&mut runner, prompt).await;
        let duration = timer.stop();
        per_intent_durations.push(duration);
        bench.add_sample(duration);

        // Verify the intent completed with a result message
        if has_result_message(&messages) {
            successful_count += 1;
        }

        // Verify text output is present
        let texts = extract_text_content(&messages);
        assert!(
            !texts.is_empty(),
            "Intent {} ('{}') should produce text output",
            i + 1,
            prompt
        );
    }

    // Verify all 10 intents completed successfully
    assert_eq!(
        successful_count, total_intents,
        "All {} intents should complete successfully, but only {} did",
        total_intents, successful_count
    );

    let avg_ms: f64 = per_intent_durations
        .iter()
        .map(|d| d.as_secs_f64() * 1000.0)
        .sum::<f64>()
        / total_intents as f64;

    bench.add_metric("total_intents", total_intents as f64);
    bench.add_metric("successful_intents", successful_count as f64);
    bench.add_metric("avg_ms", avg_ms);
    for (i, d) in per_intent_durations.iter().enumerate() {
        bench.add_metric(format!("per_intent_{}_ms", i), d.as_secs_f64() * 1000.0);
    }
    print_bench_inline(&bench);
}
