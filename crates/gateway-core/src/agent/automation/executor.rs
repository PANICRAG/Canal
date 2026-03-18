//! Script Executor - Layer 4 of the Five-Layer Automation Architecture
//!
//! Executes generated automation scripts locally with zero token consumption.
//! Business data never passes through the LLM.

use super::types::{ExecutionStats, GeneratedScript, ScriptType};
use crate::agent::hybrid::{ExecutionRequest, ExecutionResult, HybridExecutor, HybridRouter};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

// ============================================================================
// Error Types
// ============================================================================

#[derive(Error, Debug)]
pub enum ExecutorError {
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Script error: {0}")]
    ScriptError(String),

    #[error("Timeout after {0}ms")]
    Timeout(u64),

    #[error("Invalid script: {0}")]
    InvalidScript(String),

    #[error("Router unavailable: {0}")]
    RouterUnavailable(String),
}

// ============================================================================
// Execution Options
// ============================================================================

/// Options for script execution
#[derive(Debug, Clone)]
pub struct ExecutionOptions {
    /// Timeout for entire execution (milliseconds)
    pub timeout_ms: u64,
    /// Whether to run in parallel (for multiple data items)
    pub parallel: bool,
    /// Maximum parallel executions
    pub max_parallel: usize,
    /// Whether to continue on error
    pub continue_on_error: bool,
    /// Whether to capture screenshots on error
    pub screenshot_on_error: bool,
    /// Session ID for stateful execution
    pub session_id: Option<String>,
    /// Environment variables
    pub env_vars: std::collections::HashMap<String, String>,
}

impl Default for ExecutionOptions {
    fn default() -> Self {
        Self {
            timeout_ms: 300000, // 5 minutes
            parallel: false,
            max_parallel: 5,
            continue_on_error: true,
            screenshot_on_error: true,
            session_id: None,
            env_vars: std::collections::HashMap::new(),
        }
    }
}

// ============================================================================
// Execution Result
// ============================================================================

/// Result of a single item execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemExecutionResult {
    /// Item index
    pub index: usize,
    /// Whether execution succeeded
    pub success: bool,
    /// Output data
    pub output: Option<serde_json::Value>,
    /// Error message if failed
    pub error: Option<String>,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Batch execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchExecutionResult {
    /// Overall success (all items succeeded)
    pub success: bool,
    /// Individual results
    pub results: Vec<ItemExecutionResult>,
    /// Total items processed
    pub total_items: usize,
    /// Successful items
    pub successful_items: usize,
    /// Failed items
    pub failed_items: usize,
    /// Total duration in milliseconds
    pub total_duration_ms: u64,
    /// Statistics
    pub stats: ExecutionStats,
}

impl BatchExecutionResult {
    /// Create a new result
    pub fn new() -> Self {
        Self {
            success: true,
            results: Vec::new(),
            total_items: 0,
            successful_items: 0,
            failed_items: 0,
            total_duration_ms: 0,
            stats: ExecutionStats::default(),
        }
    }

    /// Add an item result
    pub fn add_result(&mut self, result: ItemExecutionResult) {
        if result.success {
            self.successful_items += 1;
        } else {
            self.failed_items += 1;
            self.success = false;
        }
        self.total_items += 1;
        self.results.push(result);
    }

    /// Finalize the result
    pub fn finalize(&mut self, duration_ms: u64) {
        self.total_duration_ms = duration_ms;
        self.stats.duration_ms = duration_ms;
        self.stats.items_processed = self.successful_items;
        self.stats.items_failed = self.failed_items;
        // Execution phase uses 0 tokens
        self.stats.execution_tokens = 0;
    }
}

