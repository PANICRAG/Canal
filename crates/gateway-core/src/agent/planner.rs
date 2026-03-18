//! Task planner for creating execution plans

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use super::intent::TaskType;
use super::worker::types::WorkerSpecJson;
use crate::error::Result;
use crate::llm::{ChatRequest, LlmRouter, Message};
use crate::mcp::McpGateway;

/// Task execution plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    pub id: Uuid,
    pub summary: String,
    pub steps: Vec<PlanStep>,
    pub requires_approval: bool,
    pub warnings: Vec<String>,
    pub estimated_duration: Option<String>,
}

/// Single step in a task plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: Uuid,
    pub order: u32,
    pub description: String,
    pub action: StepAction,
    #[serde(default)]
    pub depends_on: Vec<Uuid>,
}

/// Action to perform in a step
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StepAction {
    /// Generate content using LLM
    LlmGenerate {
        prompt_template: String,
        model: Option<String>,
    },
    /// Call an MCP tool
    McpCall {
        server: String,
        tool: String,
        args: serde_json::Value,
    },
    /// Call an HTTP API
    HttpCall {
        method: String,
        url: String,
        headers: Option<serde_json::Value>,
        body: Option<serde_json::Value>,
    },
    /// Transform/process data
    Transform {
        operation: String,
        input_ref: String,
        params: serde_json::Value,
    },
    /// Wait for user input/confirmation
    WaitForInput { prompt: String },
    /// Conditional branching
    Condition {
        expression: String,
        then_steps: Vec<Uuid>,
        else_steps: Vec<Uuid>,
    },

    /// Spawn multiple worker agents for parallel execution (Orchestrator-Worker pattern)
    SpawnWorkers {
        /// Worker specifications
        workers: Vec<WorkerSpecJson>,
        /// Whether to synthesize results from all workers
        #[serde(default = "default_synthesize_results")]
        synthesize_results: bool,
        /// Optional prompt for result synthesis
        #[serde(default)]
        synthesis_prompt: Option<String>,
    },

    /// Execute code that programmatically orchestrates tool calls
    CodeOrchestration {
        /// The code to execute
        code: String,
        /// Programming language ("python" or "javascript")
        #[serde(default = "default_code_language")]
        language: String,
        /// References to prior step outputs to inject as context
        #[serde(default)]
        context_refs: Vec<String>,
        /// Execution timeout in milliseconds
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
}

fn default_synthesize_results() -> bool {
    true
}

fn default_code_language() -> String {
    "python".to_string()
}

/// Task planner
pub struct TaskPlanner {
    llm_router: Arc<LlmRouter>,
    mcp_gateway: Arc<McpGateway>,
    /// Unified Tool System (preferred over mcp_gateway when available)
    tool_system: Option<Arc<crate::tool_system::ToolSystem>>,
}

impl TaskPlanner {
    /// Create a new task planner
    pub fn new(llm_router: Arc<LlmRouter>, mcp_gateway: Arc<McpGateway>) -> Self {
        Self {
            llm_router,
            mcp_gateway,
            tool_system: None,
        }
    }

    /// Set the unified tool system
    pub fn with_tool_system(mut self, tool_system: Arc<crate::tool_system::ToolSystem>) -> Self {
        self.tool_system = Some(tool_system);
        self
    }

    /// Create an execution plan for a task
    pub async fn create_plan(
        &self,
        message: &str,
        task_type: TaskType,
        context: Option<&str>,
    ) -> Result<TaskPlan> {
        // Get available tools (prefer ToolSystem)
        let tools_json = if let Some(ref ts) = self.tool_system {
            let entries = ts.list_tools().await;
            serde_json::to_string_pretty(&entries).unwrap_or_default()
        } else {
            let tools = self.mcp_gateway.get_tools().await;
            serde_json::to_string_pretty(&tools).unwrap_or_default()
        };

        let context_str = context.unwrap_or("No additional context");

        let prompt = format!(
            r#"Create an execution plan for the following task.

User request: "{message}"
Task type: {task_type:?}
Context: {context_str}

Available tools:
{tools_json}

Create a step-by-step plan. Each step should have:
- description: What this step does
- action: The action type and parameters

Action types:
1. llm_generate: Generate content with LLM
   - prompt_template: The prompt to use (can reference previous step outputs with {{{{step_N_output}}}})
2. mcp_call: Call an MCP tool
   - server: Server name
   - tool: Tool name
   - args: Tool arguments
3. transform: Process/transform data
   - operation: The operation (format, filter, merge, etc.)
   - input_ref: Reference to input data
   - params: Operation parameters
4. spawn_workers: Spawn multiple parallel worker agents (Orchestrator-Worker pattern)
   - workers: Array of worker specs, each with:
     - name: Worker name
     - prompt: Task prompt for the worker
     - agent_type: "Explore", "Bash", "Plan", or "Code" (optional)
     - model: Override model (optional, defaults to sonnet)
     - max_turns: Max iterations for the worker (optional)
     - allowed_tools: List of tool names the worker can use (optional)
     - depends_on: List of worker names this worker depends on (optional)
   - synthesize_results: Whether to combine all worker outputs (default true)
   - synthesis_prompt: Custom prompt for combining results (optional)
   Use this when the task can be parallelized into independent subtasks.
5. code_orchestration: Execute code that programmatically calls tools
   - code: Python or JavaScript code using the tools SDK
   - language: "python" or "javascript" (default "python")
   - context_refs: References to prior step outputs (available as `context` variable)
   - timeout_ms: Execution timeout in ms (optional)
   The code has access to a `tools` object with methods: read(path), bash(cmd), glob(pattern), grep(pattern), write(path, content), edit(path, old, new), mcp(server, tool, **kwargs).
   Use this when the task requires loops, conditionals, or complex orchestration of multiple tool calls.

Return JSON:
{{
    "summary": "Brief description of what the plan will do",
    "requires_approval": true/false,
    "warnings": ["Any warnings or considerations"],
    "estimated_duration": "X minutes",
    "steps": [
        {{
            "order": 1,
            "description": "Step description",
            "action": {{
                "type": "llm_generate|mcp_call|transform|spawn_workers|code_orchestration",
                ...action specific fields...
            }}
        }}
    ]
}}

Only return valid JSON."#
        );

        let request = ChatRequest {
            messages: vec![Message::text("user", prompt)],
            model: None,
            max_tokens: Some(2000),
            temperature: Some(0.3),
            stream: false,
            ..Default::default()
        };

        let response = self.llm_router.route(request).await?;

        let content = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        // Parse the plan
        let parsed: serde_json::Value =
            serde_json::from_str(&content).unwrap_or_else(|e| {
                // R1-M: Log warning on LLM parse failure instead of silently proceeding
                tracing::warn!(error = %e, content_len = content.len(), "Failed to parse LLM plan response as JSON — proceeding with empty plan");
                serde_json::json!({})
            });

        let plan = TaskPlan {
            id: Uuid::new_v4(),
            summary: parsed["summary"]
                .as_str()
                .unwrap_or("Task execution")
                .to_string(),
            requires_approval: parsed["requires_approval"].as_bool().unwrap_or(false)
                || Self::needs_approval(&task_type),
            warnings: parsed["warnings"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            estimated_duration: parsed["estimated_duration"].as_str().map(String::from),
            steps: Self::parse_steps(&parsed["steps"]),
        };

        Ok(plan)
    }

    /// Check if task type requires approval
    fn needs_approval(task_type: &TaskType) -> bool {
        matches!(
            task_type,
            TaskType::Publish | TaskType::Manage | TaskType::ExecuteWorkflow
        )
    }

    /// Parse steps from JSON
    fn parse_steps(steps_json: &serde_json::Value) -> Vec<PlanStep> {
        let steps_array = match steps_json.as_array() {
            Some(arr) => arr,
            None => return vec![],
        };

        steps_array
            .iter()
            .enumerate()
            .map(|(i, step)| {
                let action = Self::parse_action(&step["action"]);
                PlanStep {
                    id: Uuid::new_v4(),
                    order: step["order"].as_u64().unwrap_or(i as u64 + 1) as u32,
                    description: step["description"].as_str().unwrap_or("Step").to_string(),
                    action,
                    depends_on: vec![],
                }
            })
            .collect()
    }

    /// Parse action from JSON
    fn parse_action(action_json: &serde_json::Value) -> StepAction {
        let action_type = action_json["type"].as_str().unwrap_or("llm_generate");

        match action_type {
            "llm_generate" => StepAction::LlmGenerate {
                prompt_template: action_json["prompt_template"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                model: action_json["model"].as_str().map(String::from),
            },
            "mcp_call" => StepAction::McpCall {
                server: action_json["server"]
                    .as_str()
                    .unwrap_or("default")
                    .to_string(),
                tool: action_json["tool"].as_str().unwrap_or("").to_string(),
                args: action_json["args"].clone(),
            },
            "http_call" => StepAction::HttpCall {
                method: action_json["method"].as_str().unwrap_or("GET").to_string(),
                url: action_json["url"].as_str().unwrap_or("").to_string(),
                headers: Some(action_json["headers"].clone()),
                body: Some(action_json["body"].clone()),
            },
            "transform" => StepAction::Transform {
                operation: action_json["operation"]
                    .as_str()
                    .unwrap_or("identity")
                    .to_string(),
                input_ref: action_json["input_ref"].as_str().unwrap_or("").to_string(),
                params: action_json["params"].clone(),
            },
            "wait_for_input" => StepAction::WaitForInput {
                prompt: action_json["prompt"]
                    .as_str()
                    .unwrap_or("Please provide input")
                    .to_string(),
            },
            "spawn_workers" => {
                let workers: Vec<WorkerSpecJson> = action_json["workers"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|w| serde_json::from_value(w.clone()).ok())
                            .collect()
                    })
                    .unwrap_or_default();

                StepAction::SpawnWorkers {
                    workers,
                    synthesize_results: action_json["synthesize_results"].as_bool().unwrap_or(true),
                    synthesis_prompt: action_json["synthesis_prompt"].as_str().map(String::from),
                }
            }
            "code_orchestration" => StepAction::CodeOrchestration {
                code: action_json["code"].as_str().unwrap_or("").to_string(),
                language: action_json["language"]
                    .as_str()
                    .unwrap_or("python")
                    .to_string(),
                context_refs: action_json["context_refs"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                timeout_ms: action_json["timeout_ms"].as_u64(),
            },
            _ => StepAction::LlmGenerate {
                prompt_template: "Continue with the task".to_string(),
                model: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_approval() {
        assert!(TaskPlanner::needs_approval(&TaskType::Publish));
        assert!(TaskPlanner::needs_approval(&TaskType::Manage));
        assert!(!TaskPlanner::needs_approval(&TaskType::CreateContent));
        assert!(!TaskPlanner::needs_approval(&TaskType::Search));
    }

    #[test]
    fn test_parse_action_llm_generate() {
        let action_json = serde_json::json!({
            "type": "llm_generate",
            "prompt_template": "Write a script about {{topic}}"
        });

        let action = TaskPlanner::parse_action(&action_json);
        match action {
            StepAction::LlmGenerate {
                prompt_template, ..
            } => {
                assert!(prompt_template.contains("script"));
            }
            _ => panic!("Expected LlmGenerate action"),
        }
    }
}
