//! Task Tool - Subagent delegation with full lifecycle management
//!
//! This module provides the Task tool for delegating work to specialized subagents.
//! It includes both a placeholder implementation for testing and a real implementation
//! that uses actual `AgentRunner` instances.
//!
//! # Features
//! - Create subagents with custom configurations
//! - Support for different agent types (Sonnet, Opus, Haiku)
//! - Budget and max_turns control
//! - Tool inheritance from parent agent
//! - Hook events: SubagentSpawn, SubagentComplete

use super::{AgentTool, ToolContext, ToolError, ToolResult};
use crate::agent::hooks::HookExecutor;
use crate::agent::llm_adapter::LlmRouterAdapter;
use crate::agent::r#loop::{AgentConfig, AgentLoop, AgentRunner};
use crate::agent::tools::registry::ToolRegistry;
use crate::agent::types::{
    AgentMessage, HookContext, HookEvent, PermissionContext, PermissionMode,
    SubagentCompleteHookData, SubagentSpawnHookData, Usage,
};
use crate::llm::LlmRouter;
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use uuid::Uuid;

// ============================================================================
// Task Input/Output Types
// ============================================================================

/// Task tool input
#[derive(Debug, Clone, Deserialize)]
pub struct TaskInput {
    /// Short description of the task (3-5 words)
    pub description: String,
    /// The task prompt for the subagent
    pub prompt: String,
    /// Type of subagent to use (Explore, Bash, Plan, Code, or custom)
    pub subagent_type: String,
    /// Optional model override (sonnet, opus, haiku, or full model ID)
    #[serde(default)]
    pub model: Option<String>,
    /// Maximum turns for the subagent (default varies by type)
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Maximum budget in USD for this subagent
    #[serde(default)]
    pub max_budget_usd: Option<f64>,
    /// Tools to grant this agent (if None, inherits from parent)
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Tools to block for this agent
    #[serde(default)]
    pub blocked_tools: Option<Vec<String>>,
    /// Agent ID to resume (for continuing a previous agent)
    #[serde(default)]
    pub resume: Option<String>,
}

/// Task tool output
#[derive(Debug, Clone, Serialize)]
pub struct TaskOutput {
    /// Result from the subagent
    pub result: String,
    /// Agent ID (for resuming later or tracking)
    pub agent_id: String,
    /// Token usage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Total cost in USD
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Whether the agent is still running (background)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,
    /// Output file path (for background tasks)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
    /// Number of turns used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_turns: Option<u32>,
    /// Whether the agent hit a limit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_reached: Option<String>,
}

// ============================================================================
// Agent Factory and Subagent Traits
// ============================================================================

/// Agent factory trait for creating subagents
#[async_trait]
pub trait AgentFactory: Send + Sync {
    /// Create a new subagent
    async fn create(
        &self,
        agent_type: &str,
        config: SubagentConfig,
    ) -> Result<Box<dyn Subagent>, ToolError>;

    /// Resume a subagent
    async fn resume(&self, agent_id: &str) -> Result<Box<dyn Subagent>, ToolError>;

    /// List available agent types
    fn available_types(&self) -> Vec<AgentTypeInfo>;

    /// Fire SubagentSpawn hook
    async fn fire_spawn_hook(&self, hook_data: SubagentSpawnHookData, context: &HookContext);

    /// Fire SubagentComplete hook
    async fn fire_complete_hook(&self, hook_data: SubagentCompleteHookData, context: &HookContext);
}

/// Subagent trait
#[async_trait]
pub trait Subagent: Send + Sync {
    /// Get the agent ID
    fn id(&self) -> &str;

    /// Get the agent type
    fn agent_type(&self) -> &str;

    /// Run the agent with a prompt
    async fn run(&mut self, prompt: &str) -> Result<SubagentResult, ToolError>;

    /// Interrupt the agent
    async fn interrupt(&mut self) -> Result<(), ToolError>;

    /// Check if the agent is running
    fn is_running(&self) -> bool;

    /// Get current usage
    fn current_usage(&self) -> Option<Usage>;

    /// Get current cost
    fn current_cost(&self) -> Option<f64>;
}

/// Subagent configuration
#[derive(Clone)]
pub struct SubagentConfig {
    /// Model to use (sonnet, opus, haiku, or full model ID)
    pub model: Option<String>,
    /// Maximum turns
    pub max_turns: Option<u32>,
    /// Maximum budget in USD
    pub max_budget_usd: Option<f64>,
    /// Allowed tools (if None, uses type defaults)
    pub allowed_tools: Option<Vec<String>>,
    /// Blocked tools
    pub blocked_tools: Option<Vec<String>>,
    /// Parent session ID
    pub parent_session_id: String,
    /// Working directory
    pub cwd: String,
    /// Parent's tool registry (for inheritance)
    pub parent_tools: Option<Arc<ToolRegistry>>,
    /// Permission mode
    pub permission_mode: Option<PermissionMode>,
}

impl std::fmt::Debug for SubagentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentConfig")
            .field("model", &self.model)
            .field("max_turns", &self.max_turns)
            .field("max_budget_usd", &self.max_budget_usd)
            .field("allowed_tools", &self.allowed_tools)
            .field("blocked_tools", &self.blocked_tools)
            .field("parent_session_id", &self.parent_session_id)
            .field("cwd", &self.cwd)
            .field(
                "parent_tools",
                &self.parent_tools.as_ref().map(|_| "<ToolRegistry>"),
            )
            .field("permission_mode", &self.permission_mode)
            .finish()
    }
}

/// Subagent result
#[derive(Debug, Clone)]
pub struct SubagentResult {
    /// Result content
    pub content: String,
    /// Token usage
    pub usage: Option<Usage>,
    /// Total cost
    pub total_cost_usd: Option<f64>,
    /// Duration
    pub duration_ms: u64,
    /// Number of turns used
    pub num_turns: u32,
    /// Limit reached (if any)
    pub limit_reached: Option<String>,
}

/// Agent type information
#[derive(Debug, Clone, Serialize)]
pub struct AgentTypeInfo {
    /// Type name
    pub name: String,
    /// Description
    pub description: String,
    /// Available tools
    pub tools: Vec<String>,
    /// Default model for this type
    pub default_model: String,
    /// Default max turns
    pub default_max_turns: u32,
}

// ============================================================================
// Model Resolution
// ============================================================================