impl Default for BatchExecutionResult {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Script Executor
// ============================================================================

/// Script Executor - Runs automation scripts locally
#[allow(dead_code)]
pub struct ScriptExecutor {
    /// Hybrid router for code execution
    hybrid_router: Option<Arc<HybridRouter>>,
    /// Configuration
    config: ScriptExecutorConfig,
}

/// Configuration for the script executor
#[derive(Debug, Clone)]
pub struct ScriptExecutorConfig {
    /// Default timeout (milliseconds)
    pub default_timeout_ms: u64,
    /// Default max parallel
    pub default_max_parallel: usize,
    /// Whether to enable logging
    pub enable_logging: bool,
    /// Working directory for script execution
    pub working_dir: Option<String>,
}

impl Default for ScriptExecutorConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 300000,
            default_max_parallel: 5,
            enable_logging: true,
            working_dir: None,
        }
    }
}

impl ScriptExecutor {
    /// Create a new script executor
    pub fn new() -> Self {
        Self {
            hybrid_router: None,
            config: ScriptExecutorConfig::default(),
        }
    }

    /// Create a builder
    pub fn builder() -> ScriptExecutorBuilder {
        ScriptExecutorBuilder::default()
    }

    /// Execute a script with data
    pub async fn execute(
        &self,
        script: &GeneratedScript,
        data: Vec<serde_json::Value>,
        options: ExecutionOptions,
    ) -> Result<BatchExecutionResult, ExecutorError> {
        let start = std::time::Instant::now();
        let mut batch_result = BatchExecutionResult::new();

        // Wrap the script with data handling
        let wrapped_code = self.wrap_script_with_data(script, &data)?;

        // Execute based on script type
        match script.script_type {
            ScriptType::Playwright | ScriptType::Puppeteer => {
                self.execute_javascript(&wrapped_code, &data, &options, &mut batch_result)
                    .await?;
            }
            ScriptType::Selenium | ScriptType::RestApi | ScriptType::GraphQl => {
                self.execute_python(&wrapped_code, &data, &options, &mut batch_result)
                    .await?;
            }
            ScriptType::Native => {
                self.execute_native(&wrapped_code, &data, &options, &mut batch_result)
                    .await?;
            }
        }

        batch_result.finalize(start.elapsed().as_millis() as u64);
        Ok(batch_result)
    }

    /// Wrap script with data injection
    fn wrap_script_with_data(
        &self,
        script: &GeneratedScript,
        data: &[serde_json::Value],
    ) -> Result<String, ExecutorError> {
        let data_json = serde_json::to_string(data).map_err(|e| {
            ExecutorError::InvalidScript(format!("Data serialization failed: {}", e))
        })?;

        match script.script_type {
            ScriptType::Playwright | ScriptType::Puppeteer => {
                // JavaScript: Inject data as a variable
                Ok(format!(
                    r#"
// Injected data
const __DATA__ = {};

// Original script
{}

// Execute with data
(async () => {{
    if (typeof processData === 'function') {{
        const results = await processData(__DATA__);
        console.log(JSON.stringify({{ success: true, results }}));
    }} else if (typeof main === 'function') {{
        await main(__DATA__);
    }}
}})().catch(e => console.log(JSON.stringify({{ success: false, error: e.message }})));
"#,
                    data_json, script.code
                ))
            }
            ScriptType::Selenium | ScriptType::RestApi | ScriptType::GraphQl => {
                // Python: Inject data as a variable
                Ok(format!(
                    r#"
import json
import sys

# Injected data
__DATA__ = {}

# Original script
{}

# Execute with data
if __name__ == '__main__':
    try:
        if 'process_data' in dir():
            results = process_data(__DATA__)
            print(json.dumps({{'success': True, 'results': results}}))
        elif 'main' in dir():
            main(__DATA__)
    except Exception as e:
        print(json.dumps({{'success': False, 'error': str(e)}}))
        sys.exit(1)
"#,
                    data_json, script.code
                ))
            }
            ScriptType::Native => {
                // Native: Just the script
                Ok(script.code.clone())
            }
        }
    }

