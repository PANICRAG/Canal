//! Programmable Mock Tool Executor
//!
//! Provides a configurable tool executor for testing agent tool orchestration
//! without actual MCP servers or filesystem operations. Supports dynamic results,
//! failure injection, latency simulation, and concurrency tracking.

use async_trait::async_trait;
use gateway_core::agent::r#loop::runner::ToolExecutor;
use gateway_core::agent::tools::ToolContext;
use gateway_core::agent::AgentError;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// A recorded tool call
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub call_index: u64,
    pub tool_name: String,
    pub input: Value,
    pub result: Result<Value, String>,
    pub duration_ms: u64,
    pub timestamp: std::time::SystemTime,
}

/// Mock tool result types
#[derive(Clone)]
pub enum MockToolResult {
    /// Static success value
    Success(Value),
    /// Static error
    Error(String),
    /// Dynamic function-based result
    DynamicFn(Arc<dyn Fn(&Value) -> Value + Send + Sync>),
}

impl std::fmt::Debug for MockToolResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MockToolResult::Success(v) => write!(f, "Success({v})"),
            MockToolResult::Error(e) => write!(f, "Error({e})"),
            MockToolResult::DynamicFn(_) => write!(f, "DynamicFn(...)"),
        }
    }
}

/// Programmable mock tool executor
pub struct MockToolExecutor {
    /// Registered tool schemas (JSON)
    tool_schemas: Arc<RwLock<Vec<Value>>>,
    /// Tool name → result mapping
    tool_results: Arc<RwLock<HashMap<String, MockToolResult>>>,
    /// Tool name → latency in ms
    tool_latencies: Arc<RwLock<HashMap<String, u64>>>,
    /// Tool name → fail count (first N calls fail, then succeed)
    tool_fail_count: Arc<RwLock<HashMap<String, AtomicU64>>>,
    /// Tool name → remaining failures
    tool_remaining_failures: Arc<RwLock<HashMap<String, AtomicU64>>>,
    /// Call log for inspection
    call_log: Arc<Mutex<Vec<ToolCallRecord>>>,
    /// Global call counter
    call_count: Arc<AtomicU64>,
    /// Concurrent call counter
    concurrent_calls: Arc<AtomicU64>,
    /// Peak concurrent calls observed
    peak_concurrent: Arc<AtomicU64>,
}

impl MockToolExecutor {
    pub fn new() -> Self {
        Self {
            tool_schemas: Arc::new(RwLock::new(Vec::new())),
            tool_results: Arc::new(RwLock::new(HashMap::new())),
            tool_latencies: Arc::new(RwLock::new(HashMap::new())),
            tool_fail_count: Arc::new(RwLock::new(HashMap::new())),
            tool_remaining_failures: Arc::new(RwLock::new(HashMap::new())),
            call_log: Arc::new(Mutex::new(Vec::new())),
            call_count: Arc::new(AtomicU64::new(0)),
            concurrent_calls: Arc::new(AtomicU64::new(0)),
            peak_concurrent: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Register a tool with its schema and default result
    pub async fn register_tool(
        &self,
        name: &str,
        description: &str,
        input_schema: Value,
        result: MockToolResult,
    ) {
        let schema = serde_json::json!({
            "name": name,
            "description": description,
            "input_schema": input_schema,
        });
        self.tool_schemas.write().await.push(schema);
        self.tool_results
            .write()
            .await
            .insert(name.to_string(), result);
    }

    /// Set the result for an existing tool
    pub async fn set_result(&self, name: &str, result: MockToolResult) {
        self.tool_results
            .write()
            .await
            .insert(name.to_string(), result);
    }

    /// Set latency for a specific tool
    pub async fn set_latency(&self, name: &str, latency_ms: u64) {
        self.tool_latencies
            .write()
            .await
            .insert(name.to_string(), latency_ms);
    }

    /// Set the number of initial failures for a tool
    /// The first `count` calls will fail, then subsequent calls succeed
    pub async fn set_fail_count(&self, name: &str, count: u64) {
        self.tool_remaining_failures
            .write()
            .await
            .insert(name.to_string(), AtomicU64::new(count));
    }

    /// Get total call count
    pub fn call_count(&self) -> u64 {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Get peak concurrent calls observed
    pub fn peak_concurrent(&self) -> u64 {
        self.peak_concurrent.load(Ordering::SeqCst)
    }

    /// Get the full call log
    pub async fn call_log(&self) -> Vec<ToolCallRecord> {
        self.call_log.lock().await.clone()
    }

    /// Get calls for a specific tool
    pub async fn calls_for_tool(&self, name: &str) -> Vec<ToolCallRecord> {
        self.call_log
            .lock()
            .await
            .iter()
            .filter(|r| r.tool_name == name)
            .cloned()
            .collect()
    }

    /// Get the last call record
    pub async fn last_call(&self) -> Option<ToolCallRecord> {
        self.call_log.lock().await.last().cloned()
    }

    /// Reset all state
    pub async fn reset(&self) {
        self.call_log.lock().await.clear();
        self.call_count.store(0, Ordering::SeqCst);
        self.concurrent_calls.store(0, Ordering::SeqCst);
        self.peak_concurrent.store(0, Ordering::SeqCst);
    }

    /// Helper: register common filesystem tools
    pub async fn register_filesystem_tools(&self) {
        self.register_tool(
            "read_file",
            "Read the contents of a file",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
            MockToolResult::Success(serde_json::json!({"content": "file contents"})),
        )
        .await;

        self.register_tool(
            "write_file",
            "Write content to a file",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }),
            MockToolResult::Success(serde_json::json!({"success": true})),
        )
        .await;

        self.register_tool(
            "search_files",
            "Search for files matching a pattern",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"}
                },
                "required": ["pattern"]
            }),
            MockToolResult::Success(serde_json::json!({"files": ["main.rs", "lib.rs"]})),
        )
        .await;
    }

    /// Helper: register bash tool
    pub async fn register_bash_tool(&self) {
        self.register_tool(
            "bash",
            "Execute a shell command",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                },
                "required": ["command"]
            }),
            MockToolResult::Success(
                serde_json::json!({"output": "command output", "exit_code": 0}),
            ),
        )
        .await;
    }

    /// Helper: register a tool that returns dynamic content based on input
    pub async fn register_dynamic_tool(
        &self,
        name: &str,
        description: &str,
        handler: Arc<dyn Fn(&Value) -> Value + Send + Sync>,
    ) {
        self.register_tool(
            name,
            description,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": {"type": "string"}
                }
            }),
            MockToolResult::DynamicFn(handler),
        )
        .await;
    }
}

