//! Task planning via LLM Function Calling.
//!
//! Provides structured task decomposition using `create_plan` and `update_plan`
//! tool definitions with `ToolChoice::Tool` to force structured output.
//!
//! # Architecture
//!
//! - `TaskPlanner` calls `LlmRouter.route()` directly (not AgentRunner)
//! - Uses Function Calling format for reliable structured output
//! - Strong model (e.g., qwen-max) for planning, weaker model for execution
//! - Re-planning on step failure with analysis

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

use crate::llm::router::{
    ChatRequest, ChatResponse, ContentBlock, Message, ToolChoice, ToolDefinition,
};
use crate::llm::LlmRouter;

/// Timeout for LLM calls in the planner to prevent indefinite hangs.
const PLANNER_LLM_TIMEOUT: Duration = Duration::from_secs(60);

// ============================================================================
// Types
// ============================================================================

/// A single step in an execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step identifier.
    pub id: u32,
    /// Concrete action to perform.
    pub action: String,
    /// Which tool category this step uses.
    pub tool_category: ToolCategory,
    /// Dependency type for scheduling.
    pub dependency: StepDependency,
    /// What success looks like for this step.
    #[serde(default)]
    pub expected_output: Option<String>,
    /// Per-step agent specialization (e.g., "coder", "browser-expert").
    /// Subsumes Swarm-style per-agent routing within PlanExecute.
    #[serde(default)]
    pub executor_agent: Option<String>,
    /// Per-step model override (e.g., "qwen-max", "claude-opus").
    /// Subsumes Expert-style per-specialist model selection.
    #[serde(default)]
    pub executor_model: Option<String>,
    /// Whether the Judge should capture a screenshot and use vision model
    /// to evaluate this step's visual output (e.g., UI layout changes).
    #[serde(default)]
    pub requires_visual_verification: Option<bool>,
    /// Detailed PRD document for this step. Contains specific files to modify,
    /// exact changes, constraints, and acceptance criteria. This is the actual
    /// instruction passed to ClaudeCode (not the brief `action` field).
    #[serde(default)]
    pub prd_content: Option<String>,
    /// Execution routing for this step.
    ///
    /// - `claude_code`: Direct ClaudeCode CLI invocation. Use for autonomous coding
    ///   tasks (reading, writing, modifying code). `prd_content` MUST be provided.
    /// - `shell`: Direct shell command execution. `action` is the command.
    /// - `llm_agent`: Default LLM agent with tool calling loop.
    #[serde(default)]
    pub executor_type: Option<ExecutorType>,
}

/// Executor type for step execution routing.
///
/// Determines how a plan step is executed: via the standard LLM agent loop,
/// direct ClaudeCode CLI invocation, or a shell command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorType {
    /// Default: LLM agent with tool calling loop.
    LlmAgent,
    /// Direct ClaudeCode CLI invocation for autonomous coding tasks.
    ClaudeCode,
    /// Direct shell command execution (e.g., cargo test, npm build).
    Shell,
}

impl Default for ExecutorType {
    fn default() -> Self {
        Self::LlmAgent
    }
}

impl std::fmt::Display for ExecutorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutorType::LlmAgent => write!(f, "llm_agent"),
            ExecutorType::ClaudeCode => write!(f, "claude_code"),
            ExecutorType::Shell => write!(f, "shell"),
        }
    }
}

/// Tool category for step classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    /// Browser automation (navigate, click, type, screenshot).
    Browser,
    /// File operations (read, write, search).
    File,
    /// Shell/terminal commands.
    Shell,
    /// Code generation/modification.
    Code,
    /// Pure LLM reasoning (no tools).
    Llm,
    /// Web search.
    Search,
}

impl ToolCategory {
    /// Get the string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolCategory::Browser => "browser",
            ToolCategory::File => "file",
            ToolCategory::Shell => "shell",
            ToolCategory::Code => "code",
            ToolCategory::Llm => "llm",
            ToolCategory::Search => "search",
        }
    }
}

impl std::fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Step dependency type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepDependency {
    /// Must run after previous step.
    Sequential,
    /// Can run concurrently with other parallel steps.
    Parallel,
    /// No dependency (first step or independent).
    None,
}

/// A complete execution plan generated by the planner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    /// One-line summary of what this plan achieves.
    pub goal: String,
    /// Ordered list of steps.
    pub steps: Vec<PlanStep>,
    /// How to verify the overall task is complete.
    pub success_criteria: String,
}