    /// Execute JavaScript code via hybrid router
    async fn execute_javascript(
        &self,
        code: &str,
        data: &[serde_json::Value],
        options: &ExecutionOptions,
        result: &mut BatchExecutionResult,
    ) -> Result<(), ExecutorError> {
        if let Some(router) = &self.hybrid_router {
            let request =
                ExecutionRequest::code(code, "javascript").with_timeout(options.timeout_ms);

            let request = if let Some(ref session) = options.session_id {
                request.with_session(session)
            } else {
                request
            };

            match router.execute(request).await {
                Ok(exec_result) => {
                    self.parse_execution_output(&exec_result, data.len(), result);
                }
                Err(e) => {
                    // Mark all items as failed
                    for i in 0..data.len() {
                        result.add_result(ItemExecutionResult {
                            index: i,
                            success: false,
                            output: None,
                            error: Some(e.to_string()),
                            duration_ms: 0,
                        });
                    }
                }
            }
        } else {
            // Fallback: Direct execution (would need Node.js runtime)
            for i in 0..data.len() {
                result.add_result(ItemExecutionResult {
                    index: i,
                    success: false,
                    output: None,
                    error: Some("Hybrid router not configured".to_string()),
                    duration_ms: 0,
                });
            }
        }

        Ok(())
    }

    /// Execute Python code via hybrid router
    async fn execute_python(
        &self,
        code: &str,
        data: &[serde_json::Value],
        options: &ExecutionOptions,
        result: &mut BatchExecutionResult,
    ) -> Result<(), ExecutorError> {
        if let Some(router) = &self.hybrid_router {
            let request = ExecutionRequest::code(code, "python").with_timeout(options.timeout_ms);

            let request = if let Some(ref session) = options.session_id {
                request.with_session(session)
            } else {
                request
            };

            match router.execute(request).await {
                Ok(exec_result) => {
                    self.parse_execution_output(&exec_result, data.len(), result);
                }
                Err(e) => {
                    for i in 0..data.len() {
                        result.add_result(ItemExecutionResult {
                            index: i,
                            success: false,
                            output: None,
                            error: Some(e.to_string()),
                            duration_ms: 0,
                        });
                    }
                }
            }
        } else {
            for i in 0..data.len() {
                result.add_result(ItemExecutionResult {
                    index: i,
                    success: false,
                    output: None,
                    error: Some("Hybrid router not configured".to_string()),
                    duration_ms: 0,
                });
            }
        }

        Ok(())
    }

    /// Execute native script
    async fn execute_native(
        &self,
        _code: &str,
        data: &[serde_json::Value],
        _options: &ExecutionOptions,
        result: &mut BatchExecutionResult,
    ) -> Result<(), ExecutorError> {
        // Native execution would use std::process::Command
        // For now, mark as unsupported
        for i in 0..data.len() {
            result.add_result(ItemExecutionResult {
                index: i,
                success: false,
                output: None,
                error: Some("Native execution not yet implemented".to_string()),
                duration_ms: 0,
            });
        }

        Ok(())
    }

    /// Parse execution output into results
    fn parse_execution_output(
        &self,
        exec_result: &ExecutionResult,
        data_count: usize,
        batch_result: &mut BatchExecutionResult,
    ) {
        if exec_result.success {
            // Try to parse the output as JSON
            if let Some(ref output) = exec_result.output {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(output) {
                    // Check if it's our expected format
                    if let Some(results) = parsed.get("results").and_then(|r| r.as_array()) {
                        for (i, item_result) in results.iter().enumerate() {
                            let success = item_result
                                .get("success")
                                .and_then(|s| s.as_bool())
                                .unwrap_or(false);
                            let error = item_result
                                .get("error")
                                .and_then(|e| e.as_str())
                                .map(|s| s.to_string());

                            batch_result.add_result(ItemExecutionResult {
                                index: i,
                                success,
                                output: Some(item_result.clone()),
                                error,
                                duration_ms: exec_result.duration_ms / data_count as u64,
                            });
                        }
                        return;
                    }
                }
            }

            // If we can't parse individual results, mark all as successful
            for i in 0..data_count {
                batch_result.add_result(ItemExecutionResult {
                    index: i,
                    success: true,
                    output: exec_result.data.clone(),
                    error: None,
                    duration_ms: exec_result.duration_ms / data_count as u64,
                });
            }
        } else {
            // Execution failed, mark all as failed
            let error = exec_result
                .error
                .as_ref()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "Unknown error".to_string());

            for i in 0..data_count {
                batch_result.add_result(ItemExecutionResult {
                    index: i,
                    success: false,
                    output: None,
                    error: Some(error.clone()),
                    duration_ms: 0,
                });
            }
        }
    }
}