/// Resolve model shorthand to full model ID
fn resolve_model(model: &str) -> String {
    match model.to_lowercase().as_str() {
        "sonnet" => "claude-sonnet-4-6".to_string(),
        "opus" => "claude-opus-4-6".to_string(),
        "haiku" => "claude-haiku-3-20250514".to_string(),
        // Already a full model ID
        _ => model.to_string(),
    }
}

/// Get default model for agent type (legacy fallback)
#[allow(dead_code)]
fn default_model_for_type(_agent_type: &str) -> String {
    // All subagents use sonnet by default (configurable via SubagentSystemConfig)
    "claude-sonnet-4-6".to_string()
}

/// Get default max turns for agent type (legacy fallback)
#[allow(dead_code)]
fn default_max_turns_for_type(agent_type: &str) -> u32 {
    // Configurable via SubagentSystemConfig, these are legacy fallbacks
    match agent_type {
        "Explore" => 20,
        "Bash" => 10,
        "Plan" => 30,
        "Code" => 50,
        _ => 20,
    }
}

// ============================================================================
// Task Tool Implementation
// ============================================================================

/// Task tool for delegating to subagents
pub struct TaskTool {
    /// Agent factory
    factory: Option<Arc<dyn AgentFactory>>,
    /// Parent tool registry for inheritance by subagents
    tool_registry: Option<Arc<ToolRegistry>>,
}

impl Default for TaskTool {
    fn default() -> Self {
        Self {
            factory: None,
            tool_registry: None,
        }
    }
}

impl TaskTool {
    /// Create a new task tool
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with an agent factory
    pub fn with_factory(factory: Arc<dyn AgentFactory>) -> Self {
        Self {
            factory: Some(factory),
            tool_registry: None,
        }
    }

    /// Set the agent factory
    pub fn set_factory(&mut self, factory: Arc<dyn AgentFactory>) {
        self.factory = Some(factory);
    }

    /// Set the parent tool registry for inheritance by subagents
    pub fn set_tool_registry(&mut self, registry: Arc<ToolRegistry>) {
        self.tool_registry = Some(registry);
    }
}

#[async_trait]
impl AgentTool for TaskTool {
    type Input = TaskInput;
    type Output = TaskOutput;

    fn name(&self) -> &str {
        "Task"
    }

    fn description(&self) -> &str {
        r#"Launch a new agent to handle complex, multi-step tasks autonomously.

The Task tool launches specialized agents (subprocesses) that autonomously handle complex tasks. Each agent type has specific capabilities and tools available to it.

Available agent types:
- Bash: Command execution specialist for running bash commands. Use this for git operations, command execution, and other terminal tasks. (Tools: Bash)
- Explore: Fast agent specialized for exploring codebases. Use this when you need to quickly find files by patterns, search code for keywords, or answer questions about the codebase. (Tools: Glob, Grep, Read)
- Plan: Software architect agent for designing implementation plans. Use this when you need to plan the implementation strategy for a task. (Tools: Glob, Grep, Read)
- Code: Full-featured coding agent. Use this for implementing features or fixing bugs. (Tools: Read, Write, Edit, Bash, Glob, Grep)

Model options (optional):
- sonnet: Claude Sonnet 4 (default, balanced)
- opus: Claude Opus 4 (most capable, higher cost)
- haiku: Claude Haiku 3 (fastest, lowest cost)

When to use Task tool:
- Complex multi-step tasks that benefit from isolated context
- Exploring unfamiliar codebases
- Running multiple independent operations in parallel
- Tasks that may exceed the main agent's context limits

When NOT to use:
- If you want to read a specific file path, use the Read or Glob tool instead
- If you are searching for a specific class definition like "class Foo", use the Glob tool instead
- Simple tasks that can be done in 1-2 tool calls

Usage notes:
- Launch multiple agents concurrently whenever possible for optimal performance
- Provide clear, detailed prompts so the agent can work autonomously
- The agent's outputs should generally be trusted
- Set max_turns and max_budget_usd to control resource usage"#
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "A short (3-5 word) description of the task"
                },
                "prompt": {
                    "type": "string",
                    "description": "The task for the agent to perform"
                },
                "subagent_type": {
                    "type": "string",
                    "enum": ["Explore", "Bash", "Plan", "Code"],
                    "description": "The type of specialized agent to use"
                },
                "model": {
                    "type": "string",
                    "enum": ["sonnet", "opus", "haiku"],
                    "description": "Optional model to use (default: sonnet)"
                },
                "max_turns": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Maximum number of turns (default varies by type)"
                },
                "max_budget_usd": {
                    "type": "number",
                    "minimum": 0.01,
                    "description": "Maximum budget in USD for this agent"
                },
                "allowed_tools": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Tools to grant this agent (inherits from type defaults if not specified)"
                },
                "blocked_tools": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Tools to block for this agent"
                },
                "resume": {
                    "type": "string",
                    "description": "Agent ID to resume"
                }
            },
            "required": ["description", "prompt", "subagent_type"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true // Subagents can modify state
    }

    fn namespace(&self) -> &str {
        "agent"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        let factory = self
            .factory
            .as_ref()
            .ok_or_else(|| ToolError::ExecutionError("Agent factory not configured".to_string()))?;

        let start = Instant::now();
        let agent_type = input.subagent_type.clone();

        // Build hook context
        let hook_context = HookContext {
            session_id: context.session_id.clone(),
            cwd: Some(context.cwd.to_string_lossy().to_string()),
            env: None,
            metadata: None,
        };

        // Create or resume agent
        let mut agent = if let Some(agent_id) = &input.resume {
            factory.resume(agent_id).await?
        } else {
            let config = SubagentConfig {
                model: input.model.map(|m| resolve_model(&m)),
                max_turns: input.max_turns,
                max_budget_usd: input.max_budget_usd,
                allowed_tools: input.allowed_tools,
                blocked_tools: input.blocked_tools,
                parent_session_id: context.session_id.clone(),
                cwd: context.cwd.to_string_lossy().to_string(),
                parent_tools: self.tool_registry.clone(),
                permission_mode: Some(context.permission_mode),
            };

            let agent = factory.create(&agent_type, config).await?;

            // Fire SubagentSpawn hook
            let spawn_data = SubagentSpawnHookData {
                agent_type: agent_type.clone(),
                description: input.description.clone(),
                prompt: input.prompt.clone(),
                parent_session_id: context.session_id.clone(),
                subagent_session_id: agent.id().to_string(),
            };
            factory.fire_spawn_hook(spawn_data, &hook_context).await;

            agent
        };

        let agent_id = agent.id().to_string();

        // Run agent
        let result = agent.run(&input.prompt).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        // Fire SubagentComplete hook
        let (output_result, is_error) = match &result {
            Ok(r) => (serde_json::json!({"content": r.content}), false),
            Err(e) => (serde_json::json!({"error": e.to_string()}), true),
        };

        let complete_data = SubagentCompleteHookData {
            agent_type: agent_type.clone(),
            parent_session_id: context.session_id.clone(),
            subagent_session_id: agent_id.clone(),
            result: output_result,
            is_error,
            duration_ms,
        };
        factory
            .fire_complete_hook(complete_data, &hook_context)
            .await;

        // Return result
        match result {
            Ok(subagent_result) => Ok(TaskOutput {
                result: subagent_result.content,
                agent_id,
                usage: subagent_result.usage,
                total_cost_usd: subagent_result.total_cost_usd,
                duration_ms,
                running: None,
                output_file: None,
                num_turns: Some(subagent_result.num_turns),
                limit_reached: subagent_result.limit_reached,
            }),
            Err(e) => Err(e),
        }
    }
}