/// Configuration for the task planner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerConfig {
    /// Model to use for planning (strong model).
    pub planner_model: String,
    /// Model to use for step execution (can be multimodal).
    pub executor_model: String,
    /// Temperature for planner (low for consistency).
    pub planner_temperature: f32,
    /// Max tokens for planner output.
    pub planner_max_tokens: u32,
    /// System prompt for the planner.
    pub system_prompt: String,
    /// Few-shot example: user message.
    pub few_shot_user: String,
    /// Few-shot example: tool call arguments (JSON string).
    pub few_shot_tool_call: String,
    /// System prompt template for the replanner.
    pub replanner_system_prompt: String,
    /// Maximum replan attempts before skipping a step.
    pub max_replan_attempts: u32,
    /// Optional knowledge context injected before planning (A39).
    ///
    /// Contains relevant knowledge entries and verified plans from past
    /// executions. Appended to the system prompt to inform the planner.
    #[serde(default)]
    pub knowledge_context: Option<String>,
    /// Optional tool summary injected into planner system prompt (A40).
    ///
    /// Built from the executor's available tools to give the planner
    /// awareness of specific tools like ClaudeCode, BashTool, etc.
    #[serde(default)]
    pub tool_summary: Option<String>,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            planner_model: "qwen-max".into(),
            executor_model: "qwen-vl-plus".into(),
            planner_temperature: 0.3,
            planner_max_tokens: 2000,
            system_prompt: DEFAULT_PLANNER_SYSTEM_PROMPT.into(),
            few_shot_user: DEFAULT_FEW_SHOT_USER.into(),
            few_shot_tool_call: DEFAULT_FEW_SHOT_TOOL_CALL.into(),
            replanner_system_prompt: DEFAULT_REPLANNER_PROMPT.into(),
            max_replan_attempts: 2,
            knowledge_context: None,
            tool_summary: None,
        }
    }
}

/// Progress event emitted during plan execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PlanProgressEvent {
    /// Plan was created successfully.
    PlanCreated {
        goal: String,
        total_steps: usize,
        steps: Vec<PlanStepPreview>,
    },
    /// A step started executing.
    StepStarted {
        step_id: u32,
        action: String,
        tool_category: String,
    },
    /// A step completed successfully.
    StepCompleted {
        step_id: u32,
        result_preview: String,
    },
    /// A step failed.
    StepFailed { step_id: u32, error: String },
    /// Replanning started due to a step failure.
    ReplanStarted { reason: String },
    /// Replanning completed with new steps.
    ReplanCompleted { new_steps: Vec<PlanStepPreview> },
    /// All steps completed.
    Complete {
        response: String,
        total_tokens: usize,
        steps_completed: usize,
        steps_failed: usize,
    },
    /// An error occurred during execution.
    Error { message: String },
}

/// Lightweight step preview for progress events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepPreview {
    pub id: u32,
    pub action: String,
}

// ============================================================================
// Default Prompts
// ============================================================================

const DEFAULT_PLANNER_SYSTEM_PROMPT: &str = r#"You are a task planner. Analyze the user's request and create a structured execution plan.

RULES:
1. Each step must be a single, atomic operation
2. Steps with dependency="sequential" MUST come after their prerequisite
3. Steps with dependency="parallel" can execute concurrently
4. Use the most specific tool_category for each step
5. Keep steps granular — one action per step, not "do everything"
6. For steps with tool_category="code", you MUST include a detailed `prd_content` field containing:
   - GOAL: What this step achieves
   - FILES: Specific file paths to modify (be exact)
   - CHANGES: What to change in each file (describe the diff)
   - CONSTRAINTS: What NOT to change, scope boundaries
   - ACCEPTANCE CRITERIA: How to verify success
   - CONTEXT: Relevant information from previous steps
   The `prd_content` is the actual instruction given to the code executor.
   The `action` field is just a short label for UI display.

TOOL CATEGORIES:
- browser: Navigate web pages, click elements, fill forms, take screenshots
- file: Read, write, search files on disk
- shell: Run terminal commands (git, npm, cargo, etc.)
- code: Generate or modify source code (includes ClaudeCode for complex multi-file changes)
- llm: Ask LLM for analysis, summarization, or content generation
- search: Web search for information

