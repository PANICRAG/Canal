//! Code Orchestration Tool - Enables programmatic tool calling from agentic loop
//!
//! This tool allows an AgentRunner to execute LLM-generated code (Python/JS)
//! that programmatically orchestrates multiple tool calls with loops,
//! conditionals, and error handling. Code runs in a Docker sandbox with
//! an HTTP proxy bridge back to the tool registry.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::context::ToolContext;
use super::traits::{AgentTool, ToolError, ToolResult};
use crate::agent::code_orchestration::runtime::CodeOrchestrationRuntime;
use crate::agent::code_orchestration::types::CodeOrchestrationRequest;

/// Input for the CodeOrchestration tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeOrchestrationInput {
    /// The code to execute
    pub code: String,
    /// Programming language: "python" or "javascript"
    #[serde(default = "default_language")]
    pub language: String,
    /// Optional context data to inject (available as `context` variable)
    #[serde(default)]
    pub context_data: Option<serde_json::Value>,
    /// Execution timeout in milliseconds
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

fn default_language() -> String {
    "python".to_string()
}

/// Output from the CodeOrchestration tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeOrchestrationOutput {
    /// Whether code executed successfully
    pub success: bool,
    /// Return value parsed from stdout (if JSON)
    pub return_value: Option<serde_json::Value>,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Exit code
    pub exit_code: i32,
    /// Number of tool calls made
    pub tool_calls_count: usize,
    /// Execution duration in ms
    pub duration_ms: u64,
    /// Error message if any
    pub error: Option<String>,
}

/// Tool for executing code that programmatically calls other tools
pub struct CodeOrchestrationTool {
    runtime: Arc<CodeOrchestrationRuntime>,
}

impl CodeOrchestrationTool {
    /// Create a new CodeOrchestrationTool
    pub fn new(runtime: Arc<CodeOrchestrationRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl AgentTool for CodeOrchestrationTool {
    type Input = CodeOrchestrationInput;
    type Output = CodeOrchestrationOutput;

    fn name(&self) -> &str {
        "CodeOrchestration"
    }

    fn description(&self) -> &str {
        "Execute Python or JavaScript code that programmatically orchestrates tool calls. \
         The code runs in a sandbox and has access to a `tools` object with methods: \
         tools.read(path), tools.write(path, content), tools.edit(path, old, new), \
         tools.bash(command), tools.glob(pattern), tools.grep(pattern), tools.mcp(server, tool, **kwargs). \
         A `context` variable is also available with any injected data. \
         Use this for tasks requiring loops, conditionals, or complex multi-tool orchestration."
    }

    fn namespace(&self) -> &str {
        "code_orchestration"
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["code"],
            "properties": {
                "code": {
                    "type": "string",
                    "description": "Python or JavaScript code to execute. Has access to `tools` object for calling tools and `context` for injected data."
                },
                "language": {
                    "type": "string",
                    "enum": ["python", "javascript"],
                    "default": "python",
                    "description": "Programming language for the code"
                },
                "context_data": {
                    "type": "object",
                    "description": "Optional context data available as `context` variable in the code"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Execution timeout in milliseconds (default: 300000)"
                }
            }
        })
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        let request = CodeOrchestrationRequest {
            code: input.code,
            language: input.language,
            context_refs: vec![],
            context_data: input.context_data.unwrap_or(serde_json::json!({})),
            timeout: input
                .timeout_ms
                .map(std::time::Duration::from_millis)
                .unwrap_or(std::time::Duration::from_secs(300)),
        };

        // Create a tool context for the proxy bridge
        let proxy_context = ToolContext::new(&context.session_id, &context.cwd);

        // Copy allowed directories from the parent context
        let proxy_context = if !context.allowed_directories.is_empty() {
            let mut ctx = proxy_context;
            for dir in &context.allowed_directories {
                ctx = ctx.with_allowed_directory(dir);
            }
            ctx
        } else {
            proxy_context
        };

        let result = self
            .runtime
            .execute(request, proxy_context)
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Code orchestration failed: {}", e)))?;

        Ok(CodeOrchestrationOutput {
            success: result.success,
            return_value: result.return_value,
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
            tool_calls_count: result.tool_calls.len(),
            duration_ms: result.duration_ms,
            error: result.error,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::code_orchestration::types::CodeOrchestrationConfig;
    use crate::agent::tools::ToolRegistry;

    #[test]
    fn test_code_orchestration_tool_metadata() {
        let registry = Arc::new(ToolRegistry::new());
        let config = CodeOrchestrationConfig::default();
        let runtime = Arc::new(CodeOrchestrationRuntime::new(registry, config));
        let tool = CodeOrchestrationTool::new(runtime);

        assert_eq!(tool.name(), "CodeOrchestration");
        assert_eq!(tool.namespace(), "code_orchestration");
        assert!(tool.requires_permission());

        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("code")));
    }

    #[test]
    fn test_input_schema_has_all_fields() {
        let registry = Arc::new(ToolRegistry::new());
        let config = CodeOrchestrationConfig::default();
        let runtime = Arc::new(CodeOrchestrationRuntime::new(registry, config));
        let tool = CodeOrchestrationTool::new(runtime);

        let schema = tool.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("code"));
        assert!(props.contains_key("language"));
        assert!(props.contains_key("context_data"));
        assert!(props.contains_key("timeout_ms"));
    }
}