// ============================================================================
// Real Agent Factory Implementation
// ============================================================================

/// Running agent state for resumption
#[allow(dead_code)]
struct RunningAgent {
    /// Agent ID
    id: String,
    /// Agent type
    agent_type: String,
    /// The underlying runner (wrapped for ownership)
    runner: AgentRunner,
    /// Whether the agent is currently running
    running: bool,
    /// Creation time
    created_at: Instant,
}

/// Real agent factory that creates actual subagents
///
/// This factory creates real `AgentRunner` instances configured for specific
/// agent types (Explore, Bash, Plan, Code). Each subagent has its own LLM client,
/// tool registry, and permission context.
///
/// ## Claude-style Architecture
///
/// Uses Single Agent + Subagent pattern:
/// - Lead Agent (configurable, default Opus) coordinates complex tasks
/// - Subagents (configurable, default Sonnet) execute specialized subtasks
/// - Max 3 parallel subagents by default
/// - 150K token context per subagent
#[allow(dead_code)]
pub struct RealAgentFactory {
    /// LLM router for API calls
    llm_router: Arc<LlmRouter>,
    /// Default hook executor
    hooks: Arc<HookExecutor>,
    /// Running agents by ID (for resumption)
    running_agents: Arc<RwLock<HashMap<String, RunningAgent>>>,
    /// Default working directory
    default_cwd: Option<PathBuf>,
    /// Default model to use (overrides subagent_config if set)
    default_model: Option<String>,
    /// Default max tokens
    default_max_tokens: Option<u32>,
    /// Parent tool registry for inheritance
    parent_tools: Option<Arc<ToolRegistry>>,
    /// Total cost tracker
    total_cost: Arc<AtomicU64>,
    /// Subagent system configuration
    subagent_config: crate::agent::r#loop::SubagentSystemConfig,
    /// Semaphore for parallel execution control
    parallel_semaphore: Arc<tokio::sync::Semaphore>,
}

impl RealAgentFactory {
    /// Create a new real agent factory with default configuration
    ///
    /// Default settings:
    /// - Lead model: claude-opus-4 (configurable via SubagentSystemConfig)
    /// - Subagent model: claude-sonnet-4 (configurable)
    /// - Max parallel: 3
    /// - Context: 150K tokens
    pub fn new(llm_router: Arc<LlmRouter>) -> Self {
        let config = crate::agent::r#loop::SubagentSystemConfig::default();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(config.max_parallel_subagents));
        Self {
            llm_router,
            hooks: Arc::new(HookExecutor::new()),
            running_agents: Arc::new(RwLock::new(HashMap::new())),
            default_cwd: None,
            default_model: None,
            default_max_tokens: None,
            parent_tools: None,
            total_cost: Arc::new(AtomicU64::new(0)),
            subagent_config: config,
            parallel_semaphore: semaphore,
        }
    }