EXECUTOR TYPES — REQUIRED for every step:
- claude_code: MUST use for any step involving file creation, code writing, code modification, refactoring, or multi-file changes. This routes to ClaudeCode CLI for autonomous coding. Always set prd_content with detailed instructions (GOAL, FILES, CHANGES, CONSTRAINTS, ACCEPTANCE CRITERIA).
- shell: MUST use for steps that are a single terminal command (cargo test, cargo check, npm build, git operations). The action field IS the command to execute.
- llm_agent: Use ONLY for analysis, search, browser automation, question answering, or tasks that need the LLM tool-calling loop but do NOT involve writing code.

CRITICAL RULE: If a step involves creating, modifying, or reading code files, you MUST set executor_type to "claude_code". Do NOT leave it unset for coding tasks — unset defaults to llm_agent which is slower and less capable for coding.

If AVAILABLE TOOLS are listed below, prefer using the specific tool names in your plan steps.

You MUST call the create_plan tool with your plan. Do not respond with text."#;

const DEFAULT_FEW_SHOT_USER: &str = "在 SwiftUI 项目中给主按钮添加阴影效果，并写一个单元测试验证";

const DEFAULT_FEW_SHOT_TOOL_CALL: &str = r###"{"goal":"Add shadow effect to main button in SwiftUI and add unit test","steps":[{"id":1,"action":"Add shadow modifier to MainButton","tool_category":"code","dependency":"none","executor_type":"claude_code","expected_output":"MainButton has .shadow() modifier","prd_content":"GOAL: Add a shadow effect to the MainButton component in the SwiftUI app.\n\nFILES:\n- Views/Components/MainButton.swift: Add shadow modifier\n\nCHANGES:\nIn MainButton.swift, add .shadow(color: .black.opacity(0.15), radius: 4, x: 0, y: 2) to the Button view body chain.\n\nCONSTRAINTS:\n- Do NOT change button text, color, or padding\n- Do NOT modify any other views\n- Only add the shadow modifier\n\nACCEPTANCE CRITERIA:\n- MainButton.swift compiles without errors\n- Button view body includes .shadow() modifier\n- Shadow parameters: color=black@15%, radius=4, x=0, y=2"},{"id":2,"action":"Add unit test for shadow","tool_category":"code","dependency":"sequential","executor_type":"claude_code","expected_output":"Test verifies shadow is present","prd_content":"GOAL: Add a unit test that verifies the MainButton has a shadow modifier.\n\nFILES:\n- Tests/Components/MainButtonTests.swift: Add test case\n\nCHANGES:\nAdd a new test method testMainButtonHasShadow() that instantiates MainButton and verifies the shadow modifier is applied using ViewInspector.\n\nCONSTRAINTS:\n- Do NOT modify existing tests\n- Only add the new test method\n\nACCEPTANCE CRITERIA:\n- Test file compiles\n- swift test passes with the new test"},{"id":3,"action":"swift build","tool_category":"shell","dependency":"sequential","executor_type":"shell","expected_output":"Build succeeds with 0 errors"}],"success_criteria":"MainButton has shadow effect, unit test passes, project compiles cleanly"}"###;

const DEFAULT_REPLANNER_PROMPT: &str = r#"You are reviewing an execution plan that encountered an issue.

COMPLETED STEPS:
{completed_steps}

FAILED STEP:
{failed_step}

ERROR:
{error_message}

REMAINING STEPS:
{remaining_steps}

Analyze the failure and call update_plan with adjusted remaining steps.
Options:
1. Retry the failed step with a different approach
2. Add recovery steps before continuing
3. Remove the failed step if non-critical and adjust subsequent steps

IMPORTANT: For steps with tool_category="code", you MUST include a detailed `prd_content` field.
Include context from the failure analysis so the executor has full information for the retry."#;

// ============================================================================
// TaskPlanner
// ============================================================================

/// LLM-based task planner using Function Calling.
///
/// Calls `LlmRouter.route()` directly with `ToolChoice::Tool` to force
/// structured plan output. Does not use AgentRunner (no tool execution needed).
pub struct TaskPlanner {
    llm_router: Arc<LlmRouter>,
    config: PlannerConfig,
}

impl TaskPlanner {
    /// Create a new task planner.
    pub fn new(llm_router: Arc<LlmRouter>, config: PlannerConfig) -> Self {
        Self { llm_router, config }
    }