impl Default for MockToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolExecutor for MockToolExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        tool_input: Value,
        _context: &ToolContext,
    ) -> Result<Value, AgentError> {
        let call_index = self.call_count.fetch_add(1, Ordering::SeqCst);
        let start = std::time::Instant::now();

        // Track concurrency
        let current = self.concurrent_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut peak = self.peak_concurrent.load(Ordering::SeqCst);
        while current > peak {
            match self.peak_concurrent.compare_exchange_weak(
                peak,
                current,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => peak = actual,
            }
        }

        // Simulate latency
        if let Some(latency) = self.tool_latencies.read().await.get(tool_name) {
            tokio::time::sleep(std::time::Duration::from_millis(*latency)).await;
        }

        // Check for remaining failures
        let should_fail = {
            let failures = self.tool_remaining_failures.read().await;
            if let Some(remaining) = failures.get(tool_name) {
                let prev = remaining.fetch_sub(1, Ordering::SeqCst);
                prev > 0
            } else {
                false
            }
        };

        let result = if should_fail {
            Err(AgentError::ToolError(format!(
                "Injected failure for tool '{tool_name}'"
            )))
        } else {
            // Get the tool result
            let results = self.tool_results.read().await;
            match results.get(tool_name) {
                Some(MockToolResult::Success(v)) => Ok(v.clone()),
                Some(MockToolResult::Error(e)) => Err(AgentError::ToolError(e.clone())),
                Some(MockToolResult::DynamicFn(f)) => Ok(f(&tool_input)),
                None => Err(AgentError::ToolError(format!("Unknown tool: {tool_name}"))),
            }
        };

        // Decrement concurrency
        self.concurrent_calls.fetch_sub(1, Ordering::SeqCst);

        // Log the call
        let record = ToolCallRecord {
            call_index,
            tool_name: tool_name.to_string(),
            input: tool_input,
            result: result
                .as_ref()
                .map(|v| v.clone())
                .map_err(|e| e.to_string()),
            duration_ms: start.elapsed().as_millis() as u64,
            timestamp: std::time::SystemTime::now(),
        };
        self.call_log.lock().await.push(record);

        result
    }

    fn get_tool_schemas(&self) -> Vec<Value> {
        // Use try_read to avoid async in sync context
        // In test scenarios, this is always available
        let schemas = self.tool_schemas.try_read();
        match schemas {
            Ok(s) => s.clone(),
            Err(_) => Vec::new(),
        }
    }
}