impl Default for ScriptExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Builder
// ============================================================================

/// Builder for ScriptExecutor
#[derive(Default)]
pub struct ScriptExecutorBuilder {
    hybrid_router: Option<Arc<HybridRouter>>,
    config: ScriptExecutorConfig,
}

impl ScriptExecutorBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set hybrid router
    pub fn hybrid_router(mut self, router: Arc<HybridRouter>) -> Self {
        self.hybrid_router = Some(router);
        self
    }

    /// Set default timeout
    pub fn default_timeout(mut self, timeout_ms: u64) -> Self {
        self.config.default_timeout_ms = timeout_ms;
        self
    }

    /// Set default max parallel
    pub fn default_max_parallel(mut self, max: usize) -> Self {
        self.config.default_max_parallel = max;
        self
    }

    /// Set working directory
    pub fn working_dir(mut self, dir: impl Into<String>) -> Self {
        self.config.working_dir = Some(dir.into());
        self
    }

    /// Enable/disable logging
    pub fn enable_logging(mut self, enable: bool) -> Self {
        self.config.enable_logging = enable;
        self
    }

    /// Build the executor
    pub fn build(self) -> ScriptExecutor {
        ScriptExecutor {
            hybrid_router: self.hybrid_router,
            config: self.config,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_options_default() {
        let options = ExecutionOptions::default();
        assert_eq!(options.timeout_ms, 300000);
        assert!(!options.parallel);
        assert!(options.continue_on_error);
    }

    #[test]
    fn test_batch_result_tracking() {
        let mut result = BatchExecutionResult::new();

        result.add_result(ItemExecutionResult {
            index: 0,
            success: true,
            output: None,
            error: None,
            duration_ms: 100,
        });

        result.add_result(ItemExecutionResult {
            index: 1,
            success: false,
            output: None,
            error: Some("Test error".to_string()),
            duration_ms: 50,
        });

        result.add_result(ItemExecutionResult {
            index: 2,
            success: true,
            output: None,
            error: None,
            duration_ms: 100,
        });

        result.finalize(250);

        assert_eq!(result.total_items, 3);
        assert_eq!(result.successful_items, 2);
        assert_eq!(result.failed_items, 1);
        assert!(!result.success); // At least one failure
        assert_eq!(result.stats.items_processed, 2);
        assert_eq!(result.stats.items_failed, 1);
        assert_eq!(result.stats.execution_tokens, 0); // No tokens for execution
    }

    #[test]
    fn test_wrap_script_javascript() {
        let executor = ScriptExecutor::new();
        let script = GeneratedScript::new(
            ScriptType::Playwright,
            "async function processData(data) { return data; }",
            "javascript",
            "hash",
            "sig",
        );

        let data = vec![serde_json::json!({"name": "test"})];
        let wrapped = executor.wrap_script_with_data(&script, &data).unwrap();

        assert!(wrapped.contains("__DATA__"));
        assert!(wrapped.contains("processData"));
    }

    #[test]
    fn test_wrap_script_python() {
        let executor = ScriptExecutor::new();
        let script = GeneratedScript::new(
            ScriptType::Selenium,
            "def process_data(data): return data",
            "python",
            "hash",
            "sig",
        );

        let data = vec![serde_json::json!({"name": "test"})];
        let wrapped = executor.wrap_script_with_data(&script, &data).unwrap();

        assert!(wrapped.contains("__DATA__"));
        assert!(wrapped.contains("process_data"));
        assert!(wrapped.contains("import json"));
    }
}