    /// Generate an execution plan for a user request.
    ///
    /// Uses `create_plan` Function Calling to get structured output.
    /// Falls back to text JSON parsing if tool_use is not returned.
    pub async fn plan(&self, user_request: &str) -> anyhow::Result<ExecutionPlan> {
        // Build system prompt with optional knowledge context (A39) and tool summary (A40)
        let mut system_prompt = if let Some(ref knowledge) = self.config.knowledge_context {
            format!("{}\n\n{}", self.config.system_prompt, knowledge)
        } else {
            self.config.system_prompt.clone()
        };

        // Inject tool summary so planner knows about specific tools (A40)
        if let Some(ref tools) = self.config.tool_summary {
            system_prompt = format!("{}\n\n{}", system_prompt, tools);
        }

        let mut messages = vec![Message::text("system", &system_prompt)];

        // Add few-shot example if configured
        if !self.config.few_shot_user.is_empty() {
            messages.push(Message::text("user", &self.config.few_shot_user));
            // Simulate assistant tool_use response for few-shot
            messages.push(Message::with_blocks(
                "assistant",
                vec![ContentBlock::ToolUse {
                    id: "example_1".into(),
                    name: "create_plan".into(),
                    input: serde_json::from_str(&self.config.few_shot_tool_call)
                        .unwrap_or_default(),
                }],
            ));
            // Tool result acknowledging the example
            messages.push(Message::with_blocks(
                "user",
                vec![ContentBlock::ToolResult {
                    tool_use_id: "example_1".into(),
                    content: "Plan accepted.".into(),
                    is_error: false,
                }],
            ));
        }

        messages.push(Message::text("user", user_request));

        let request = ChatRequest {
            messages,
            model: Some(self.config.planner_model.clone()),
            temperature: Some(self.config.planner_temperature),
            max_tokens: Some(self.config.planner_max_tokens),
            tools: vec![Self::create_plan_tool_def()],
            tool_choice: Some(ToolChoice::Tool {
                name: "create_plan".into(),
            }),
            ..Default::default()
        };

        let response = tokio::time::timeout(PLANNER_LLM_TIMEOUT, self.llm_router.route(request))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "planner LLM call timed out after {}s",
                    PLANNER_LLM_TIMEOUT.as_secs()
                )
            })??;
        Self::parse_plan_response(&response)
    }

    /// Re-plan after a step failure.
    ///
    /// Uses `update_plan` Function Calling to get adjusted remaining steps.
    pub async fn replan(
        &self,
        completed: &[PlanStep],
        failed_step: &PlanStep,
        error: &str,
        remaining: &[PlanStep],
    ) -> anyhow::Result<Vec<PlanStep>> {
        let system = self
            .config
            .replanner_system_prompt
            .replace(
                "{completed_steps}",
                &serde_json::to_string_pretty(completed).unwrap_or_default(),
            )
            .replace(
                "{failed_step}",
                &serde_json::to_string_pretty(failed_step).unwrap_or_default(),
            )
            .replace("{error_message}", error)
            .replace(
                "{remaining_steps}",
                &serde_json::to_string_pretty(remaining).unwrap_or_default(),
            );

        let request = ChatRequest {
            messages: vec![
                Message::text("system", &system),
                Message::text("user", "Analyze the failure and create an updated plan."),
            ],
            model: Some(self.config.planner_model.clone()),
            temperature: Some(self.config.planner_temperature),
            max_tokens: Some(self.config.planner_max_tokens),
            tools: vec![Self::update_plan_tool_def()],
            tool_choice: Some(ToolChoice::Tool {
                name: "update_plan".into(),
            }),
            ..Default::default()
        };

        let response = tokio::time::timeout(PLANNER_LLM_TIMEOUT, self.llm_router.route(request))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "replanner LLM call timed out after {}s",
                    PLANNER_LLM_TIMEOUT.as_secs()
                )
            })??;
        Self::parse_replan_response(&response)
    }

    /// Parse a plan from a ChatResponse (tool_use or text fallback).
    pub fn parse_plan_response(response: &ChatResponse) -> anyhow::Result<ExecutionPlan> {
        if let Some(choice) = response.choices.first() {
            // Try to find create_plan tool_use in content_blocks
            for block in &choice.message.content_blocks {
                if let ContentBlock::ToolUse { name, input, .. } = block {
                    if name == "create_plan" {
                        return serde_json::from_value::<ExecutionPlan>(input.clone())
                            .map_err(|e| anyhow::anyhow!("Parse plan failed: {}", e));
                    }
                }
            }

            // Fallback: try to parse JSON from text content
            let text = &choice.message.content;
            if let Some(json_start) = text.find('{') {
                if let Some(json_end) = text.rfind('}') {
                    if json_start < json_end {
                        return serde_json::from_str::<ExecutionPlan>(&text[json_start..=json_end])
                            .map_err(|e| anyhow::anyhow!("Text parse failed: {}", e));
                    }
                }
            }
        }

        Err(anyhow::anyhow!("No plan found in LLM response"))
    }

    /// Parse replan response to extract updated steps.
    pub fn parse_replan_response(response: &ChatResponse) -> anyhow::Result<Vec<PlanStep>> {
        if let Some(choice) = response.choices.first() {
            for block in &choice.message.content_blocks {
                if let ContentBlock::ToolUse { name, input, .. } = block {
                    if name == "update_plan" {
                        // Extract remaining_steps from the update_plan input
                        if let Some(steps) = input.get("remaining_steps") {
                            return serde_json::from_value::<Vec<PlanStep>>(steps.clone())
                                .map_err(|e| anyhow::anyhow!("Parse replan steps failed: {}", e));
                        }
                    }
                }
            }

            // Fallback: try text JSON
            let text = &choice.message.content;
            if let Some(json_start) = text.find('{') {
                if let Some(json_end) = text.rfind('}') {
                    if json_start < json_end {
                        #[derive(Deserialize)]
                        struct UpdatePlan {
                            remaining_steps: Vec<PlanStep>,
                        }
                        if let Ok(update) =
                            serde_json::from_str::<UpdatePlan>(&text[json_start..=json_end])
                        {
                            return Ok(update.remaining_steps);
                        }
                    }
                }
            }
        }

        Err(anyhow::anyhow!("No replan found in LLM response"))
    }

    /// Build the `create_plan` tool definition.
    pub fn create_plan_tool_def() -> ToolDefinition {
        ToolDefinition {
            name: "create_plan".into(),
            description: "Create a structured execution plan for a user task".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "goal": {
                        "type": "string",
                        "description": "One-line summary of what this plan achieves"
                    },
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "integer" },
                                "action": {
                                    "type": "string",
                                    "description": "Concrete action to perform"
                                },
                                "tool_category": {
                                    "type": "string",
                                    "enum": ["browser", "file", "shell", "code", "llm", "search"],
                                    "description": "Which tool category this step uses"
                                },
                                "dependency": {
                                    "type": "string",
                                    "enum": ["sequential", "parallel", "none"],
                                    "description": "sequential=needs prev step, parallel=can run concurrently"
                                },
                                "expected_output": {
                                    "type": "string",
                                    "description": "What success looks like for this step"
                                },
                                "executor_agent": {
                                    "type": "string",
                                    "description": "Optional agent specialization for this step (e.g. 'coder', 'browser-expert')"
                                },
                                "executor_model": {
                                    "type": "string",
                                    "description": "Optional model override for this step (e.g. 'qwen-max', 'claude-opus')"
                                },
                                "requires_visual_verification": {
                                    "type": "boolean",
                                    "description": "Set to true if this step modifies UI and needs screenshot-based visual verification by the judge (e.g. layout changes, styling, component rendering)"
                                },
                                "prd_content": {
                                    "type": "string",
                                    "description": "Detailed execution document for this step. For code steps, MUST include: (1) GOAL — what this step achieves, (2) FILES — specific file paths to modify, (3) CHANGES — what to change in each file, (4) CONSTRAINTS — scope boundaries, (5) ACCEPTANCE CRITERIA — how to verify success. This document is passed directly to ClaudeCode as its instruction."
                                },
                                "executor_type": {
                                    "type": "string",
                                    "enum": ["llm_agent", "claude_code", "shell"],
                                    "description": "Execution routing: 'claude_code' for autonomous coding tasks (read/write/modify code, create modules) — prd_content MUST be provided with detailed instructions; 'shell' for direct commands (cargo test, npm build, git operations) — action field is the command; 'llm_agent' (default) for analysis, search, browser automation, or simple tool-calling tasks."
                                }
                            },
                            "required": ["id", "action", "tool_category", "dependency"]
                        }
                    },
                    "success_criteria": {
                        "type": "string",
                        "description": "How to verify the overall task is complete"
                    }
                },
                "required": ["goal", "steps", "success_criteria"]
            }),
        }
    }

    /// Build the `update_plan` tool definition.
    pub fn update_plan_tool_def() -> ToolDefinition {
        ToolDefinition {
            name: "update_plan".into(),
            description: "Adjust remaining plan steps after a step failure or new information"
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "analysis": {
                        "type": "string",
                        "description": "Brief analysis of why the step failed and what to adjust"
                    },
                    "remaining_steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "integer" },
                                "action": {
                                    "type": "string",
                                    "description": "Concrete action to perform"
                                },
                                "tool_category": {
                                    "type": "string",
                                    "enum": ["browser", "file", "shell", "code", "llm", "search"]
                                },
                                "dependency": {
                                    "type": "string",
                                    "enum": ["sequential", "parallel", "none"]
                                },
                                "expected_output": {
                                    "type": "string"
                                },
                                "executor_agent": {
                                    "type": "string",
                                    "description": "Optional agent specialization for this step"
                                },
                                "executor_model": {
                                    "type": "string",
                                    "description": "Optional model override for this step"
                                },
                                "requires_visual_verification": {
                                    "type": "boolean",
                                    "description": "Set to true if this step modifies UI and needs screenshot-based visual verification"
                                },
                                "prd_content": {
                                    "type": "string",
                                    "description": "Detailed execution document for this step. For code steps, MUST include: GOAL, FILES, CHANGES, CONSTRAINTS, ACCEPTANCE CRITERIA. Include context from the failure analysis for retried steps."
                                },
                                "executor_type": {
                                    "type": "string",
                                    "enum": ["llm_agent", "claude_code", "shell"],
                                    "description": "Execution routing: 'claude_code' for autonomous coding, 'shell' for direct commands, 'llm_agent' (default) for agent loop."
                                }
                            },
                            "required": ["id", "action", "tool_category", "dependency"]
                        }
                    }
                },
                "required": ["analysis", "remaining_steps"]
            }),
        }
    }

    /// Build a tool summary string from available tools.
    pub fn build_tool_summary(tools: &[ToolDefinition]) -> String {
        let mut summary = String::from("AVAILABLE TOOLS:\n");
        for tool in tools {
            summary.push_str(&format!("- {}: {}\n", tool.name, tool.description));
        }
        summary
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_planner_config_default() {
        let config = PlannerConfig::default();
        assert_eq!(config.planner_model, "qwen-max");
        assert_eq!(config.executor_model, "qwen-vl-plus");
        assert!((config.planner_temperature - 0.3).abs() < f32::EPSILON);
        assert_eq!(config.max_replan_attempts, 2);
        assert!(!config.system_prompt.is_empty());
    }

    #[test]
    fn test_execution_plan_serde_roundtrip() {
        let plan = ExecutionPlan {
            goal: "Test goal".into(),
            steps: vec![PlanStep {
                id: 1,
                action: "Do something".into(),
                tool_category: ToolCategory::Browser,
                dependency: StepDependency::Sequential,
                expected_output: Some("success".into()),
                executor_agent: None,
                executor_model: None,
                requires_visual_verification: None,
                prd_content: None,
                executor_type: None,
            }],
            success_criteria: "All done".into(),
        };
        let json = serde_json::to_string(&plan).unwrap();
        let parsed: ExecutionPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.goal, "Test goal");
        assert_eq!(parsed.steps.len(), 1);
        assert_eq!(parsed.steps[0].tool_category, ToolCategory::Browser);
    }

    #[test]
    fn test_plan_step_serde_all_categories() {
        for category in [
            ToolCategory::Browser,
            ToolCategory::File,
            ToolCategory::Shell,
            ToolCategory::Code,
            ToolCategory::Llm,
            ToolCategory::Search,
        ] {
            let step = PlanStep {
                id: 1,
                action: "test".into(),
                tool_category: category,
                dependency: StepDependency::None,
                expected_output: None,
                executor_agent: None,
                executor_model: None,
                requires_visual_verification: None,
                prd_content: None,
                executor_type: None,
            };
            let json = serde_json::to_string(&step).unwrap();
            let parsed: PlanStep = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.tool_category, category);
        }
    }

    #[test]
    fn test_step_dependency_serde() {
        for dep in [
            StepDependency::Sequential,
            StepDependency::Parallel,
            StepDependency::None,
        ] {
            let json = serde_json::to_string(&dep).unwrap();
            let parsed: StepDependency = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, dep);
        }
    }

    #[test]
    fn test_tool_category_as_str() {
        assert_eq!(ToolCategory::Browser.as_str(), "browser");
        assert_eq!(ToolCategory::File.as_str(), "file");
        assert_eq!(ToolCategory::Shell.as_str(), "shell");
        assert_eq!(ToolCategory::Code.as_str(), "code");
        assert_eq!(ToolCategory::Llm.as_str(), "llm");
        assert_eq!(ToolCategory::Search.as_str(), "search");
    }

    #[test]
    fn test_create_plan_tool_def_valid() {
        let tool = TaskPlanner::create_plan_tool_def();
        assert_eq!(tool.name, "create_plan");
        let schema_str = serde_json::to_string(&tool.input_schema).unwrap();
        assert!(schema_str.contains("goal"));
        assert!(schema_str.contains("steps"));
        assert!(schema_str.contains("success_criteria"));
    }

    #[test]
    fn test_update_plan_tool_def_valid() {
        let tool = TaskPlanner::update_plan_tool_def();
        assert_eq!(tool.name, "update_plan");
        let schema_str = serde_json::to_string(&tool.input_schema).unwrap();
        assert!(schema_str.contains("analysis"));
        assert!(schema_str.contains("remaining_steps"));
    }

    #[test]
    fn test_parse_plan_from_tool_use() {
        let response = ChatResponse {
            id: "test".into(),
            model: "test".into(),
            choices: vec![crate::llm::router::Choice {
                index: 0,
                message: Message::with_blocks(
                    "assistant",
                    vec![ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "create_plan".into(),
                        input: serde_json::json!({
                            "goal": "Send email",
                            "steps": [
                                {"id": 1, "action": "Open Gmail", "tool_category": "browser", "dependency": "none"},
                                {"id": 2, "action": "Click compose", "tool_category": "browser", "dependency": "sequential"}
                            ],
                            "success_criteria": "Email sent"
                        }),
                    }],
                ),
                finish_reason: "tool_use".into(),
                stop_reason: None,
            }],
            usage: crate::llm::router::Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
        };
        let plan = TaskPlanner::parse_plan_response(&response).unwrap();
        assert_eq!(plan.goal, "Send email");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].tool_category, ToolCategory::Browser);
    }

    #[test]
    fn test_parse_plan_from_text_json_fallback() {
        let response = ChatResponse {
            id: "test".into(),
            model: "test".into(),
            choices: vec![crate::llm::router::Choice {
                index: 0,
                message: Message::text(
                    "assistant",
                    r#"{"goal":"Test","steps":[],"success_criteria":"done"}"#,
                ),
                finish_reason: "end_turn".into(),
                stop_reason: None,
            }],
            usage: crate::llm::router::Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
        };
        let plan = TaskPlanner::parse_plan_response(&response).unwrap();
        assert_eq!(plan.goal, "Test");
        assert!(plan.steps.is_empty());
    }

    #[test]
    fn test_parse_plan_invalid_format_returns_error() {
        let response = ChatResponse {
            id: "test".into(),
            model: "test".into(),
            choices: vec![crate::llm::router::Choice {
                index: 0,
                message: Message::text("assistant", "I don't know how to plan this"),
                finish_reason: "end_turn".into(),
                stop_reason: None,
            }],
            usage: crate::llm::router::Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
        };
        assert!(TaskPlanner::parse_plan_response(&response).is_err());
    }

    #[test]
    fn test_parse_replan_from_tool_use() {
        let response = ChatResponse {
            id: "test".into(),
            model: "test".into(),
            choices: vec![crate::llm::router::Choice {
                index: 0,
                message: Message::with_blocks(
                    "assistant",
                    vec![ContentBlock::ToolUse {
                        id: "call_1".into(),
                        name: "update_plan".into(),
                        input: serde_json::json!({
                            "analysis": "Step 2 failed because button not found",
                            "remaining_steps": [
                                {"id": 3, "action": "Try alternative approach", "tool_category": "browser", "dependency": "none"}
                            ]
                        }),
                    }],
                ),
                finish_reason: "tool_use".into(),
                stop_reason: None,
            }],
            usage: crate::llm::router::Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
        };
        let steps = TaskPlanner::parse_replan_response(&response).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].id, 3);
    }

    #[test]
    fn test_plan_progress_event_serde_all_variants() {
        let events: Vec<PlanProgressEvent> = vec![
            PlanProgressEvent::PlanCreated {
                goal: "g".into(),
                total_steps: 1,
                steps: vec![PlanStepPreview {
                    id: 1,
                    action: "a".into(),
                }],
            },
            PlanProgressEvent::StepStarted {
                step_id: 1,
                action: "a".into(),
                tool_category: "browser".into(),
            },
            PlanProgressEvent::StepCompleted {
                step_id: 1,
                result_preview: "ok".into(),
            },
            PlanProgressEvent::StepFailed {
                step_id: 1,
                error: "err".into(),
            },
            PlanProgressEvent::ReplanStarted { reason: "r".into() },
            PlanProgressEvent::ReplanCompleted { new_steps: vec![] },
            PlanProgressEvent::Complete {
                response: "done".into(),
                total_tokens: 100,
                steps_completed: 3,
                steps_failed: 0,
            },
            PlanProgressEvent::Error {
                message: "fail".into(),
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            assert!(!json.is_empty());
        }
    }

    #[test]
    fn test_build_tool_summary_format() {
        let tools = vec![
            ToolDefinition {
                name: "computer_screenshot".into(),
                description: "Take a screenshot".into(),
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "computer_click_at".into(),
                description: "Click at coordinates".into(),
                input_schema: serde_json::json!({}),
            },
        ];
        let summary = TaskPlanner::build_tool_summary(&tools);
        assert!(summary.contains("AVAILABLE TOOLS:"));
        assert!(summary.contains("- computer_screenshot: Take a screenshot"));
        assert!(summary.contains("- computer_click_at: Click at coordinates"));
    }

    #[test]
    fn test_planner_config_tool_summary() {
        let mut config = PlannerConfig::default();
        assert!(config.tool_summary.is_none());

        config.tool_summary = Some(
            "AVAILABLE TOOLS:\n- ClaudeCode: AI-powered coding\n- BashTool: Run shell commands\n"
                .into(),
        );
        assert!(config.tool_summary.as_ref().unwrap().contains("ClaudeCode"));

        // Verify serde roundtrip
        let json = serde_json::to_string(&config).unwrap();
        let parsed: PlannerConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.tool_summary.as_ref().unwrap().contains("BashTool"));
    }

    #[test]
    fn test_planner_config_from_yaml() {
        let yaml = r#"
planner_model: "test-model"
executor_model: "test-exec"
planner_temperature: 0.5
planner_max_tokens: 1000
max_replan_attempts: 3
system_prompt: "You are a planner"
few_shot_user: "test prompt"
few_shot_tool_call: "{}"
replanner_system_prompt: "You are a replanner"
"#;
        let config: PlannerConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.planner_model, "test-model");
        assert_eq!(config.max_replan_attempts, 3);
    }

    #[test]
    fn test_executor_type_serde_roundtrip() {
        for et in [
            ExecutorType::LlmAgent,
            ExecutorType::ClaudeCode,
            ExecutorType::Shell,
        ] {
            let json = serde_json::to_string(&et).unwrap();
            let parsed: ExecutorType = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, et);
        }
    }

    #[test]
    fn test_executor_type_default() {
        assert_eq!(ExecutorType::default(), ExecutorType::LlmAgent);
    }

    #[test]
    fn test_executor_type_display() {
        assert_eq!(ExecutorType::LlmAgent.to_string(), "llm_agent");
        assert_eq!(ExecutorType::ClaudeCode.to_string(), "claude_code");
        assert_eq!(ExecutorType::Shell.to_string(), "shell");
    }

    #[test]
    fn test_plan_step_with_executor_type() {
        let step = PlanStep {
            id: 1,
            action: "Implement auth module".into(),
            tool_category: ToolCategory::Code,
            dependency: StepDependency::None,
            expected_output: Some("Auth module compiles".into()),
            executor_agent: None,
            executor_model: None,
            requires_visual_verification: None,
            prd_content: Some("GOAL: Create auth module\nFILES: src/auth.rs".into()),
            executor_type: Some(ExecutorType::ClaudeCode),
        };
        let json = serde_json::to_string(&step).unwrap();
        assert!(json.contains("claude_code"));
        let parsed: PlanStep = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.executor_type, Some(ExecutorType::ClaudeCode));
    }

    #[test]
    fn test_plan_step_executor_type_defaults_to_none() {
        let json = r#"{"id":1,"action":"test","tool_category":"code","dependency":"none"}"#;
        let step: PlanStep = serde_json::from_str(json).unwrap();
        assert!(step.executor_type.is_none());
    }

    #[test]
    fn test_create_plan_schema_includes_executor_type() {
        let tool = TaskPlanner::create_plan_tool_def();
        let schema_str = serde_json::to_string(&tool.input_schema).unwrap();
        assert!(schema_str.contains("executor_type"));
        assert!(schema_str.contains("claude_code"));
        assert!(schema_str.contains("llm_agent"));
        assert!(schema_str.contains("shell"));
    }

    #[test]
    fn test_update_plan_schema_includes_executor_type() {
        let tool = TaskPlanner::update_plan_tool_def();
        let schema_str = serde_json::to_string(&tool.input_schema).unwrap();
        assert!(schema_str.contains("executor_type"));
    }
}