    /// Create with custom subagent configuration
    pub fn with_subagent_config(
        llm_router: Arc<LlmRouter>,
        config: crate::agent::r#loop::SubagentSystemConfig,
    ) -> Self {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(config.max_parallel_subagents));
        Self {
            llm_router,
            hooks: Arc::new(HookExecutor::new()),
            running_agents: Arc::new(RwLock::new(HashMap::new())),
            default_cwd: None,
            default_model: None,
            default_max_tokens: None,
            parent_tools: None,
            total_cost: Arc::new(AtomicU64::new(0)),
            subagent_config: config,
            parallel_semaphore: semaphore,
        }
    }

    /// Set the hook executor
    pub fn with_hooks(mut self, hooks: Arc<HookExecutor>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Set default working directory
    pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
        self.default_cwd = Some(cwd);
        self
    }

    /// Set default model (overrides subagent_config.default_subagent_model)
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }

    /// Set default max tokens
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.default_max_tokens = Some(max_tokens);
        self
    }

    /// Set parent tool registry for inheritance
    pub fn with_parent_tools(mut self, tools: Arc<ToolRegistry>) -> Self {
        self.parent_tools = Some(tools);
        self
    }

    /// Get the parallel execution semaphore for limiting concurrent subagents
    pub fn parallel_semaphore(&self) -> Arc<tokio::sync::Semaphore> {
        self.parallel_semaphore.clone()
    }

    /// Get the subagent configuration
    pub fn subagent_config(&self) -> &crate::agent::r#loop::SubagentSystemConfig {
        &self.subagent_config
    }

    /// Create an LLM adapter with optional model override
    ///
    /// Model resolution order:
    /// 1. Explicit model_override parameter
    /// 2. default_model field (if set via with_model())
    /// 3. subagent_config.default_subagent_model
    fn create_llm_adapter(&self, model_override: Option<&str>) -> Arc<LlmRouterAdapter> {
        let mut adapter = LlmRouterAdapter::new(self.llm_router.clone());

        // Resolve model using priority order
        let model = model_override
            .map(|m| self.subagent_config.resolve_model(m))
            .or_else(|| self.default_model.clone())
            .unwrap_or_else(|| self.subagent_config.default_subagent_model.clone());

        adapter = adapter.with_model(&model);

        // Use configured context size for max tokens
        let max_tokens = self
            .default_max_tokens
            .unwrap_or(self.subagent_config.subagent_context_tokens as u32);
        adapter = adapter.with_max_tokens(max_tokens);

        Arc::new(adapter)
    }

    /// Create a tool registry configured for a specific agent type
    fn create_tool_registry_for_type(
        &self,
        agent_type: &str,
        allowed_tools: Option<&[String]>,
        blocked_tools: Option<&[String]>,
    ) -> Arc<ToolRegistry> {
        use super::{BashTool, ClaudeCodeTool, EditTool, GlobTool, GrepTool, ReadTool, WriteTool};

        let mut registry = ToolRegistry::new();

        // Get default tools for this agent type
        let default_tools = match agent_type {
            "Explore" | "Plan" => vec!["Glob", "Grep", "Read"],
            "Bash" => vec!["Bash", "ClaudeCode"],
            "Code" => vec![
                "Read",
                "Write",
                "Edit",
                "Bash",
                "Glob",
                "Grep",
                "ClaudeCode",
            ],
            _ => vec!["Read", "Glob", "Grep"],
        };

        // Use allowed_tools if specified, otherwise use defaults
        let tools_to_use: Vec<&str> = allowed_tools
            .map(|t| t.iter().map(|s| s.as_str()).collect())
            .unwrap_or(default_tools);

        // Filter out blocked tools
        let blocked: Vec<&str> = blocked_tools
            .map(|t| t.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();

        // Register only allowed tools
        for tool_name in tools_to_use {
            if blocked.contains(&tool_name) {
                continue;
            }

            match tool_name {
                "Read" => registry.register_tool(ReadTool::default()),
                "Write" => registry.register_tool(WriteTool),
                "Edit" => registry.register_tool(EditTool),
                "Bash" => registry.register_tool(BashTool::new()),
                "Glob" => registry.register_tool(GlobTool::default()),
                "Grep" => registry.register_tool(GrepTool::default()),
                "ClaudeCode" => registry.register_tool(ClaudeCodeTool::new()),
                _ => {
                    // Try to inherit from parent if available
                    if let Some(parent) = &self.parent_tools {
                        if let Some(_tool) = parent.get_builtin(tool_name) {
                            // Note: We can't easily clone dynamic tools, so we skip unknown tools
                            tracing::warn!(
                                tool_name = tool_name,
                                "Cannot inherit tool from parent - tool cloning not supported"
                            );
                        }
                    }
                }
            }
        }

        Arc::new(registry)
    }

    /// Create a permission context for a subagent
    fn create_permission_context(
        &self,
        agent_type: &str,
        cwd: &str,
        permission_mode: Option<PermissionMode>,
    ) -> PermissionContext {
        let mode = permission_mode.unwrap_or_else(|| match agent_type {
            "Plan" => PermissionMode::Plan,        // Read-only mode for planning
            "Explore" => PermissionMode::Plan,     // Read-only for exploration
            "Bash" => PermissionMode::AcceptEdits, // Bash needs execution permission
            "Code" => PermissionMode::AcceptEdits, // Code agent needs write permission
            _ => PermissionMode::Default,
        });

        PermissionContext {
            mode,
            cwd: Some(cwd.to_string()),
            allowed_directories: vec![cwd.to_string()],
            ..Default::default()
        }
    }

    /// Get system prompt for agent type
    fn get_system_prompt(&self, agent_type: &str) -> String {
        match agent_type {
            "Explore" => r#"You are an Explore agent specialized in quickly searching and understanding codebases.

Your capabilities:
- Glob: Find files matching patterns
- Grep: Search file contents with regex
- Read: Read file contents

Your task is to efficiently explore and answer questions about the codebase.
Focus on finding relevant files and understanding code structure.
Be concise and direct in your responses.

Guidelines:
- Use Glob to find files by name patterns
- Use Grep to search for specific code patterns or text
- Use Read to examine file contents
- Combine tools effectively to answer questions quickly"#.to_string(),

            "Bash" => r#"You are a Bash agent specialized in executing shell commands.

Your capabilities:
- Bash: Execute shell commands

Your task is to help with command-line operations like:
- Git operations (status, add, commit, push, pull, branch, etc.)
- File system commands (ls, mkdir, rm, mv, cp, etc.)
- Running scripts and builds (npm, cargo, make, etc.)
- System administration tasks

Guidelines:
- Always explain what commands you're running and why
- Be careful with destructive commands (rm, reset, etc.)
- Check command results before proceeding
- Use --dry-run flags when available for risky operations"#.to_string(),

            "Plan" => r#"You are a Plan agent specialized in software architecture and planning.

Your capabilities:
- Glob: Find files matching patterns
- Grep: Search file contents with regex
- Read: Read file contents

Your task is to analyze codebases and design implementation plans.
Focus on:
- Understanding existing architecture
- Identifying patterns and conventions
- Creating step-by-step implementation plans
- Considering edge cases and potential issues

Guidelines:
- Do not make changes - only analyze and plan
- Provide concrete, actionable steps
- Reference specific files and code locations
- Consider testing requirements"#.to_string(),

            "Code" => r#"You are a Code agent specialized in implementing features and fixing bugs.

Your capabilities:
- Read: Read file contents
- Write: Create or overwrite files
- Edit: Make targeted edits to existing files
- Bash: Execute shell commands
- Glob: Find files matching patterns
- Grep: Search file contents with regex

Your task is to implement code changes effectively and safely.

Guidelines:
- Always read existing code before making changes
- Prefer Edit over Write for existing files
- Follow existing code style and patterns
- Run tests after making changes when possible
- Create small, focused commits"#.to_string(),

            _ => format!("You are a {} agent. Complete the assigned task efficiently.", agent_type),
        }
    }

    /// Get allowed tools for agent type
    fn get_allowed_tools(&self, agent_type: &str) -> Vec<String> {
        match agent_type {
            "Explore" | "Plan" => vec!["Glob".to_string(), "Grep".to_string(), "Read".to_string()],
            "Bash" => vec!["Bash".to_string()],
            "Code" => vec![
                "Read".to_string(),
                "Write".to_string(),
                "Edit".to_string(),
                "Bash".to_string(),
                "Glob".to_string(),
                "Grep".to_string(),
            ],
            _ => vec!["Read".to_string()],
        }
    }
}

#[async_trait]
impl AgentFactory for RealAgentFactory {
    async fn create(
        &self,
        agent_type: &str,
        config: SubagentConfig,
    ) -> Result<Box<dyn Subagent>, ToolError> {
        // Validate agent type
        let valid_types = ["Explore", "Bash", "Plan", "Code"];
        if !valid_types.contains(&agent_type) {
            return Err(ToolError::InvalidInput(format!(
                "Unknown agent type: {}. Valid types are: {:?}",
                agent_type, valid_types
            )));
        }

        // Create agent ID
        let agent_id = Uuid::new_v4().to_string();

        // Determine configuration values using SubagentSystemConfig
        let cwd = PathBuf::from(&config.cwd);
        let max_turns = config
            .max_turns
            .unwrap_or_else(|| self.subagent_config.max_turns_for_type(agent_type));
        let model = config
            .model
            .clone()
            .map(|m| self.subagent_config.resolve_model(&m))
            .unwrap_or_else(|| self.subagent_config.default_subagent_model.clone());

        // Create agent config
        let mut agent_config = AgentConfig::default();
        agent_config.cwd = Some(cwd);
        agent_config.max_turns = max_turns;
        agent_config.max_budget_usd = config.max_budget_usd;
        agent_config.system_prompt = Some(self.get_system_prompt(agent_type));
        agent_config.tools = self.get_allowed_tools(agent_type);

        // Set permission mode based on agent type
        agent_config.permission_mode = config.permission_mode.unwrap_or_else(|| match agent_type {
            "Plan" | "Explore" => PermissionMode::Plan,
            "Bash" | "Code" => PermissionMode::AcceptEdits,
            _ => PermissionMode::Default,
        });

        // Create components
        let llm_adapter = self.create_llm_adapter(Some(&model));
        let tool_registry = self.create_tool_registry_for_type(
            agent_type,
            config.allowed_tools.as_deref(),
            config.blocked_tools.as_deref(),
        );
        let permission_ctx =
            self.create_permission_context(agent_type, &config.cwd, config.permission_mode);

        // Create the runner
        let runner = AgentRunner::with_session_id(agent_config, &agent_id)
            .with_llm(llm_adapter)
            .with_tools(tool_registry)
            .with_hooks(self.hooks.clone())
            .with_permission_context(permission_ctx);

        // Create the subagent
        let subagent = RealSubagent {
            id: agent_id.clone(),
            agent_type: agent_type.to_string(),
            runner: Arc::new(RwLock::new(runner)),
            running_agents: self.running_agents.clone(),
            max_budget_usd: config.max_budget_usd,
            total_cost: Arc::new(AtomicU64::new(0)),
        };

        Ok(Box::new(subagent))
    }

    async fn resume(&self, agent_id: &str) -> Result<Box<dyn Subagent>, ToolError> {
        let agents = self.running_agents.read().await;

        if agents.contains_key(agent_id) {
            // Agent exists - return a resumed subagent
            // Note: We can't actually resume mid-execution, but we can continue
            // the conversation with the same agent
            return Err(ToolError::ExecutionError(format!(
                "Agent {} is already running. Wait for completion or interrupt it.",
                agent_id
            )));
        }

        Err(ToolError::NotFound(format!(
            "Agent not found: {}. It may have completed or been cleaned up.",
            agent_id
        )))
    }

    fn available_types(&self) -> Vec<AgentTypeInfo> {
        let default_model = self.subagent_config.default_subagent_model.clone();
        vec![
            AgentTypeInfo {
                name: "Explore".to_string(),
                description: "Fast agent for exploring codebases. Use for finding files and understanding code structure.".to_string(),
                tools: vec!["Glob".to_string(), "Grep".to_string(), "Read".to_string()],
                default_model: default_model.clone(),
                default_max_turns: self.subagent_config.explore_max_turns,
            },
            AgentTypeInfo {
                name: "Bash".to_string(),
                description: "Command execution specialist. Use for git operations, running scripts, and system commands.".to_string(),
                tools: vec!["Bash".to_string()],
                default_model: default_model.clone(),
                default_max_turns: self.subagent_config.bash_max_turns,
            },
            AgentTypeInfo {
                name: "Plan".to_string(),
                description: "Software architect for designing plans. Use for analyzing code and creating implementation strategies.".to_string(),
                tools: vec!["Glob".to_string(), "Grep".to_string(), "Read".to_string()],
                default_model: default_model.clone(),
                default_max_turns: self.subagent_config.plan_max_turns,
            },
            AgentTypeInfo {
                name: "Code".to_string(),
                description: "Full-featured coding agent. Use for implementing features or fixing bugs.".to_string(),
                tools: vec![
                    "Read".to_string(),
                    "Write".to_string(),
                    "Edit".to_string(),
                    "Bash".to_string(),
                    "Glob".to_string(),
                    "Grep".to_string(),
                ],
                default_model: default_model.clone(),
                default_max_turns: self.subagent_config.code_max_turns,
            },
        ]
    }

    async fn fire_spawn_hook(&self, hook_data: SubagentSpawnHookData, context: &HookContext) {
        let data = serde_json::to_value(&hook_data).unwrap_or_default();
        self.hooks
            .execute(HookEvent::SubagentSpawn, data, context)
            .await;
    }

    async fn fire_complete_hook(&self, hook_data: SubagentCompleteHookData, context: &HookContext) {
        let data = serde_json::to_value(&hook_data).unwrap_or_default();
        self.hooks
            .execute(HookEvent::SubagentComplete, data, context)
            .await;
    }
}

// ============================================================================
// Real Subagent Implementation
// ============================================================================

/// Real subagent implementation using AgentRunner
#[allow(dead_code)]
struct RealSubagent {
    /// Unique agent ID
    id: String,
    /// Agent type (Explore, Bash, Plan, Code)
    agent_type: String,
    /// The underlying AgentRunner
    runner: Arc<RwLock<AgentRunner>>,
    /// Reference to running agents map for cleanup
    running_agents: Arc<RwLock<HashMap<String, RunningAgent>>>,
    /// Maximum budget in USD
    max_budget_usd: Option<f64>,
    /// Total cost tracker (in micro-USD for atomic ops)
    total_cost: Arc<AtomicU64>,
}

#[async_trait]
impl Subagent for RealSubagent {
    fn id(&self) -> &str {
        &self.id
    }

    fn agent_type(&self) -> &str {
        &self.agent_type
    }

    async fn run(&mut self, prompt: &str) -> Result<SubagentResult, ToolError> {
        let start = Instant::now();
        let mut result_content = String::new();
        let mut final_usage: Option<Usage> = None;
        let mut num_turns = 0u32;
        let mut limit_reached: Option<String> = None;

        // Run the agent
        {
            let mut runner = self.runner.write().await;
            let mut stream = runner.query(prompt).await;

            while let Some(msg_result) = stream.next().await {
                match msg_result {
                    Ok(msg) => {
                        match msg {
                            AgentMessage::Assistant(assistant) => {
                                // Collect text content from assistant messages
                                for block in &assistant.content {
                                    if let Some(text) = block.as_text() {
                                        if !result_content.is_empty() {
                                            result_content.push('\n');
                                        }
                                        result_content.push_str(text);
                                    }
                                }
                            }
                            AgentMessage::Result(result) => {
                                // Use the final result if available
                                if let Some(text) = result.result {
                                    result_content = text;
                                }
                                final_usage = result.usage;
                                num_turns = result.num_turns;

                                // Check for limits
                                match result.subtype {
                                    crate::agent::types::ResultSubtype::ErrorMaxTurns => {
                                        limit_reached = Some("max_turns".to_string());
                                    }
                                    crate::agent::types::ResultSubtype::ErrorMaxBudgetUsd => {
                                        limit_reached = Some("max_budget".to_string());
                                    }
                                    _ => {}
                                }
                                break;
                            }
                            _ => {
                                // Skip other message types (User, System, StreamEvent)
                            }
                        }
                    }
                    Err(e) => {
                        return Err(ToolError::ExecutionError(format!(
                            "Agent execution error: {}",
                            e
                        )));
                    }
                }
            }
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        // If we got no content, provide a default message
        if result_content.is_empty() {
            result_content = format!("[{}] Agent completed with no output.", self.agent_type);
        }

        // Calculate cost (simplified estimate)
        let total_cost_usd = final_usage.as_ref().map(|u| {
            // Rough cost estimate: $3/million input, $15/million output for Sonnet
            let input_cost = (u.input_tokens as f64) * 0.000003;
            let output_cost = (u.output_tokens as f64) * 0.000015;
            input_cost + output_cost
        });

        Ok(SubagentResult {
            content: result_content,
            usage: final_usage,
            total_cost_usd,
            duration_ms,
            num_turns,
            limit_reached,
        })
    }

    async fn interrupt(&mut self) -> Result<(), ToolError> {
        let mut runner = self.runner.write().await;
        runner
            .interrupt()
            .await
            .map_err(|e| ToolError::ExecutionError(format!("Failed to interrupt agent: {}", e)))
    }

    fn is_running(&self) -> bool {
        // Check via try_read to avoid blocking
        if let Ok(runner) = self.runner.try_read() {
            runner.is_running()
        } else {
            // If we can't get the lock, assume it's running
            true
        }
    }

    fn current_usage(&self) -> Option<Usage> {
        // Would need to track this separately for real-time access
        None
    }

    fn current_cost(&self) -> Option<f64> {
        let micro_usd = self.total_cost.load(Ordering::Relaxed);
        if micro_usd > 0 {
            Some(micro_usd as f64 / 1_000_000.0)
        } else {
            None
        }
    }
}

// ============================================================================
// Placeholder Implementation (for testing)
// ============================================================================

/// Placeholder agent factory for testing
///
/// This factory returns placeholder subagents that don't actually call any LLM.
/// Useful for unit tests and integration tests where you don't want to make
/// real API calls.
pub struct PlaceholderAgentFactory {
    hooks: Arc<HookExecutor>,
}

impl PlaceholderAgentFactory {
    pub fn new() -> Self {
        Self {
            hooks: Arc::new(HookExecutor::new()),
        }
    }

    pub fn with_hooks(hooks: Arc<HookExecutor>) -> Self {
        Self { hooks }
    }
}

impl Default for PlaceholderAgentFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AgentFactory for PlaceholderAgentFactory {
    async fn create(
        &self,
        agent_type: &str,
        _config: SubagentConfig,
    ) -> Result<Box<dyn Subagent>, ToolError> {
        Ok(Box::new(PlaceholderSubagent {
            id: Uuid::new_v4().to_string(),
            agent_type: agent_type.to_string(),
        }))
    }

    async fn resume(&self, agent_id: &str) -> Result<Box<dyn Subagent>, ToolError> {
        Ok(Box::new(PlaceholderSubagent {
            id: agent_id.to_string(),
            agent_type: "resumed".to_string(),
        }))
    }

    fn available_types(&self) -> Vec<AgentTypeInfo> {
        vec![
            AgentTypeInfo {
                name: "Explore".to_string(),
                description: "Fast agent for exploring codebases".to_string(),
                tools: vec!["Glob".to_string(), "Grep".to_string(), "Read".to_string()],
                default_model: "claude-sonnet-4-6".to_string(),
                default_max_turns: 20,
            },
            AgentTypeInfo {
                name: "Bash".to_string(),
                description: "Command execution specialist".to_string(),
                tools: vec!["Bash".to_string()],
                default_model: "claude-sonnet-4-6".to_string(),
                default_max_turns: 10,
            },
            AgentTypeInfo {
                name: "Plan".to_string(),
                description: "Software architect for designing plans".to_string(),
                tools: vec!["Glob".to_string(), "Grep".to_string(), "Read".to_string()],
                default_model: "claude-sonnet-4-6".to_string(),
                default_max_turns: 30,
            },
            AgentTypeInfo {
                name: "Code".to_string(),
                description: "Full-featured coding agent".to_string(),
                tools: vec![
                    "Read".to_string(),
                    "Write".to_string(),
                    "Edit".to_string(),
                    "Bash".to_string(),
                    "Glob".to_string(),
                    "Grep".to_string(),
                ],
                default_model: "claude-sonnet-4-6".to_string(),
                default_max_turns: 50,
            },
        ]
    }

    async fn fire_spawn_hook(&self, hook_data: SubagentSpawnHookData, context: &HookContext) {
        let data = serde_json::to_value(&hook_data).unwrap_or_default();
        self.hooks
            .execute(HookEvent::SubagentSpawn, data, context)
            .await;
    }

    async fn fire_complete_hook(&self, hook_data: SubagentCompleteHookData, context: &HookContext) {
        let data = serde_json::to_value(&hook_data).unwrap_or_default();
        self.hooks
            .execute(HookEvent::SubagentComplete, data, context)
            .await;
    }
}

/// Placeholder subagent for testing
struct PlaceholderSubagent {
    id: String,
    agent_type: String,
}

#[async_trait]
impl Subagent for PlaceholderSubagent {
    fn id(&self) -> &str {
        &self.id
    }

    fn agent_type(&self) -> &str {
        &self.agent_type
    }

    async fn run(&mut self, prompt: &str) -> Result<SubagentResult, ToolError> {
        // Simulate some work
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        Ok(SubagentResult {
            content: format!(
                "[{}] Placeholder result for: {}",
                self.agent_type,
                &prompt[..prompt.len().min(50)]
            ),
            usage: Some(Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            }),
            total_cost_usd: Some(0.0005),
            duration_ms: 100,
            num_turns: 1,
            limit_reached: None,
        })
    }

    async fn interrupt(&mut self) -> Result<(), ToolError> {
        Ok(())
    }

    fn is_running(&self) -> bool {
        false
    }

    fn current_usage(&self) -> Option<Usage> {
        None
    }

    fn current_cost(&self) -> Option<f64> {
        None
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmConfig;

    #[tokio::test]
    async fn test_task_tool_with_placeholder() {
        let factory = Arc::new(PlaceholderAgentFactory::new());
        let tool = TaskTool::with_factory(factory);

        let context = ToolContext::new("s1", "/tmp")
            .with_allowed_directory("/tmp")
            .with_permission_mode(PermissionMode::AcceptEdits);

        let input = TaskInput {
            description: "Test task".to_string(),
            prompt: "Do something".to_string(),
            subagent_type: "Explore".to_string(),
            model: None,
            max_turns: None,
            max_budget_usd: None,
            allowed_tools: None,
            blocked_tools: None,

            resume: None,
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert!(output.result.contains("Placeholder"));
        assert!(output.usage.is_some());
        assert!(output.total_cost_usd.is_some());
    }

    #[tokio::test]
    async fn test_task_tool_with_model_override() {
        let factory = Arc::new(PlaceholderAgentFactory::new());
        let tool = TaskTool::with_factory(factory);

        let context =
            ToolContext::new("s1", "/tmp").with_permission_mode(PermissionMode::AcceptEdits);

        let input = TaskInput {
            description: "Test with opus".to_string(),
            prompt: "Do something complex".to_string(),
            subagent_type: "Code".to_string(),
            model: Some("opus".to_string()),
            max_turns: Some(10),
            max_budget_usd: Some(1.0),
            allowed_tools: None,
            blocked_tools: None,

            resume: None,
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert!(output.result.contains("Placeholder"));
    }

    #[test]
    fn test_model_resolution() {
        assert_eq!(resolve_model("sonnet"), "claude-sonnet-4-6");
        assert_eq!(resolve_model("opus"), "claude-opus-4-6");
        assert_eq!(resolve_model("haiku"), "claude-haiku-3-20250514");
        assert_eq!(resolve_model("claude-sonnet-4-6"), "claude-sonnet-4-6");
    }

    #[test]
    fn test_default_max_turns() {
        assert_eq!(default_max_turns_for_type("Explore"), 20);
        assert_eq!(default_max_turns_for_type("Bash"), 10);
        assert_eq!(default_max_turns_for_type("Plan"), 30);
        assert_eq!(default_max_turns_for_type("Code"), 50);
    }

    #[test]
    fn test_agent_type_info() {
        let factory = PlaceholderAgentFactory::new();
        let types = factory.available_types();
        assert_eq!(types.len(), 4);
        assert!(types.iter().any(|t| t.name == "Explore"));
        assert!(types.iter().any(|t| t.name == "Bash"));
        assert!(types.iter().any(|t| t.name == "Plan"));
        assert!(types.iter().any(|t| t.name == "Code"));
    }

    // Tests for RealAgentFactory

    fn create_test_router() -> Arc<LlmRouter> {
        Arc::new(LlmRouter::new(LlmConfig::default()))
    }

    #[test]
    fn test_real_agent_factory_creation() {
        let router = create_test_router();
        let factory = RealAgentFactory::new(router);

        // Check available types
        let types = factory.available_types();
        assert_eq!(types.len(), 4);
        assert!(types.iter().any(|t| t.name == "Explore"));
        assert!(types.iter().any(|t| t.name == "Bash"));
        assert!(types.iter().any(|t| t.name == "Plan"));
        assert!(types.iter().any(|t| t.name == "Code"));
    }

    #[test]
    fn test_real_agent_factory_builder() {
        let router = create_test_router();
        let factory = RealAgentFactory::new(router)
            .with_model("claude-sonnet-4")
            .with_max_tokens(4096)
            .with_cwd(PathBuf::from("/tmp"));

        assert_eq!(factory.default_model, Some("claude-sonnet-4".to_string()));
        assert_eq!(factory.default_max_tokens, Some(4096));
        assert_eq!(factory.default_cwd, Some(PathBuf::from("/tmp")));
    }

    // Note: These tests that create RealAgentFactory subagents are more integration tests.
    // The PlaceholderAgentFactory tests above cover the core TaskTool functionality.
    // These tests are currently disabled because AgentState uses blocking_write which
    // doesn't work well with nested tokio runtimes.
    //
    // To run these tests properly, you would need to:
    // 1. Modify AgentState to use async writes instead of blocking_write, or
    // 2. Run these as integration tests in a separate process

    #[test]
    fn test_real_agent_factory_available_types() {
        let router = create_test_router();
        let factory = RealAgentFactory::new(router);

        // Test available types without creating agents
        let types = factory.available_types();
        assert_eq!(types.len(), 4);

        // Verify Explore type
        let explore = types.iter().find(|t| t.name == "Explore").unwrap();
        assert_eq!(explore.tools, vec!["Glob", "Grep", "Read"]);
        assert_eq!(explore.default_max_turns, 20);

        // Verify Bash type
        let bash = types.iter().find(|t| t.name == "Bash").unwrap();
        assert_eq!(bash.tools, vec!["Bash"]);
        assert_eq!(bash.default_max_turns, 10);

        // Verify Code type
        let code = types.iter().find(|t| t.name == "Code").unwrap();
        assert_eq!(code.tools.len(), 6);
        assert!(code.tools.contains(&"Write".to_string()));
        assert!(code.tools.contains(&"Edit".to_string()));
        assert_eq!(code.default_max_turns, 50);
    }

    #[test]
    fn test_real_agent_factory_system_prompts() {
        let router = create_test_router();
        let factory = RealAgentFactory::new(router);

        // Test system prompts
        let explore_prompt = factory.get_system_prompt("Explore");
        assert!(explore_prompt.contains("Explore agent"));
        assert!(explore_prompt.contains("Glob"));

        let bash_prompt = factory.get_system_prompt("Bash");
        assert!(bash_prompt.contains("Bash agent"));

        let code_prompt = factory.get_system_prompt("Code");
        assert!(code_prompt.contains("Code agent"));
        assert!(code_prompt.contains("implementing features"));
    }

    #[test]
    fn test_real_agent_factory_allowed_tools() {
        let router = create_test_router();
        let factory = RealAgentFactory::new(router);

        let explore_tools = factory.get_allowed_tools("Explore");
        assert_eq!(explore_tools, vec!["Glob", "Grep", "Read"]);

        let bash_tools = factory.get_allowed_tools("Bash");
        assert_eq!(bash_tools, vec!["Bash"]);

        let code_tools = factory.get_allowed_tools("Code");
        assert_eq!(code_tools.len(), 6);
    }

    #[tokio::test]
    async fn test_real_agent_factory_invalid_type() {
        let router = create_test_router();
        let factory = RealAgentFactory::new(router);

        let config = SubagentConfig {
            model: None,
            max_turns: None,
            max_budget_usd: None,
            allowed_tools: None,
            blocked_tools: None,
            parent_session_id: "parent-123".to_string(),
            cwd: "/tmp".to_string(),
            parent_tools: None,
            permission_mode: None,
        };

        let result = factory.create("InvalidType", config).await;
        assert!(result.is_err());

        match result {
            Err(ToolError::InvalidInput(msg)) => {
                assert!(msg.contains("Unknown agent type"));
            }
            _ => panic!("Expected InvalidInput error"),
        }
    }

    #[tokio::test]
    async fn test_real_agent_factory_resume_not_found() {
        let router = create_test_router();
        let factory = RealAgentFactory::new(router);

        let result = factory.resume("non-existent-id").await;
        assert!(result.is_err());

        match result {
            Err(ToolError::NotFound(msg)) => {
                assert!(msg.contains("Agent not found"));
            }
            _ => panic!("Expected NotFound error"),
        }
    }

    #[test]
    fn test_get_system_prompt() {
        let router = create_test_router();
        let factory = RealAgentFactory::new(router);

        let explore_prompt = factory.get_system_prompt("Explore");
        assert!(explore_prompt.contains("Explore agent"));
        assert!(explore_prompt.contains("Glob"));
        assert!(explore_prompt.contains("Grep"));
        assert!(explore_prompt.contains("Read"));

        let bash_prompt = factory.get_system_prompt("Bash");
        assert!(bash_prompt.contains("Bash agent"));
        assert!(bash_prompt.contains("shell commands"));

        let plan_prompt = factory.get_system_prompt("Plan");
        assert!(plan_prompt.contains("Plan agent"));
        assert!(plan_prompt.contains("architecture"));

        let code_prompt = factory.get_system_prompt("Code");
        assert!(code_prompt.contains("Code agent"));
        assert!(code_prompt.contains("Write"));
        assert!(code_prompt.contains("Edit"));
    }

    #[test]
    fn test_get_allowed_tools() {
        let router = create_test_router();
        let factory = RealAgentFactory::new(router);

        let explore_tools = factory.get_allowed_tools("Explore");
        assert_eq!(explore_tools.len(), 3);
        assert!(explore_tools.contains(&"Glob".to_string()));
        assert!(explore_tools.contains(&"Grep".to_string()));
        assert!(explore_tools.contains(&"Read".to_string()));

        let bash_tools = factory.get_allowed_tools("Bash");
        assert_eq!(bash_tools.len(), 1);
        assert!(bash_tools.contains(&"Bash".to_string()));

        let code_tools = factory.get_allowed_tools("Code");
        assert_eq!(code_tools.len(), 6);
        assert!(code_tools.contains(&"Write".to_string()));
        assert!(code_tools.contains(&"Edit".to_string()));
    }

    #[tokio::test]
    async fn test_placeholder_fires_hooks() {
        use crate::agent::hooks::{HookCallback, RegisteredHook};
        use crate::agent::types::HookResult;
        use std::sync::atomic::{AtomicBool, Ordering};

        // Create a flag to track if hook was called
        let spawn_called = Arc::new(AtomicBool::new(false));
        let complete_called = Arc::new(AtomicBool::new(false));

        // Create hooks
        let hooks = Arc::new(HookExecutor::new());

        // Create spawn hook callback
        struct SpawnHook(Arc<AtomicBool>);
        #[async_trait]
        impl HookCallback for SpawnHook {
            async fn on_event(
                &self,
                _event: HookEvent,
                _data: serde_json::Value,
                _context: &HookContext,
            ) -> HookResult {
                self.0.store(true, Ordering::SeqCst);
                HookResult::continue_()
            }
            fn name(&self) -> &str {
                "spawn_test"
            }
            fn handles_event(&self, event: HookEvent) -> bool {
                matches!(event, HookEvent::SubagentSpawn)
            }
        }

        // Create complete hook callback
        struct CompleteHook(Arc<AtomicBool>);
        #[async_trait]
        impl HookCallback for CompleteHook {
            async fn on_event(
                &self,
                _event: HookEvent,
                _data: serde_json::Value,
                _context: &HookContext,
            ) -> HookResult {
                self.0.store(true, Ordering::SeqCst);
                HookResult::continue_()
            }
            fn name(&self) -> &str {
                "complete_test"
            }
            fn handles_event(&self, event: HookEvent) -> bool {
                matches!(event, HookEvent::SubagentComplete)
            }
        }

        // Register hooks
        hooks
            .register(RegisteredHook::new(
                Arc::new(SpawnHook(spawn_called.clone())),
                vec![HookEvent::SubagentSpawn],
            ))
            .await;

        hooks
            .register(RegisteredHook::new(
                Arc::new(CompleteHook(complete_called.clone())),
                vec![HookEvent::SubagentComplete],
            ))
            .await;

        // Create factory with hooks
        let factory = Arc::new(PlaceholderAgentFactory::with_hooks(hooks));
        let tool = TaskTool::with_factory(factory);

        let context =
            ToolContext::new("s1", "/tmp").with_permission_mode(PermissionMode::AcceptEdits);

        let input = TaskInput {
            description: "Hook test".to_string(),
            prompt: "Test hooks".to_string(),
            subagent_type: "Explore".to_string(),
            model: None,
            max_turns: None,
            max_budget_usd: None,
            allowed_tools: None,
            blocked_tools: None,

            resume: None,
        };

        let _ = tool.execute(input, &context).await.unwrap();

        // Verify hooks were called
        assert!(
            spawn_called.load(Ordering::SeqCst),
            "SubagentSpawn hook should have been called"
        );
        assert!(
            complete_called.load(Ordering::SeqCst),
            "SubagentComplete hook should have been called"
        );
    }
}
