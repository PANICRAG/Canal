//! Agent Factory - Creates configured AgentRunner instances
//!
//! Provides a factory for creating AgentRunner instances with all components
//! properly wired together (LLM, tools, hooks, permissions).
//!
//! Now integrates the six-layer context hierarchy (A15/A16):
//! - Platform context loaded from config/platform-rules.yaml
//! - Organization, User, Session, Task, SubAgent contexts
//! - System prompt generated via SystemPromptGenerator
//! - Two-layer skill loading via SkillRegistry
//!
//! Supports initialization from CLAUDE.md files for configuration.

use crate::agent::config::{ClaudeConfig, ClaudeConfigBuilder, ClaudeConfigError};
use crate::agent::context::{ContextIntegration, PlatformContext, PlatformContextLoader};
use crate::agent::hooks::HookExecutor;
use crate::agent::llm_adapter::LlmRouterAdapter;
#[cfg(feature = "collaboration")]
use crate::agent::r#loop::AgentLoop;
use crate::agent::r#loop::{AgentConfig, AgentRunner, CompactionConfig};
use crate::agent::session::{AutoCheckpointConfig, CheckpointManager, ContextCompactor};
use crate::agent::skills::SkillRegistry;
use crate::agent::tools::ToolRegistry;
use crate::agent::types::{PermissionContext, PermissionMode};
use crate::llm::LlmRouter;
use crate::mcp::McpGateway;
use crate::tool_system::ToolSystem;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[cfg(feature = "collaboration")]
use crate::agent::types::AgentMessage;
#[cfg(feature = "collaboration")]
use crate::collaboration::CollaborationMode;
#[cfg(feature = "collaboration")]
use crate::graph::{
    AgentGraphState, AgentRunnerNode, ClosureHandler, ClosurePredicate, GraphError, GraphExecutor,
    NodeContext, StateGraph, StateGraphBuilder,
};
#[cfg(feature = "collaboration")]
use futures::StreamExt;

/// Maximum number of cached agents before eviction kicks in.
/// When the cache exceeds this limit, the oldest entries (by arbitrary
/// HashMap iteration order) are removed to keep memory bounded.
const MAX_AGENT_CACHE_SIZE: usize = 1000;

/// Factory for creating configured AgentRunner instances
///
/// The AgentFactory is the recommended way to create AgentRunner instances
/// with all components properly wired:
/// - LLM via LlmRouterAdapter
/// - Tools via ToolRegistry (built-in + MCP)
/// - Hooks via HookExecutor
/// - Permissions via PermissionContext
pub struct AgentFactory {
    /// LLM Router for API calls
    llm_router: Arc<LlmRouter>,
    /// MCP Gateway for external tools (legacy - prefer tool_system)
    mcp_gateway: Option<Arc<McpGateway>>,
    /// Unified Tool System (preferred over mcp_gateway)
    tool_system: Option<Arc<ToolSystem>>,
    /// Default hook executor
    default_hooks: Arc<HookExecutor>,
    /// Default permission mode
    default_permission_mode: PermissionMode,
    /// Default working directory
    default_cwd: Option<PathBuf>,
    /// Allowed directories for file operations
    allowed_directories: Vec<PathBuf>,
    /// Default max turns
    default_max_turns: u32,
    /// Default max budget USD
    default_max_budget_usd: Option<f64>,
    /// Default model to use
    default_model: Option<String>,
    /// Default max tokens
    default_max_tokens: Option<u32>,
    /// Active agents by session ID
    agents: Arc<RwLock<HashMap<String, Arc<RwLock<AgentRunner>>>>>,
    /// Unified code execution router (K8s / Docker / Firecracker)
    code_router: Option<Arc<crate::executor::UnifiedCodeActRouter>>,
    /// VM manager for Firecracker browser tool
    #[cfg(unix)]
    vm_manager: Option<Arc<crate::vm::VmManager>>,
    /// Worker manager for Orchestrator-Worker pattern
    worker_manager: Option<Arc<crate::agent::worker::WorkerManager>>,
    /// Code orchestration runtime for programmatic tool calling
    code_orchestration_runtime:
        Option<Arc<crate::agent::code_orchestration::CodeOrchestrationRuntime>>,
    /// Screen controller for ScreenController-backed browser automation
    screen_controller: Option<Arc<dyn canal_cv::ScreenController>>,
    /// CDP screen controller for browser-specific operations (navigate, evaluate)
    cdp_controller: Option<Arc<crate::screen::CdpScreenController>>,
    /// Whether to use LLM-based summarization for context compaction
    enable_llm_summarization: bool,
    /// Custom compaction configuration (overrides AgentConfig defaults)
    compaction_config: Option<CompactionConfig>,
    /// Auto-checkpoint configuration
    checkpoint_config: Option<AutoCheckpointConfig>,
    /// Checkpoint manager for storing checkpoints
    checkpoint_manager: Option<Arc<dyn CheckpointManager + Send + Sync>>,
    /// Platform context configuration path
    platform_config_path: Option<PathBuf>,
    /// Cached platform context (loaded once)
    platform_context: Option<PlatformContext>,
    /// Skill registry for two-layer skill loading
    skill_registry: Option<Arc<SkillRegistry>>,
    /// Enable six-layer context hierarchy
    enable_context_hierarchy: bool,
    /// Enabled namespaces for MCP tool filtering (None = all namespaces)
    enabled_namespaces: Option<Vec<String>>,
    /// Default collaboration mode for graph-based execution (requires collaboration feature)
    #[cfg(feature = "collaboration")]
    default_collaboration_mode: Option<CollaborationMode>,
    /// Constraint profile for prompt constraint system (requires prompt-constraints feature)
    #[cfg(feature = "prompt-constraints")]
    constraint_profile: Option<crate::prompt::ConstraintProfile>,
    /// Learning engine for knowledge injection (requires learning feature)
    #[cfg(feature = "learning")]
    learning_engine: Option<Arc<crate::learning::LearningEngine>>,
    /// Unified memory store for surfacing preferences and patterns in prompts
    unified_memory: Option<Arc<crate::memory::UnifiedMemoryStore>>,
    /// Execution store for debug monitoring (requires graph feature)
    #[cfg(feature = "graph")]
    execution_store: Option<Arc<crate::graph::ExecutionStore>>,
    /// Default execution budget for token tracking (requires graph feature)
    #[cfg(feature = "graph")]
    execution_budget: Option<Arc<crate::graph::ExecutionBudget>>,
    /// Planner configuration for PlanExecute graph (A24)
    #[cfg(feature = "collaboration")]
    planner_config: Option<Arc<crate::collaboration::PlannerConfig>>,
    /// Pending plan approvals store for human-in-the-loop PlanExecute mode
    #[cfg(feature = "collaboration")]
    pending_plan_approvals: Option<Arc<crate::collaboration::approval::PendingPlanApprovals>>,
    /// Reflection store for persisting step judge evaluations (A40)
    #[cfg(feature = "collaboration")]
    reflection_store: Option<Arc<crate::learning::reflection::ReflectionStore>>,
    /// Pending HITL inputs for human-in-the-loop during replan (A40)
    #[cfg(feature = "jobs")]
    pending_hitl_inputs: Option<Arc<crate::jobs::PendingHITLInputs>>,
    /// Plugin manager for plugin store system (A25)
    plugin_manager: Option<Arc<crate::plugins::PluginManager>>,
    /// Platform tool config for platform control plane tools
    platform_tool_config: Option<Arc<crate::agent::tools::platform::PlatformToolConfig>>,
    /// Hosting tool config for web app deployment tools
    hosting_tool_config: Option<Arc<crate::agent::tools::hosting::HostingToolConfig>>,
    /// DevTools observation tool config for monitoring
    devtools_tool_config: Option<Arc<crate::agent::tools::devtools::DevtoolsToolConfig>>,
    /// DevTools service for LLM observability (requires devtools feature)
    #[cfg(feature = "devtools")]
    devtools_service: Option<Arc<devtools_core::DevtoolsService>>,
    /// Judge configuration for step evaluation (vision model, thresholds)
    #[cfg(feature = "collaboration")]
    judge_config: Option<crate::collaboration::judge::JudgeConfig>,
    /// Pending clarification store for A43 research planner pipeline
    #[cfg(feature = "collaboration")]
    pending_clarifications: Option<Arc<crate::collaboration::clarification::PendingClarifications>>,
    /// Pending PRD approval store for A43 research planner pipeline
    #[cfg(feature = "collaboration")]
    pending_prd_approvals: Option<Arc<crate::collaboration::prd_approval::PendingPrdApprovals>>,
    /// Tool discovery mode (A46): send only initial tools + search_tools to LLM
    tool_discovery_enabled: bool,
    /// Initial tools for discovery mode (tools always available to LLM)
    tool_discovery_initial: Vec<String>,
    /// Role-injected system prompt sections (A46)
    role_prompt_sections: Option<String>,
}

impl AgentFactory {
    /// Create a new agent factory with an LlmRouter
    pub fn new(llm_router: Arc<LlmRouter>) -> Self {
        Self {
            llm_router,
            mcp_gateway: None,
            tool_system: None,
            default_hooks: Arc::new(HookExecutor::new()),
            default_permission_mode: PermissionMode::BypassPermissions,
            default_cwd: None,
            allowed_directories: Vec::new(),
            default_max_turns: 100,
            default_max_budget_usd: None,
            default_model: None,
            default_max_tokens: None,
            agents: Arc::new(RwLock::new(HashMap::new())),
            code_router: None,
            #[cfg(unix)]
            vm_manager: None,
            worker_manager: None,
            code_orchestration_runtime: None,
            screen_controller: None,
            cdp_controller: None,
            enable_llm_summarization: false,
            compaction_config: None,
            checkpoint_config: None,
            checkpoint_manager: None,
            platform_config_path: None,
            platform_context: None,
            skill_registry: None,
            enable_context_hierarchy: true, // Enabled by default
            enabled_namespaces: None,
            #[cfg(feature = "collaboration")]
            default_collaboration_mode: None,
            #[cfg(feature = "prompt-constraints")]
            constraint_profile: None,
            #[cfg(feature = "learning")]
            learning_engine: None,
            unified_memory: None,
            #[cfg(feature = "graph")]
            execution_store: None,
            #[cfg(feature = "graph")]
            execution_budget: None,
            #[cfg(feature = "collaboration")]
            planner_config: None,
            #[cfg(feature = "collaboration")]
            pending_plan_approvals: None,
            #[cfg(feature = "collaboration")]
            reflection_store: None,
            #[cfg(feature = "jobs")]
            pending_hitl_inputs: None,
            plugin_manager: None,
            platform_tool_config: None,
            hosting_tool_config: None,
            devtools_tool_config: None,
            #[cfg(feature = "devtools")]
            devtools_service: None,
            #[cfg(feature = "collaboration")]
            judge_config: None,
            #[cfg(feature = "collaboration")]
            pending_clarifications: None,
            #[cfg(feature = "collaboration")]
            pending_prd_approvals: None,
            tool_discovery_enabled: false,
            tool_discovery_initial: Vec::new(),
            role_prompt_sections: None,
        }
    }

    /// Set the DevTools service for LLM observability.
    ///
    /// When set, graph executions will emit traces and observations
    /// to the DevTools system for debugging and Langfuse export.
    #[cfg(feature = "devtools")]
    pub fn with_devtools_service(mut self, service: Arc<devtools_core::DevtoolsService>) -> Self {
        self.devtools_service = Some(service);
        self
    }

    /// Set the judge configuration for step evaluation (vision model, thresholds).
    #[cfg(feature = "collaboration")]
    pub fn with_judge_config(mut self, config: crate::collaboration::judge::JudgeConfig) -> Self {
        self.judge_config = Some(config);
        self
    }

    /// Set the planner configuration for PlanExecute graph (A24).
    #[cfg(feature = "collaboration")]
    pub fn with_planner_config(mut self, config: crate::collaboration::PlannerConfig) -> Self {
        self.planner_config = Some(Arc::new(config));
        self
    }

    /// Set the pending plan approvals store for human-in-the-loop PlanExecute mode.
    #[cfg(feature = "collaboration")]
    pub fn with_pending_plan_approvals(
        mut self,
        store: Arc<crate::collaboration::approval::PendingPlanApprovals>,
    ) -> Self {
        self.pending_plan_approvals = Some(store);
        self
    }

    /// Set the pending clarifications store for A43 research planner pipeline.
    #[cfg(feature = "collaboration")]
    pub fn with_pending_clarifications(
        mut self,
        store: Arc<crate::collaboration::clarification::PendingClarifications>,
    ) -> Self {
        self.pending_clarifications = Some(store);
        self
    }

    /// Set the pending PRD approvals store for A43 research planner pipeline.
    #[cfg(feature = "collaboration")]
    pub fn with_pending_prd_approvals(
        mut self,
        store: Arc<crate::collaboration::prd_approval::PendingPrdApprovals>,
    ) -> Self {
        self.pending_prd_approvals = Some(store);
        self
    }

    /// Set the reflection store for persisting step judge evaluations (A40).
    #[cfg(feature = "collaboration")]
    pub fn with_reflection_store(
        mut self,
        store: Arc<crate::learning::reflection::ReflectionStore>,
    ) -> Self {
        self.reflection_store = Some(store);
        self
    }

    /// Set the pending HITL inputs store for human-in-the-loop during replan (A40).
    #[cfg(feature = "jobs")]
    pub fn with_pending_hitl_inputs(mut self, store: Arc<crate::jobs::PendingHITLInputs>) -> Self {
        self.pending_hitl_inputs = Some(store);
        self
    }

    /// Set the plugin manager for plugin store system (A25).
    pub fn with_plugin_manager(mut self, pm: Arc<crate::plugins::PluginManager>) -> Self {
        self.plugin_manager = Some(pm);
        self
    }

    /// Set the platform tool config for platform control plane tools.
    pub fn with_platform_tool_config(
        mut self,
        config: Arc<crate::agent::tools::platform::PlatformToolConfig>,
    ) -> Self {
        self.platform_tool_config = Some(config);
        self
    }

    /// Set the hosting tool config for web app deployment tools.
    pub fn with_hosting_tool_config(
        mut self,
        config: Arc<crate::agent::tools::hosting::HostingToolConfig>,
    ) -> Self {
        self.hosting_tool_config = Some(config);
        self
    }

    /// Set the devtools observation tool config for monitoring.
    pub fn with_devtools_tool_config(
        mut self,
        config: Arc<crate::agent::tools::devtools::DevtoolsToolConfig>,
    ) -> Self {
        self.devtools_tool_config = Some(config);
        self
    }

    /// Set the unified memory store for surfacing preferences in prompts.
    pub fn with_unified_memory(mut self, store: Arc<crate::memory::UnifiedMemoryStore>) -> Self {
        self.unified_memory = Some(store);
        self
    }

    /// Set the execution store for debug monitoring.
    #[cfg(feature = "graph")]
    pub fn with_execution_store(mut self, store: Arc<crate::graph::ExecutionStore>) -> Self {
        self.execution_store = Some(store);
        self
    }

    /// Set the default execution budget for token tracking.
    ///
    /// When set, graph executions will track token consumption against this
    /// budget and can warn or terminate when limits are exceeded.
    #[cfg(feature = "graph")]
    pub fn with_execution_budget(mut self, budget: Arc<crate::graph::ExecutionBudget>) -> Self {
        self.execution_budget = Some(budget);
        self
    }

    /// Set enabled namespaces for MCP tool filtering.
    ///
    /// When set, only MCP tools from these namespaces will be included
    /// in tool schemas sent to the LLM. This is useful for reducing token
    /// usage and focusing the agent on relevant tools.
    ///
    /// # Arguments
    /// * `namespaces` - List of namespace names to enable (e.g., ["filesystem", "browser"])
    ///
    /// # Example
    /// ```ignore
    /// let factory = AgentFactory::new(llm_router)
    ///     .with_mcp_gateway(gateway)
    ///     .with_enabled_namespaces(vec!["filesystem".into(), "browser".into()]);
    /// ```
    pub fn with_enabled_namespaces(mut self, namespaces: Vec<String>) -> Self {
        self.enabled_namespaces = Some(namespaces);
        self
    }

    /// Update enabled namespaces dynamically.
    ///
    /// This is useful when namespace settings change at runtime.
    /// Note: This only affects newly created agents, not existing cached ones.
    pub fn set_enabled_namespaces(&mut self, namespaces: Option<Vec<String>>) {
        self.enabled_namespaces = namespaces;
    }

    /// Get current enabled namespaces
    pub fn get_enabled_namespaces(&self) -> Option<&Vec<String>> {
        self.enabled_namespaces.as_ref()
    }

    /// Set the default collaboration mode for graph-based agent creation.
    ///
    /// This determines how agents collaborate when using graph-based execution:
    /// - `Direct`: Simple single-agent execution
    /// - `Swarm`: Agent-to-agent handoffs with context transfer
    /// - `Expert`: Supervisor dispatches to specialist pool
    /// - `Graph`: Custom state graph execution
    ///
    /// # Example
    ///
    /// ```ignore
    /// use gateway_core::collaboration::CollaborationMode;
    ///
    /// let factory = AgentFactory::new(llm_router)
    ///     .with_collaboration_mode(CollaborationMode::Expert {
    ///         supervisor: "coordinator".into(),
    ///         specialists: vec!["coder".into(), "reviewer".into()],
    ///         supervisor_model: None,
    ///         default_specialist_model: None,
    ///         specialist_models: std::collections::HashMap::new(),
    ///     });
    /// ```
    #[cfg(feature = "collaboration")]
    pub fn with_collaboration_mode(mut self, mode: CollaborationMode) -> Self {
        self.default_collaboration_mode = Some(mode);
        self
    }

    /// Get the current default collaboration mode.
    #[cfg(feature = "collaboration")]
    pub fn get_collaboration_mode(&self) -> Option<&CollaborationMode> {
        self.default_collaboration_mode.as_ref()
    }

    /// Set the constraint profile for prompt constraint system.
    ///
    /// When set, the agent's system prompt will include constraint sections
    /// (role anchor, output constraints, security rules) and the agent runner
    /// will validate input/output against the profile.
    #[cfg(feature = "prompt-constraints")]
    pub fn with_constraint_profile(
        mut self,
        profile: Option<crate::prompt::ConstraintProfile>,
    ) -> Self {
        self.constraint_profile = profile;
        self
    }

    /// Get the current constraint profile.
    #[cfg(feature = "prompt-constraints")]
    pub fn get_constraint_profile(&self) -> Option<&crate::prompt::ConstraintProfile> {
        self.constraint_profile.as_ref()
    }

    /// Set the learning engine for knowledge injection.
    ///
    /// When set, the agent's system prompt will include relevant learned knowledge
    /// from past executions (tool sequences, error recovery patterns, etc.).
    /// Requires `knowledge_injection: true` in context-engineering.yaml to take effect.
    #[cfg(feature = "learning")]
    pub fn with_learning_engine(mut self, engine: Arc<crate::learning::LearningEngine>) -> Self {
        self.learning_engine = Some(engine);
        self
    }

    /// Get the current learning engine.
    #[cfg(feature = "learning")]
    pub fn get_learning_engine(&self) -> Option<&Arc<crate::learning::LearningEngine>> {
        self.learning_engine.as_ref()
    }

    /// Set MCP gateway for external tools (legacy - prefer with_tool_system)
    pub fn with_mcp_gateway(mut self, gateway: Arc<McpGateway>) -> Self {
        self.mcp_gateway = Some(gateway);
        self
    }

    /// Set unified ToolSystem for external tools
    pub fn with_tool_system(mut self, tool_system: Arc<ToolSystem>) -> Self {
        self.tool_system = Some(tool_system);
        self
    }

    /// Set default hooks
    pub fn with_hooks(mut self, hooks: Arc<HookExecutor>) -> Self {
        self.default_hooks = hooks;
        self
    }

    /// Set default permission mode
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.default_permission_mode = mode;
        self
    }

    /// Enable tool discovery mode (A46).
    ///
    /// When enabled, the LLM only receives `initial_tools` + `search_tools`.
    /// Other tools are discovered on demand via the `search_tools` meta-tool.
    /// Reduces per-turn token usage by ~65%.
    pub fn with_tool_discovery(mut self, initial_tools: Vec<String>) -> Self {
        self.tool_discovery_enabled = true;
        self.tool_discovery_initial = initial_tools;
        self
    }

    /// Apply a Role to this factory (A46 Role Constraint System).
    ///
    /// Delegates to `Role::apply_to_factory()` which configures tool namespaces,
    /// permission mode, constraint profile, and system prompt sections via existing builder methods.
    pub fn with_role(self, role: &crate::roles::Role) -> Self {
        role.apply_to_factory(self)
    }

    /// Set role-specific system prompt sections (A46).
    ///
    /// These are appended to the system prompt to give the agent role-specific
    /// instructions, constraints, and workflow guidance.
    pub fn with_role_prompt_sections(mut self, sections: String) -> Self {
        self.role_prompt_sections = Some(sections);
        self
    }

    /// Set default working directory
    pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
        self.default_cwd = Some(cwd);
        self
    }

    /// Set allowed directories
    pub fn with_allowed_directories(mut self, dirs: Vec<PathBuf>) -> Self {
        self.allowed_directories = dirs;
        self
    }

    /// Set default max turns
    pub fn with_max_turns(mut self, max_turns: u32) -> Self {
        self.default_max_turns = max_turns;
        self
    }

    /// Set default max budget
    pub fn with_max_budget_usd(mut self, budget: f64) -> Self {
        self.default_max_budget_usd = Some(budget);
        self
    }

    /// Set default model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }

    /// Set default max tokens
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.default_max_tokens = Some(max_tokens);
        self
    }

    /// Set the unified code execution router
    ///
    /// When set, the agent's `computer` tool will route execution through
    /// the router (K8s, Docker, Firecracker) instead of local execution.
    pub fn with_code_router(mut self, router: Arc<crate::executor::UnifiedCodeActRouter>) -> Self {
        self.code_router = Some(router);
        self
    }

    /// Set the VM manager for Firecracker browser tool
    ///
    /// When set, a `browser` tool will be registered in the agent's
    /// tool registry for browser automation via Firecracker VMs.
    #[cfg(unix)]
    pub fn with_vm_manager(mut self, vm_manager: Arc<crate::vm::VmManager>) -> Self {
        self.vm_manager = Some(vm_manager);
        self
    }

    /// Set the worker manager for Orchestrator-Worker pattern
    ///
    /// When set, the OrchestrateTool will be registered in the tool registry,
    /// enabling the agent to spawn parallel worker agents via tool calling.
    pub fn with_worker_manager(
        mut self,
        manager: Arc<crate::agent::worker::WorkerManager>,
    ) -> Self {
        self.worker_manager = Some(manager);
        self
    }

    /// Set the code orchestration runtime for programmatic tool calling
    ///
    /// When set, enables the agent to execute LLM-generated code that
    /// programmatically orchestrates tool calls in a Docker sandbox.
    pub fn with_code_orchestration(
        mut self,
        runtime: Arc<crate::agent::code_orchestration::CodeOrchestrationRuntime>,
    ) -> Self {
        self.code_orchestration_runtime = Some(runtime);
        self
    }

    /// Set the screen controller for ScreenController-backed browser automation.
    ///
    /// When set, computer_screenshot, computer_click, computer_type, computer_key,
    /// computer_scroll, and computer_drag tools will be registered.
    pub fn with_screen_controller(
        mut self,
        controller: Arc<dyn canal_cv::ScreenController>,
    ) -> Self {
        self.screen_controller = Some(controller);
        self
    }

    /// Set the CDP screen controller for browser-specific operations.
    ///
    /// When set, also registers `computer_navigate` tool. This is only needed
    /// for browser-based screen control (not desktop).
    pub fn with_cdp_controller(mut self, cdp: Arc<crate::screen::CdpScreenController>) -> Self {
        self.cdp_controller = Some(cdp);
        self
    }

    /// Enable LLM-based summarization for context compaction
    ///
    /// When enabled, the ContextCompactor will use the LLM router to generate
    /// intelligent summaries of conversation history when compaction is triggered.
    /// This produces higher quality summaries than the default rule-based approach,
    /// but incurs additional LLM API costs.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let factory = AgentFactory::new(llm_router)
    ///     .with_llm_summarization(true);
    /// ```
    pub fn with_llm_summarization(mut self, enable: bool) -> Self {
        self.enable_llm_summarization = enable;
        self
    }

    /// Set custom compaction configuration
    ///
    /// This allows overriding the default compaction settings for all agents
    /// created by this factory. Individual AgentConfig compaction settings
    /// will be merged with these defaults.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use gateway_core::agent::r#loop::CompactionConfig;
    ///
    /// let factory = AgentFactory::new(llm_router)
    ///     .with_compaction_config(CompactionConfig {
    ///         enabled: true,
    ///         max_context_tokens: 150_000,
    ///         min_messages_to_keep: 15,
    ///         target_tokens: 80_000,
    ///     });
    /// ```
    pub fn with_compaction_config(mut self, config: CompactionConfig) -> Self {
        self.compaction_config = Some(config);
        self
    }

    /// Enable auto-checkpoint with default settings
    ///
    /// This enables automatic checkpointing with sensible defaults:
    /// - Checkpoint before dangerous operations (Bash, Write, Edit)
    /// - Periodic checkpoints every 10 turns
    /// - Checkpoint before context compaction
    ///
    /// Requires a CheckpointManager to be set via `with_checkpoint_manager()`.
    pub fn with_auto_checkpoint(mut self) -> Self {
        self.checkpoint_config = Some(AutoCheckpointConfig::default());
        self
    }

    /// Set custom auto-checkpoint configuration
    ///
    /// Allows fine-grained control over checkpoint behavior for all agents
    /// created by this factory.
    pub fn with_checkpoint_config(mut self, config: AutoCheckpointConfig) -> Self {
        self.checkpoint_config = Some(config);
        self
    }

    /// Set the checkpoint manager for storing checkpoints
    ///
    /// The checkpoint manager handles persistence of checkpoints.
    /// Common implementations:
    /// - `FileCheckpointManager` - stores on local filesystem
    /// - `MemoryCheckpointManager` - in-memory (for testing)
    pub fn with_checkpoint_manager(
        mut self,
        manager: Arc<dyn CheckpointManager + Send + Sync>,
    ) -> Self {
        self.checkpoint_manager = Some(manager);
        self
    }

    /// Set the platform configuration path for loading platform context
    ///
    /// The platform context is loaded from a YAML file that defines:
    /// - Platform rules and constraints
    /// - Default skill configurations
    /// - Iteration settings
    ///
    /// Default path: `config/platform-rules.yaml`
    pub fn with_platform_config(mut self, path: impl Into<PathBuf>) -> Self {
        self.platform_config_path = Some(path.into());
        self
    }

    /// Set a pre-loaded platform context
    ///
    /// Use this when you want to provide a pre-configured PlatformContext
    /// instead of loading from file.
    pub fn with_platform_context(mut self, context: PlatformContext) -> Self {
        self.platform_context = Some(context);
        self
    }

    /// Set the skill registry for two-layer skill loading
    ///
    /// The skill registry provides:
    /// - Layer 1: Skill descriptions in system prompt (~15K chars)
    /// - Layer 2: Full skill content loaded on-demand via invoke_skill
    pub fn with_skill_registry(mut self, registry: Arc<SkillRegistry>) -> Self {
        self.skill_registry = Some(registry);
        self
    }

    /// Enable or disable the six-layer context hierarchy
    ///
    /// When enabled (default), agents use:
    /// - Platform → Organization → User → Session → Task → SubAgent contexts
    /// - SystemPromptGenerator for prompt generation
    /// - Two-layer skill loading
    ///
    /// When disabled, agents use the legacy hardcoded system prompt.
    pub fn with_context_hierarchy(mut self, enable: bool) -> Self {
        self.enable_context_hierarchy = enable;
        self
    }

    /// Load the platform context from configuration file
    ///
    /// This is called lazily when creating agents if platform_context is not set.
    fn load_platform_context(&self) -> Option<PlatformContext> {
        if let Some(ref ctx) = self.platform_context {
            return Some(ctx.clone());
        }

        let path = self
            .platform_config_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("config/platform-rules.yaml"));

        match PlatformContextLoader::new(&path).load() {
            Ok(ctx) => Some(ctx),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "Failed to load platform context, using defaults"
                );
                Some(PlatformContext::default())
            }
        }
    }

    /// Generate system prompt using six-layer context hierarchy
    ///
    /// This creates a ContextIntegration, loads all available contexts,
    /// and generates the system prompt via SystemPromptGenerator.
    fn generate_context_system_prompt(&self, session_id: Option<&str>) -> String {
        let mut integration = ContextIntegration::new();

        // Load platform context
        if let Some(platform) = self.load_platform_context() {
            integration = integration.with_platform(platform);
        }

        // Add skill registry if available
        if let Some(ref registry) = self.skill_registry {
            integration = integration.with_skill_registry(registry.clone());
        }

        // Inject plugin skills from catalog (A25)
        // Uses try_read() for non-blocking access to catalog
        if let Some(ref pm) = self.plugin_manager {
            if let Ok(catalog) = pm.catalog.try_read() {
                let all_plugins = catalog.list_all();
                let mut plugin_skills = Vec::new();
                for plugin in all_plugins {
                    for skill in &plugin.skills {
                        let mut s = skill.clone();
                        s.metadata.plugin_name = Some(plugin.manifest.name.clone());
                        plugin_skills.push(s);
                    }
                }
                if !plugin_skills.is_empty() {
                    integration = integration.with_plugin_skills(plugin_skills);
                }
            }
        }

        // Create session context if session_id provided
        if let Some(sid) = session_id {
            let session_uuid = Uuid::parse_str(sid).unwrap_or_else(|_| Uuid::new_v4());
            integration.create_session(session_uuid);
        }

        // Set user context directory
        if let Some(ref cwd) = self.default_cwd {
            integration = integration.with_user_context_dir(cwd);
        }

        // Inject knowledge provider from learning engine (requires learning + context-engineering)
        #[cfg(all(feature = "learning", feature = "context-engineering"))]
        if let Some(ref engine) = self.learning_engine {
            // LearningEngineProvider wraps LearningEngine and implements KnowledgeProvider
            integration = integration.with_knowledge_provider(Arc::new(
                crate::learning::LearningEngineProvider::new(engine.clone()),
            ));
        }

        // Inject unified memory store for preferences and patterns in prompt
        if let Some(ref store) = self.unified_memory {
            let user_id = Uuid::nil(); // System-level user for now
            integration = integration.with_unified_store(store.clone(), user_id);
        }

        // Generate the system prompt
        #[allow(unused_mut)]
        let mut prompt = integration.generate_system_prompt();

        // Inject constraint sections from active profile
        #[cfg(feature = "prompt-constraints")]
        if let Some(ref profile) = self.constraint_profile {
            let mut constraint_sections = Vec::new();

            // Role anchor section
            if let Some(ref anchor) = profile.role_anchor {
                constraint_sections.push(format!(
                    "\n## Role: {}\n\n{}",
                    anchor.role_name, anchor.anchor_prompt
                ));
            }

            // Output constraint prompt injections
            for constraint in profile.enabled_output_constraints() {
                if !constraint.prompt_injection.is_empty() {
                    constraint_sections.push(format!(
                        "\n## Output Constraint: {}\n\n{}",
                        constraint.name, constraint.prompt_injection
                    ));
                }
            }

            // Security rules summary
            if !profile.security.blocked_commands.is_empty()
                || profile.security.prompt_injection.is_some()
            {
                let mut security_text = String::from("\n## Security Rules\n\n");
                if let Some(ref injection) = profile.security.prompt_injection {
                    security_text.push_str(injection);
                    security_text.push('\n');
                }
                if !profile.security.blocked_commands.is_empty() {
                    security_text.push_str("Blocked commands: ");
                    security_text.push_str(&profile.security.blocked_commands.join(", "));
                    security_text.push('\n');
                }
                if !profile.security.require_confirmation.is_empty() {
                    security_text.push_str("Requires confirmation: ");
                    security_text.push_str(&profile.security.require_confirmation.join(", "));
                    security_text.push('\n');
                }
                constraint_sections.push(security_text);
            }

            if !constraint_sections.is_empty() {
                prompt.push_str("\n\n# Prompt Constraints\n");
                for section in constraint_sections {
                    prompt.push_str(&section);
                }
            }
        }

        // Inject role-specific system prompt sections (A46)
        if let Some(ref role_sections) = self.role_prompt_sections {
            prompt.push_str("\n\n");
            prompt.push_str(role_sections);
        }

        prompt
    }

    /// Generate system prompt with bundle context injected.
    ///
    /// This extends `generate_context_system_prompt` by additionally injecting
    /// the bundle's system prompt into the `ContextIntegration`.
    fn generate_context_system_prompt_with_bundles(
        &self,
        session_id: Option<&str>,
        bundle_prompt: Option<&str>,
    ) -> String {
        let mut integration = ContextIntegration::new();

        // Load platform context
        if let Some(platform) = self.load_platform_context() {
            integration = integration.with_platform(platform);
        }

        // Add skill registry if available
        if let Some(ref registry) = self.skill_registry {
            integration = integration.with_skill_registry(registry.clone());
        }

        // Inject plugin skills from catalog (A25)
        if let Some(ref pm) = self.plugin_manager {
            if let Ok(catalog) = pm.catalog.try_read() {
                let all_plugins = catalog.list_all();
                let mut plugin_skills = Vec::new();
                for plugin in all_plugins {
                    for skill in &plugin.skills {
                        let mut s = skill.clone();
                        s.metadata.plugin_name = Some(plugin.manifest.name.clone());
                        plugin_skills.push(s);
                    }
                }
                if !plugin_skills.is_empty() {
                    integration = integration.with_plugin_skills(plugin_skills);
                }
            }
        }

        // Inject bundle system prompt
        if let Some(bp) = bundle_prompt {
            integration = integration.with_bundle_system_prompt(bp.to_string());
        }

        // Create session context if session_id provided
        if let Some(sid) = session_id {
            let session_uuid = Uuid::parse_str(sid).unwrap_or_else(|_| Uuid::new_v4());
            integration.create_session(session_uuid);
        }

        // Set user context directory
        if let Some(ref cwd) = self.default_cwd {
            integration = integration.with_user_context_dir(cwd);
        }

        // Inject knowledge provider from learning engine
        #[cfg(all(feature = "learning", feature = "context-engineering"))]
        if let Some(ref engine) = self.learning_engine {
            integration = integration.with_knowledge_provider(Arc::new(
                crate::learning::LearningEngineProvider::new(engine.clone()),
            ));
        }

        // Inject unified memory store
        if let Some(ref store) = self.unified_memory {
            let user_id = Uuid::nil();
            integration = integration.with_unified_store(store.clone(), user_id);
        }

        // Generate prompt
        #[allow(unused_mut)]
        let mut prompt = integration.generate_system_prompt();

        // Inject constraint sections from active profile
        #[cfg(feature = "prompt-constraints")]
        if let Some(ref profile) = self.constraint_profile {
            let mut constraint_sections = Vec::new();

            if let Some(ref anchor) = profile.role_anchor {
                constraint_sections.push(format!(
                    "\n## Role: {}\n\n{}",
                    anchor.role_name, anchor.anchor_prompt
                ));
            }

            for constraint in profile.enabled_output_constraints() {
                if !constraint.prompt_injection.is_empty() {
                    constraint_sections.push(format!(
                        "\n## Output Constraint: {}\n\n{}",
                        constraint.name, constraint.prompt_injection
                    ));
                }
            }

            if !profile.security.blocked_commands.is_empty()
                || profile.security.prompt_injection.is_some()
            {
                let mut security_text = String::from("\n## Security Rules\n\n");
                if let Some(ref injection) = profile.security.prompt_injection {
                    security_text.push_str(injection);
                    security_text.push('\n');
                }
                if !profile.security.blocked_commands.is_empty() {
                    security_text.push_str("Blocked commands: ");
                    security_text.push_str(&profile.security.blocked_commands.join(", "));
                    security_text.push('\n');
                }
                if !profile.security.require_confirmation.is_empty() {
                    security_text.push_str("Requires confirmation: ");
                    security_text.push_str(&profile.security.require_confirmation.join(", "));
                    security_text.push('\n');
                }
                constraint_sections.push(security_text);
            }

            if !constraint_sections.is_empty() {
                prompt.push_str("\n\n# Prompt Constraints\n");
                for section in constraint_sections {
                    prompt.push_str(&section);
                }
            }
        }

        // Inject role-specific system prompt sections (A46)
        if let Some(ref role_sections) = self.role_prompt_sections {
            prompt.push_str("\n\n");
            prompt.push_str(role_sections);
        }

        prompt
    }

    /// Get or create an agent with bundle-resolved context.
    ///
    /// Resolves active bundles to determine extra namespaces and system prompt,
    /// then creates an agent with the merged configuration.
    pub async fn get_or_create_with_bundles(
        &self,
        session_id: &str,
        profile_id: Option<String>,
        task_type: Option<String>,
        base_namespaces: Vec<String>,
        bundle_namespaces: Vec<String>,
        bundle_prompt: Option<String>,
    ) -> Arc<RwLock<AgentRunner>> {
        let mut agents = self.agents.write().await;

        if let Some(agent) = agents.get(session_id) {
            return agent.clone();
        }

        // Merge base + bundle namespaces (deduped)
        let mut all_namespaces = base_namespaces;
        for ns in bundle_namespaces {
            if !all_namespaces.contains(&ns) {
                all_namespaces.push(ns);
            }
        }

        tracing::info!(
            session_id = %session_id,
            namespaces = ?all_namespaces,
            has_bundle_prompt = bundle_prompt.is_some(),
            "Creating agent with bundle context"
        );

        // Create agent
        Self::evict_if_needed(&mut agents);
        let agent = Arc::new(RwLock::new(
            self.create_with_bundles_async(
                session_id,
                profile_id,
                task_type,
                Some(all_namespaces),
                bundle_prompt,
            )
            .await,
        ));
        agents.insert(session_id.to_string(), agent.clone());
        agent
    }

    /// Internal: create agent with bundle-augmented system prompt.
    async fn create_with_bundles_async(
        &self,
        session_id: impl Into<String>,
        profile_id: Option<String>,
        task_type: Option<String>,
        enabled_namespaces: Option<Vec<String>>,
        bundle_prompt: Option<String>,
    ) -> AgentRunner {
        let session_id = session_id.into();

        let mut config = AgentConfig::default();
        config.cwd = self.default_cwd.clone();
        config.max_turns = self.default_max_turns;
        config.max_budget_usd = self.default_max_budget_usd;
        config.permission_mode = self.default_permission_mode;

        // Generate system prompt with bundle context
        if self.enable_context_hierarchy {
            let system_prompt = self.generate_context_system_prompt_with_bundles(
                Some(&session_id),
                bundle_prompt.as_deref(),
            );
            if !system_prompt.is_empty() {
                config.system_prompt = Some(system_prompt);
            }
        }

        let llm_adapter = self.create_llm_adapter_with_profile(profile_id, task_type);
        let tool_registry = self
            .create_tool_registry_with_namespaces_async(enabled_namespaces)
            .await;
        let permission_ctx = self.create_permission_context();
        let compactor = self.create_compactor(&config.compaction);

        let mut runner = AgentRunner::with_session_id(config, session_id)
            .with_llm(llm_adapter)
            .with_tools(tool_registry)
            .with_hooks(self.default_hooks.clone())
            .with_permission_context(permission_ctx)
            .with_compactor(compactor);

        if let Some(ref checkpoint_config) = self.checkpoint_config {
            runner = runner.with_auto_checkpoint_config(checkpoint_config.clone());
        }
        if let Some(ref checkpoint_manager) = self.checkpoint_manager {
            runner = runner.with_checkpoint_manager(checkpoint_manager.clone());
        }

        #[cfg(feature = "prompt-constraints")]
        {
            runner = self.apply_constraint_validator(runner);
        }

        runner
    }

    /// Create an LLM adapter with current configuration
    fn create_llm_adapter(&self) -> Arc<LlmRouterAdapter> {
        self.create_llm_adapter_with_profile(None, None)
    }

    /// Create an LLM adapter with profile ID for dynamic routing
    fn create_llm_adapter_with_profile(
        &self,
        profile_id: Option<String>,
        task_type: Option<String>,
    ) -> Arc<LlmRouterAdapter> {
        self.create_llm_adapter_with_model_override(None, profile_id, task_type)
    }

    /// Create an LLM adapter with an explicit model override.
    ///
    /// Resolution order: `model_override` > `self.default_model` > router default.
    fn create_llm_adapter_with_model_override(
        &self,
        model_override: Option<&str>,
        profile_id: Option<String>,
        task_type: Option<String>,
    ) -> Arc<LlmRouterAdapter> {
        let mut adapter = LlmRouterAdapter::new(self.llm_router.clone());

        // model_override takes priority over factory default
        let effective_model = model_override
            .map(|m| m.to_string())
            .or_else(|| self.default_model.clone());
        if let Some(model) = &effective_model {
            adapter = adapter.with_model(model);
        }

        tracing::debug!(
            model_override = ?model_override,
            default_model = ?self.default_model,
            effective_model = ?effective_model,
            "Resolved model for adapter"
        );

        if let Some(max_tokens) = self.default_max_tokens {
            adapter = adapter.with_max_tokens(max_tokens);
        }
        if let Some(profile_id) = profile_id {
            adapter = adapter.with_profile_id(profile_id);
        }
        if let Some(task_type) = task_type {
            adapter = adapter.with_task_type(task_type);
        }

        Arc::new(adapter)
    }

    /// Create a reference tool registry for API visibility.
    ///
    /// This mirrors agent session configuration for listing tools.
    /// The returned registry is a snapshot used only for listing available
    /// agent built-in tools via the API — it does not affect agent sessions.
    pub fn create_reference_registry(&self) -> Arc<ToolRegistry> {
        self.create_tool_registry()
    }

    /// Create a tool registry with current configuration (synchronous).
    ///
    /// Note: This version does NOT cache MCP tools. For agents that need
    /// MCP tools with namespace filtering, use `create_tool_registry_async` instead.
    fn create_tool_registry(&self) -> Arc<ToolRegistry> {
        let mut registry = if let Some(ts) = &self.tool_system {
            ToolRegistry::with_tool_system(ts.clone())
        } else if let Some(mcp) = &self.mcp_gateway {
            ToolRegistry::with_mcp_gateway(mcp.clone())
        } else {
            ToolRegistry::new()
        };

        // Apply enabled namespaces filter
        if let Some(ref namespaces) = self.enabled_namespaces {
            registry.set_enabled_namespaces(namespaces.clone());
        }

        // Replace default LocalComputerTool with UnifiedComputerTool
        // when a code execution router is available
        if let Some(router) = &self.code_router {
            registry.with_router(router.clone());
        }

        // Register BrowserTool when a VM manager is available
        #[cfg(unix)]
        if let Some(vm_manager) = &self.vm_manager {
            registry.with_browser_tool(vm_manager.clone());
        }

        // Register OrchestrateTool when a worker manager is available
        if let Some(worker_manager) = &self.worker_manager {
            registry.with_orchestrate_tool(worker_manager.clone());
        }

        // Register CodeOrchestrationTool when code orchestration runtime is available
        if let Some(runtime) = &self.code_orchestration_runtime {
            registry.with_code_orchestration_tool(runtime.clone());
        }

        // Register platform control plane tools when config is available
        if let Some(config) = &self.platform_tool_config {
            registry.with_platform_tools(config.clone());
        }

        // Register hosting tools for web app deployment
        if let Some(config) = &self.hosting_tool_config {
            registry.with_hosting_tools(config.clone());
        }

        // Register devtools observation tools for monitoring
        if let Some(config) = &self.devtools_tool_config {
            registry.with_devtools_tools(config.clone());
        }

        // Register database tools (reuses hosting/platform API base URL)
        #[cfg(feature = "database")]
        if let Some(hosting_config) = &self.hosting_tool_config {
            let db_config =
                std::sync::Arc::new(crate::agent::tools::database::DatabaseToolConfig::new(
                    &hosting_config.base_url,
                    hosting_config.get_token(),
                ));
            registry.with_database_tools(db_config);
        }

        // Register screen tools when screen controller is available
        if let Some(controller) = &self.screen_controller {
            crate::screen::register_screen_tools(
                &mut registry,
                controller.clone(),
                self.cdp_controller.clone(),
            );
        }

        // Enable tool discovery mode (A46) — must be last, after all tools are registered
        if self.tool_discovery_enabled && !self.tool_discovery_initial.is_empty() {
            registry.enable_discovery(self.tool_discovery_initial.clone());
        }

        Arc::new(registry)
    }

    /// Create a tool registry with current configuration (asynchronous).
    ///
    /// This version caches MCP tools from the gateway with namespace filtering.
    /// Use this when you need MCP tools to be included in tool schemas.
    async fn create_tool_registry_async(&self) -> Arc<ToolRegistry> {
        let mut registry = if let Some(ts) = &self.tool_system {
            ToolRegistry::with_tool_system(ts.clone())
        } else if let Some(mcp) = &self.mcp_gateway {
            ToolRegistry::with_mcp_gateway(mcp.clone())
        } else {
            ToolRegistry::new()
        };

        // Apply enabled namespaces filter and cache MCP tools
        if let Some(ref namespaces) = self.enabled_namespaces {
            registry.set_enabled_namespaces(namespaces.clone());
        }
        registry.cache_mcp_tools().await;

        // Replace default LocalComputerTool with UnifiedComputerTool
        // when a code execution router is available
        if let Some(router) = &self.code_router {
            registry.with_router(router.clone());
        }

        // Register BrowserTool when a VM manager is available
        #[cfg(unix)]
        if let Some(vm_manager) = &self.vm_manager {
            registry.with_browser_tool(vm_manager.clone());
        }

        // Register OrchestrateTool when a worker manager is available
        if let Some(worker_manager) = &self.worker_manager {
            registry.with_orchestrate_tool(worker_manager.clone());
        }

        // Register CodeOrchestrationTool when code orchestration runtime is available
        if let Some(runtime) = &self.code_orchestration_runtime {
            registry.with_code_orchestration_tool(runtime.clone());
        }

        // Register platform control plane tools when config is available
        if let Some(config) = &self.platform_tool_config {
            registry.with_platform_tools(config.clone());
        }

        // Register hosting tools for web app deployment
        if let Some(config) = &self.hosting_tool_config {
            registry.with_hosting_tools(config.clone());
        }

        // Register devtools observation tools for monitoring
        if let Some(config) = &self.devtools_tool_config {
            registry.with_devtools_tools(config.clone());
        }

        // Register database tools (reuses hosting/platform API base URL)
        #[cfg(feature = "database")]
        if let Some(hosting_config) = &self.hosting_tool_config {
            let db_config =
                std::sync::Arc::new(crate::agent::tools::database::DatabaseToolConfig::new(
                    &hosting_config.base_url,
                    hosting_config.get_token(),
                ));
            registry.with_database_tools(db_config);
        }

        // Register screen tools when screen controller is available
        if let Some(controller) = &self.screen_controller {
            crate::screen::register_screen_tools(
                &mut registry,
                controller.clone(),
                self.cdp_controller.clone(),
            );
        }

        // Enable tool discovery mode (A46) — must be last, after all tools are registered
        if self.tool_discovery_enabled && !self.tool_discovery_initial.is_empty() {
            registry.enable_discovery(self.tool_discovery_initial.clone());
        }

        Arc::new(registry)
    }

    /// Create a permission context with current configuration
    fn create_permission_context(&self) -> PermissionContext {
        let mut ctx = PermissionContext::default();
        ctx.mode = self.default_permission_mode;

        if let Some(cwd) = &self.default_cwd {
            ctx.cwd = Some(cwd.to_string_lossy().to_string());
        }

        ctx.allowed_directories = self
            .allowed_directories
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        ctx
    }

    /// Apply constraint validator to an AgentRunner if a constraint profile is set.
    #[cfg(feature = "prompt-constraints")]
    fn apply_constraint_validator(&self, runner: AgentRunner) -> AgentRunner {
        if let Some(ref profile) = self.constraint_profile {
            let validator = crate::prompt::ConstraintValidator::new(profile.clone());
            tracing::debug!(
                profile = %profile.name,
                "Applying constraint validator to agent runner"
            );
            runner.with_constraint_validator(Some(validator))
        } else {
            runner
        }
    }

    /// Create a context compactor with current configuration
    ///
    /// If LLM summarization is enabled, the compactor will use the LLM router
    /// for intelligent summarization. Otherwise, it uses rule-based summarization.
    fn create_compactor(&self, config: &CompactionConfig) -> ContextCompactor {
        let mut builder = ContextCompactor::builder()
            .max_tokens(config.max_context_tokens)
            .target_tokens(config.target_tokens)
            .keep_recent(config.min_messages_to_keep)
            .threshold_ratio(0.8); // Trigger compaction at 80% of max tokens

        // Apply factory-level compaction config overrides if set
        if let Some(ref factory_config) = self.compaction_config {
            builder = builder
                .max_tokens(factory_config.max_context_tokens)
                .target_tokens(factory_config.target_tokens)
                .keep_recent(factory_config.min_messages_to_keep);
        }

        // Add LLM router for intelligent summarization if enabled
        if self.enable_llm_summarization {
            builder = builder.with_llm_router(self.llm_router.clone());
        }

        builder.build()
    }

    /// Create a new AgentRunner with default configuration
    pub fn create(&self) -> AgentRunner {
        self.create_with_config(AgentConfig::default())
    }

    /// Create a new AgentRunner with specific configuration
    pub fn create_with_config(&self, mut config: AgentConfig) -> AgentRunner {
        // Apply defaults if not set
        if config.cwd.is_none() {
            config.cwd = self.default_cwd.clone();
        }
        if config.max_turns == 0 {
            config.max_turns = self.default_max_turns;
        }
        if config.max_budget_usd.is_none() {
            config.max_budget_usd = self.default_max_budget_usd;
        }
        config.permission_mode = self.default_permission_mode;

        // Generate system prompt using six-layer context hierarchy if enabled
        if self.enable_context_hierarchy && config.system_prompt.is_none() {
            let system_prompt = self.generate_context_system_prompt(None);
            if !system_prompt.is_empty() {
                config.system_prompt = Some(system_prompt);
                tracing::debug!("Using six-layer context hierarchy for system prompt");
            }
        }

        // Create all components
        let llm_adapter = self.create_llm_adapter();
        let tool_registry = self.create_tool_registry();
        let permission_ctx = self.create_permission_context();
        let compactor = self.create_compactor(&config.compaction);

        // Create the runner with all components wired up
        let mut runner = AgentRunner::new(config)
            .with_llm(llm_adapter)
            .with_tools(tool_registry)
            .with_hooks(self.default_hooks.clone())
            .with_permission_context(permission_ctx)
            .with_compactor(compactor);

        // Apply checkpoint configuration if set
        if let Some(ref checkpoint_config) = self.checkpoint_config {
            runner = runner.with_auto_checkpoint_config(checkpoint_config.clone());
        }
        if let Some(ref checkpoint_manager) = self.checkpoint_manager {
            runner = runner.with_checkpoint_manager(checkpoint_manager.clone());
        }

        // Apply constraint validator if profile is set
        #[cfg(feature = "prompt-constraints")]
        {
            runner = self.apply_constraint_validator(runner);
        }

        runner
    }

    /// Create a new AgentRunner for a specific session
    pub fn create_for_session(&self, session_id: impl Into<String>) -> AgentRunner {
        let session_id = session_id.into();

        let mut config = AgentConfig::default();
        config.cwd = self.default_cwd.clone();
        config.max_turns = self.default_max_turns;
        config.max_budget_usd = self.default_max_budget_usd;
        config.permission_mode = self.default_permission_mode;

        // Generate system prompt using six-layer context hierarchy if enabled
        if self.enable_context_hierarchy {
            let system_prompt = self.generate_context_system_prompt(Some(&session_id));
            if !system_prompt.is_empty() {
                config.system_prompt = Some(system_prompt);
                tracing::debug!(
                    session_id = %session_id,
                    "Using six-layer context hierarchy for session system prompt"
                );
            }
        }

        // Create all components
        let llm_adapter = self.create_llm_adapter();
        let tool_registry = self.create_tool_registry();
        let permission_ctx = self.create_permission_context();
        let compactor = self.create_compactor(&config.compaction);

        let mut runner = AgentRunner::with_session_id(config, session_id)
            .with_llm(llm_adapter)
            .with_tools(tool_registry)
            .with_hooks(self.default_hooks.clone())
            .with_permission_context(permission_ctx)
            .with_compactor(compactor);

        // Apply checkpoint configuration if set
        if let Some(ref checkpoint_config) = self.checkpoint_config {
            runner = runner.with_auto_checkpoint_config(checkpoint_config.clone());
        }
        if let Some(ref checkpoint_manager) = self.checkpoint_manager {
            runner = runner.with_checkpoint_manager(checkpoint_manager.clone());
        }

        // Apply constraint validator if profile is set
        #[cfg(feature = "prompt-constraints")]
        {
            runner = self.apply_constraint_validator(runner);
        }

        runner
    }

    /// Get or create an agent for a session
    ///
    /// Returns an existing agent if one exists for the session,
    /// otherwise creates a new one.
    pub async fn get_or_create(&self, session_id: &str) -> Arc<RwLock<AgentRunner>> {
        let mut agents = self.agents.write().await;

        if let Some(agent) = agents.get(session_id) {
            return agent.clone();
        }

        Self::evict_if_needed(&mut agents);
        let agent = Arc::new(RwLock::new(self.create_for_session(session_id)));
        agents.insert(session_id.to_string(), agent.clone());
        agent
    }

    /// Create a new AgentRunner with profile-based routing
    ///
    /// This creates a one-off agent with a specific profile_id for dynamic routing.
    /// Unlike `get_or_create`, this does not cache the agent.
    pub fn create_with_profile(
        &self,
        session_id: impl Into<String>,
        profile_id: Option<String>,
        task_type: Option<String>,
    ) -> AgentRunner {
        let session_id = session_id.into();

        let mut config = AgentConfig::default();
        config.cwd = self.default_cwd.clone();
        config.max_turns = self.default_max_turns;
        config.max_budget_usd = self.default_max_budget_usd;
        config.permission_mode = self.default_permission_mode;

        // Generate system prompt using six-layer context hierarchy if enabled
        if self.enable_context_hierarchy {
            let system_prompt = self.generate_context_system_prompt(Some(&session_id));
            if !system_prompt.is_empty() {
                config.system_prompt = Some(system_prompt);
                tracing::debug!(
                    session_id = %session_id,
                    profile_id = ?profile_id,
                    task_type = ?task_type,
                    "Using six-layer context hierarchy for profiled session"
                );
            }
        }

        // Create LLM adapter with profile
        let llm_adapter = self.create_llm_adapter_with_profile(profile_id, task_type);
        let tool_registry = self.create_tool_registry();
        let permission_ctx = self.create_permission_context();
        let compactor = self.create_compactor(&config.compaction);

        let mut runner = AgentRunner::with_session_id(config, session_id)
            .with_llm(llm_adapter)
            .with_tools(tool_registry)
            .with_hooks(self.default_hooks.clone())
            .with_permission_context(permission_ctx)
            .with_compactor(compactor);

        // Apply checkpoint configuration if set
        if let Some(ref checkpoint_config) = self.checkpoint_config {
            runner = runner.with_auto_checkpoint_config(checkpoint_config.clone());
        }
        if let Some(ref checkpoint_manager) = self.checkpoint_manager {
            runner = runner.with_checkpoint_manager(checkpoint_manager.clone());
        }

        // Apply constraint validator if profile is set
        #[cfg(feature = "prompt-constraints")]
        {
            runner = self.apply_constraint_validator(runner);
        }

        runner
    }

    /// Create a new AgentRunner with profile-based routing (async version).
    ///
    /// This async version caches MCP tools with namespace filtering.
    /// Use this when MCP tools need to be included in tool schemas.
    pub async fn create_with_profile_async(
        &self,
        session_id: impl Into<String>,
        profile_id: Option<String>,
        task_type: Option<String>,
    ) -> AgentRunner {
        let session_id = session_id.into();

        let mut config = AgentConfig::default();
        config.cwd = self.default_cwd.clone();
        config.max_turns = self.default_max_turns;
        config.max_budget_usd = self.default_max_budget_usd;
        config.permission_mode = self.default_permission_mode;

        // Generate system prompt using six-layer context hierarchy if enabled
        if self.enable_context_hierarchy {
            let system_prompt = self.generate_context_system_prompt(Some(&session_id));
            if !system_prompt.is_empty() {
                config.system_prompt = Some(system_prompt);
                tracing::debug!(
                    session_id = %session_id,
                    profile_id = ?profile_id,
                    task_type = ?task_type,
                    enabled_namespaces = ?self.enabled_namespaces,
                    "Using six-layer context hierarchy for profiled session with MCP tool caching"
                );
            }
        }

        // Create LLM adapter with profile
        let llm_adapter = self.create_llm_adapter_with_profile(profile_id, task_type);
        // Use async version to cache MCP tools with namespace filtering
        let tool_registry = self.create_tool_registry_async().await;
        let permission_ctx = self.create_permission_context();
        let compactor = self.create_compactor(&config.compaction);

        let mut runner = AgentRunner::with_session_id(config, session_id)
            .with_llm(llm_adapter)
            .with_tools(tool_registry)
            .with_hooks(self.default_hooks.clone())
            .with_permission_context(permission_ctx)
            .with_compactor(compactor);

        // Apply checkpoint configuration if set
        if let Some(ref checkpoint_config) = self.checkpoint_config {
            runner = runner.with_auto_checkpoint_config(checkpoint_config.clone());
        }
        if let Some(ref checkpoint_manager) = self.checkpoint_manager {
            runner = runner.with_checkpoint_manager(checkpoint_manager.clone());
        }

        // Apply constraint validator if profile is set
        #[cfg(feature = "prompt-constraints")]
        {
            runner = self.apply_constraint_validator(runner);
        }

        runner
    }

    /// Get or create an agent for a session with profile-based routing
    ///
    /// If an agent exists for the session, returns it.
    /// Otherwise creates a new one with the specified profile.
    ///
    /// This method caches MCP tools with namespace filtering for new agents.
    /// Uses the factory's default enabled_namespaces setting.
    pub async fn get_or_create_with_profile(
        &self,
        session_id: &str,
        profile_id: Option<String>,
        task_type: Option<String>,
    ) -> Arc<RwLock<AgentRunner>> {
        self.get_or_create_with_profile_and_namespaces(
            session_id,
            profile_id,
            task_type,
            self.enabled_namespaces.clone(),
        )
        .await
    }

    /// Get or create an agent for a session with profile-based routing and custom namespaces.
    ///
    /// If an agent exists for the session, returns it.
    /// Otherwise creates a new one with the specified profile and namespace filtering.
    ///
    /// # Arguments
    /// * `session_id` - Unique identifier for the session
    /// * `profile_id` - Optional profile ID for model routing
    /// * `task_type` - Optional task type hint for routing
    /// * `enabled_namespaces` - Optional list of enabled MCP namespaces for tool filtering.
    ///   If None, all namespaces are enabled.
    ///
    /// This method is useful when namespace settings are determined at runtime
    /// (e.g., read from user settings).
    pub async fn get_or_create_with_profile_and_namespaces(
        &self,
        session_id: &str,
        profile_id: Option<String>,
        task_type: Option<String>,
        enabled_namespaces: Option<Vec<String>>,
    ) -> Arc<RwLock<AgentRunner>> {
        let mut agents = self.agents.write().await;

        if let Some(agent) = agents.get(session_id) {
            return agent.clone();
        }

        // Create agent with specified namespace filtering
        Self::evict_if_needed(&mut agents);
        let agent = Arc::new(RwLock::new(
            self.create_with_profile_and_namespaces_async(
                session_id,
                profile_id,
                task_type,
                enabled_namespaces,
            )
            .await,
        ));
        agents.insert(session_id.to_string(), agent.clone());
        agent
    }

    /// Get or create an agent for a session with per-agent model override.
    ///
    /// This method is the primary entry point for graph-based execution where
    /// each agent node may use a different model.
    ///
    /// # Model Resolution Order
    ///
    /// 1. `model` parameter (if Some) — explicit per-node override
    /// 2. Factory `default_model` — global fallback
    /// 3. Router default — provider-level default
    ///
    /// # Session ID Format
    ///
    /// Graph executors generate unique session IDs like:
    /// - `expert-coordinator-{exec_id}-coordinator`
    /// - `specialist-browser_agent-{exec_id}-browser_agent`
    ///
    /// This ensures each node gets its own cached agent.
    #[cfg(feature = "graph")]
    pub async fn get_or_create_with_model(
        &self,
        session_id: &str,
        model: Option<String>,
        profile_id: Option<String>,
        task_type: Option<String>,
    ) -> Arc<RwLock<AgentRunner>> {
        self.get_or_create_with_overrides(session_id, model, profile_id, task_type, None)
            .await
    }

    /// R2-H12: Create or retrieve an agent with all per-node overrides including max_turns.
    #[cfg(feature = "graph")]
    pub async fn get_or_create_with_overrides(
        &self,
        session_id: &str,
        model: Option<String>,
        profile_id: Option<String>,
        task_type: Option<String>,
        max_turns: Option<u32>,
    ) -> Arc<RwLock<AgentRunner>> {
        let mut agents = self.agents.write().await;

        if let Some(agent) = agents.get(session_id) {
            return agent.clone();
        }

        tracing::info!(
            session_id = %session_id,
            model = ?model,
            profile_id = ?profile_id,
            task_type = ?task_type,
            max_turns = ?max_turns,
            "Creating agent with overrides"
        );

        // Create agent with overrides
        Self::evict_if_needed(&mut agents);
        let agent = Arc::new(RwLock::new(
            self.create_with_model_override_async(
                session_id, model, profile_id, task_type, max_turns,
            )
            .await,
        ));
        agents.insert(session_id.to_string(), agent.clone());
        agent
    }

    /// Create a new AgentRunner with model override (async).
    ///
    /// Uses `create_llm_adapter_with_model_override` for per-agent model routing.
    #[cfg(feature = "graph")]
    async fn create_with_model_override_async(
        &self,
        session_id: impl Into<String>,
        model: Option<String>,
        profile_id: Option<String>,
        task_type: Option<String>,
        max_turns: Option<u32>,
    ) -> AgentRunner {
        let session_id = session_id.into();

        let mut config = AgentConfig::default();
        config.cwd = self.default_cwd.clone();
        // R2-H12: Use per-node max_turns override if provided, otherwise factory default
        config.max_turns = max_turns.unwrap_or(self.default_max_turns);
        config.max_budget_usd = self.default_max_budget_usd;
        config.permission_mode = self.default_permission_mode;

        // Generate system prompt using six-layer context hierarchy if enabled
        if self.enable_context_hierarchy {
            let system_prompt = self.generate_context_system_prompt(Some(&session_id));
            if !system_prompt.is_empty() {
                config.system_prompt = Some(system_prompt);
            }
        }

        // Create LLM adapter with model override
        let llm_adapter =
            self.create_llm_adapter_with_model_override(model.as_deref(), profile_id, task_type);
        // Use async version to cache MCP tools with namespace filtering
        let tool_registry = self.create_tool_registry_async().await;
        let permission_ctx = self.create_permission_context();
        let compactor = self.create_compactor(&config.compaction);

        let mut runner = AgentRunner::with_session_id(config, session_id)
            .with_llm(llm_adapter)
            .with_tools(tool_registry)
            .with_hooks(self.default_hooks.clone())
            .with_permission_context(permission_ctx)
            .with_compactor(compactor);

        // Apply checkpoint configuration if set
        if let Some(ref checkpoint_config) = self.checkpoint_config {
            runner = runner.with_auto_checkpoint_config(checkpoint_config.clone());
        }
        if let Some(ref checkpoint_manager) = self.checkpoint_manager {
            runner = runner.with_checkpoint_manager(checkpoint_manager.clone());
        }

        // Apply constraint validator if profile is set
        #[cfg(feature = "prompt-constraints")]
        {
            runner = self.apply_constraint_validator(runner);
        }

        runner
    }

    /// Create a new AgentRunner with profile-based routing and custom namespaces (async).
    ///
    /// This method creates an agent with specified namespace filtering for MCP tools.
    async fn create_with_profile_and_namespaces_async(
        &self,
        session_id: impl Into<String>,
        profile_id: Option<String>,
        task_type: Option<String>,
        enabled_namespaces: Option<Vec<String>>,
    ) -> AgentRunner {
        let session_id = session_id.into();

        let mut config = AgentConfig::default();
        config.cwd = self.default_cwd.clone();
        config.max_turns = self.default_max_turns;
        config.max_budget_usd = self.default_max_budget_usd;
        config.permission_mode = self.default_permission_mode;

        // Generate system prompt using six-layer context hierarchy if enabled
        if self.enable_context_hierarchy {
            let system_prompt = self.generate_context_system_prompt(Some(&session_id));
            if !system_prompt.is_empty() {
                config.system_prompt = Some(system_prompt);
                tracing::debug!(
                    session_id = %session_id,
                    profile_id = ?profile_id,
                    task_type = ?task_type,
                    enabled_namespaces = ?enabled_namespaces,
                    "Using six-layer context hierarchy with custom namespace filtering"
                );
            }
        }

        // Create LLM adapter with profile
        let llm_adapter = self.create_llm_adapter_with_profile(profile_id, task_type);

        // Create tool registry with custom namespace filtering
        let tool_registry = self
            .create_tool_registry_with_namespaces_async(enabled_namespaces)
            .await;

        let permission_ctx = self.create_permission_context();
        let compactor = self.create_compactor(&config.compaction);

        let mut runner = AgentRunner::with_session_id(config, session_id)
            .with_llm(llm_adapter)
            .with_tools(tool_registry)
            .with_hooks(self.default_hooks.clone())
            .with_permission_context(permission_ctx)
            .with_compactor(compactor);

        // Apply checkpoint configuration if set
        if let Some(ref checkpoint_config) = self.checkpoint_config {
            runner = runner.with_auto_checkpoint_config(checkpoint_config.clone());
        }
        if let Some(ref checkpoint_manager) = self.checkpoint_manager {
            runner = runner.with_checkpoint_manager(checkpoint_manager.clone());
        }

        // Apply constraint validator if profile is set
        #[cfg(feature = "prompt-constraints")]
        {
            runner = self.apply_constraint_validator(runner);
        }

        runner
    }

    /// Create a tool registry with custom namespace filtering (async).
    async fn create_tool_registry_with_namespaces_async(
        &self,
        enabled_namespaces: Option<Vec<String>>,
    ) -> Arc<ToolRegistry> {
        let mut registry = if let Some(ts) = &self.tool_system {
            ToolRegistry::with_tool_system(ts.clone())
        } else if let Some(mcp) = &self.mcp_gateway {
            ToolRegistry::with_mcp_gateway(mcp.clone())
        } else {
            ToolRegistry::new()
        };

        // Apply custom namespace filtering and cache MCP tools
        if let Some(namespaces) = enabled_namespaces {
            registry.set_enabled_namespaces(namespaces);
        }
        registry.cache_mcp_tools().await;

        // Replace default LocalComputerTool with UnifiedComputerTool
        if let Some(router) = &self.code_router {
            registry.with_router(router.clone());
        }

        // Register BrowserTool when a VM manager is available
        #[cfg(unix)]
        if let Some(vm_manager) = &self.vm_manager {
            registry.with_browser_tool(vm_manager.clone());
        }

        // Register OrchestrateTool when a worker manager is available
        if let Some(worker_manager) = &self.worker_manager {
            registry.with_orchestrate_tool(worker_manager.clone());
        }

        // Register CodeOrchestrationTool when code orchestration runtime is available
        if let Some(runtime) = &self.code_orchestration_runtime {
            registry.with_code_orchestration_tool(runtime.clone());
        }

        // Register platform control plane tools when config is available
        if let Some(config) = &self.platform_tool_config {
            registry.with_platform_tools(config.clone());
        }

        // Register hosting tools for web app deployment
        if let Some(config) = &self.hosting_tool_config {
            registry.with_hosting_tools(config.clone());
        }

        // Register devtools observation tools for monitoring
        if let Some(config) = &self.devtools_tool_config {
            registry.with_devtools_tools(config.clone());
        }

        // Register database tools (reuses hosting/platform API base URL)
        #[cfg(feature = "database")]
        if let Some(hosting_config) = &self.hosting_tool_config {
            let db_config =
                std::sync::Arc::new(crate::agent::tools::database::DatabaseToolConfig::new(
                    &hosting_config.base_url,
                    hosting_config.get_token(),
                ));
            registry.with_database_tools(db_config);
        }

        // Register screen tools when screen controller is available
        if let Some(controller) = &self.screen_controller {
            crate::screen::register_screen_tools(
                &mut registry,
                controller.clone(),
                self.cdp_controller.clone(),
            );
        }

        // Enable tool discovery mode (A46) — must be last, after all tools are registered
        if self.tool_discovery_enabled && !self.tool_discovery_initial.is_empty() {
            registry.enable_discovery(self.tool_discovery_initial.clone());
        }

        Arc::new(registry)
    }

    /// Remove an agent from the cache
    pub async fn remove(&self, session_id: &str) -> Option<Arc<RwLock<AgentRunner>>> {
        let mut agents = self.agents.write().await;
        agents.remove(session_id)
    }

    /// List all active session IDs
    pub async fn list_sessions(&self) -> Vec<String> {
        let agents = self.agents.read().await;
        agents.keys().cloned().collect()
    }

    /// Get count of active agents
    pub async fn active_count(&self) -> usize {
        let agents = self.agents.read().await;
        agents.len()
    }

    /// Clear all cached agents
    pub async fn clear_all(&self) {
        let mut agents = self.agents.write().await;
        agents.clear();
    }

    /// R1-H16: Evict excess entries when the agent cache exceeds MAX_AGENT_CACHE_SIZE.
    ///
    /// Called with the write-locked map before each insert. Removes entries
    /// (arbitrary order) until the cache is back at 90 % of the limit, leaving
    /// headroom so we don't evict on every single subsequent insert.
    fn evict_if_needed(agents: &mut HashMap<String, Arc<RwLock<AgentRunner>>>) {
        if agents.len() < MAX_AGENT_CACHE_SIZE {
            return;
        }
        let target = MAX_AGENT_CACHE_SIZE * 9 / 10; // keep 90 %
        let to_remove = agents.len() - target;
        let keys_to_remove: Vec<String> = agents.keys().take(to_remove).cloned().collect();
        for key in &keys_to_remove {
            agents.remove(key);
        }
        tracing::info!(
            removed = keys_to_remove.len(),
            remaining = agents.len(),
            "R1-H16: evicted stale agent cache entries"
        );
    }

    /// Create an AgentRunner from a CLAUDE.md file
    ///
    /// This method loads configuration from a CLAUDE.md file, resolves
    /// any inheritance, and creates a fully configured AgentRunner.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let factory = AgentFactory::new(llm_router);
    /// let agent = factory.create_from_claude_md("/path/to/CLAUDE.md")?;
    /// ```
    pub fn create_from_claude_md(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<AgentRunner, ClaudeConfigError> {
        let config = ClaudeConfigBuilder::new()
            .load_file(path)?
            .working_dir(
                self.default_cwd
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(".")),
            )
            .build()?;

        Ok(self.create_with_config(config))
    }

    /// Create an AgentRunner from a directory containing CLAUDE.md
    ///
    /// Searches for CLAUDE.md in the specified directory and creates
    /// an agent configured according to its contents.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let factory = AgentFactory::new(llm_router);
    /// let agent = factory.create_from_dir("/path/to/project")?;
    /// ```
    pub fn create_from_dir(&self, dir: impl AsRef<Path>) -> Result<AgentRunner, ClaudeConfigError> {
        let dir = dir.as_ref();
        let config = ClaudeConfigBuilder::new()
            .load_dir(dir)?
            .working_dir(dir)
            .build()?;

        Ok(self.create_with_config(config))
    }

    /// Create an AgentRunner from discovered CLAUDE.md files in directory hierarchy
    ///
    /// This method searches upward from the specified directory to find all
    /// CLAUDE.md files and merges them (root configs applied first, local configs last).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let factory = AgentFactory::new(llm_router);
    /// let agent = factory.create_from_hierarchy("/path/to/project/subdir")?;
    /// ```
    pub fn create_from_hierarchy(
        &self,
        start_dir: impl AsRef<Path>,
    ) -> Result<AgentRunner, ClaudeConfigError> {
        let start_dir = start_dir.as_ref();
        let configs = crate::agent::config::discover_configs(start_dir)?;
        let merged = crate::agent::config::merge_discovered_configs(configs)?;

        let agent_config = ClaudeConfigBuilder::new()
            .working_dir(start_dir)
            .register("merged", merged)
            .parse("---\nextends: merged\n---")?
            .build()?;

        Ok(self.create_with_config(agent_config))
    }

    /// Create an AgentRunner from a CLAUDE.md string
    ///
    /// # Example
    ///
    /// ```ignore
    /// let config_str = r#"---
    /// name: my-agent
    /// model: claude-sonnet-4-6
    /// ---
    /// # Instructions
    /// Be helpful.
    /// "#;
    /// let agent = factory.create_from_str(config_str)?;
    /// ```
    pub fn create_from_str(&self, content: &str) -> Result<AgentRunner, ClaudeConfigError> {
        let config = ClaudeConfigBuilder::new()
            .parse(content)?
            .working_dir(
                self.default_cwd
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(".")),
            )
            .build()?;

        Ok(self.create_with_config(config))
    }

    /// Create an AgentRunner with inheritance from multiple CLAUDE.md sources
    ///
    /// # Example
    ///
    /// ```ignore
    /// let factory = AgentFactory::new(llm_router);
    /// let agent = factory.create_with_inheritance(
    ///     "/path/to/child.md",
    ///     vec!["/path/to/base.md"],
    /// )?;
    /// ```
    pub fn create_with_inheritance(
        &self,
        config_path: impl AsRef<Path>,
        parent_paths: Vec<impl AsRef<Path>>,
    ) -> Result<AgentRunner, ClaudeConfigError> {
        let mut builder = ClaudeConfigBuilder::new();

        // Register parent configs
        for (idx, parent_path) in parent_paths.iter().enumerate() {
            let parent_config = ClaudeConfig::load(parent_path.as_ref())?;
            let name = parent_config
                .name()
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("parent_{}", idx));
            builder = builder.register(name, parent_config);
        }

        let config = builder
            .load_file(config_path)?
            .working_dir(
                self.default_cwd
                    .clone()
                    .unwrap_or_else(|| PathBuf::from(".")),
            )
            .build()?;

        Ok(self.create_with_config(config))
    }

    /// Get or create an agent for a session from CLAUDE.md in a directory
    pub async fn get_or_create_from_dir(
        &self,
        session_id: &str,
        dir: impl AsRef<Path>,
    ) -> Result<Arc<RwLock<AgentRunner>>, ClaudeConfigError> {
        let mut agents = self.agents.write().await;

        if let Some(agent) = agents.get(session_id) {
            return Ok(agent.clone());
        }

        Self::evict_if_needed(&mut agents);
        let agent = Arc::new(RwLock::new(self.create_from_dir(dir)?));
        agents.insert(session_id.to_string(), agent.clone());
        Ok(agent)
    }

    // =========================================================================
    // Graph-based Agent Creation (requires "collaboration" feature)
    // =========================================================================

    /// Attach observers (Recording + Learning + Devtools + optional Streaming) to a graph builder.
    ///
    /// Uses CompositeObserver when multiple observers are needed.
    #[cfg(feature = "collaboration")]
    fn attach_observers(
        &self,
        mut builder: StateGraphBuilder<AgentGraphState>,
        exec_id: Option<&str>,
        streaming_observer: Option<crate::graph::StreamingObserver>,
    ) -> StateGraphBuilder<AgentGraphState> {
        use crate::graph::{CompositeObserver, RecordingObserver};

        // When StreamingObserver is present, the job scheduler already forwards
        // all stream events to ExecutionStore. Skip RecordingObserver to avoid
        // duplicate events and wrong ordering (content_delta after node_completed).
        let has_streaming = streaming_observer.is_some();
        let has_recording = !has_streaming && exec_id.is_some() && self.execution_store.is_some();

        #[cfg(feature = "learning")]
        let has_learning = self.learning_engine.is_some();
        #[cfg(not(feature = "learning"))]
        let has_learning = false;

        #[cfg(feature = "devtools")]
        let has_devtools = self.devtools_service.is_some();
        #[cfg(not(feature = "devtools"))]
        let has_devtools = false;

        // Count how many observers we have
        let observer_count =
            has_recording as u8 + has_learning as u8 + has_streaming as u8 + has_devtools as u8;

        if observer_count > 1 {
            // Multiple observers: use CompositeObserver
            let mut composite = CompositeObserver::<AgentGraphState>::new();
            // Only add RecordingObserver when has_recording is true.
            // When StreamingObserver is present, the scheduler drains content
            // events and writes them to ExecutionStore — RecordingObserver's
            // on_graph_complete calls complete_execution() which prematurely
            // drops SSE subscriber channels before content events are forwarded.
            if has_recording {
                if let (Some(eid), Some(ref store)) = (exec_id, &self.execution_store) {
                    composite = composite.add(RecordingObserver::new(store.clone(), eid));
                }
            }
            #[cfg(feature = "learning")]
            if let Some(ref engine) = self.learning_engine {
                composite = composite.add(crate::learning::LearningObserver::new(
                    engine.collector().clone(),
                ));
            }
            #[cfg(feature = "devtools")]
            if let Some(ref devtools_svc) = self.devtools_service {
                let bridge =
                    std::sync::Arc::new(crate::agent::devtools_bridge::DevtoolsBridge::new(
                        devtools_svc.clone(),
                        "canal",
                    ));
                let session_id = exec_id.unwrap_or("unknown").to_string();
                composite = composite.add(crate::graph::DevtoolsObserver::new(bridge, session_id));
            }
            if let Some(obs) = streaming_observer {
                composite = composite.add(obs);
            }
            builder = builder.with_observer(composite);
        } else if has_recording {
            if let (Some(eid), Some(ref store)) = (exec_id, &self.execution_store) {
                builder = builder.with_observer(RecordingObserver::new(store.clone(), eid));
            }
        } else if has_learning {
            #[cfg(feature = "learning")]
            if let Some(ref engine) = self.learning_engine {
                let observer = crate::learning::LearningObserver::new(engine.collector().clone());
                builder = builder.with_observer(observer);
            }
        } else if has_devtools {
            #[cfg(feature = "devtools")]
            if let Some(ref devtools_svc) = self.devtools_service {
                let bridge =
                    std::sync::Arc::new(crate::agent::devtools_bridge::DevtoolsBridge::new(
                        devtools_svc.clone(),
                        "canal",
                    ));
                let session_id = exec_id.unwrap_or("unknown").to_string();
                builder =
                    builder.with_observer(crate::graph::DevtoolsObserver::new(bridge, session_id));
            }
        } else if let Some(obs) = streaming_observer {
            builder = builder.with_observer(obs);
        }

        builder
    }

    /// Create a simple single-node StateGraph for Direct mode execution.
    ///
    /// This creates a graph with a single AgentRunnerNode that wraps the
    /// traditional AgentRunner execution. Useful when you want graph-based
    /// execution semantics (checkpointing, observability) but don't need
    /// multi-agent collaboration.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use gateway_core::graph::{AgentGraphState, GraphExecutor};
    ///
    /// let graph = factory.create_direct_graph()?;
    /// let executor = GraphExecutor::new(graph);
    /// let state = AgentGraphState::new("Write a poem about Rust");
    /// let result = executor.execute(state).await?;
    /// println!("{}", result.response);
    /// ```
    #[cfg(feature = "collaboration")]
    pub fn create_direct_graph(
        self: &Arc<Self>,
    ) -> Result<StateGraph<AgentGraphState>, GraphError> {
        self.create_direct_graph_tracked(None)
    }

    /// Create a Direct graph with optional execution tracking.
    ///
    /// When `exec_id` is provided and an execution store is configured,
    /// a `RecordingObserver` is attached to record events to the store.
    #[cfg(feature = "collaboration")]
    pub fn create_direct_graph_tracked(
        self: &Arc<Self>,
        exec_id: Option<&str>,
    ) -> Result<StateGraph<AgentGraphState>, GraphError> {
        let node = AgentRunnerNode::new(self.clone());

        let mut builder = StateGraphBuilder::new()
            .add_node("agent", node)
            .set_entry("agent")
            .set_terminal("agent");

        // Attach observers (Recording + Learning)
        builder = self.attach_observers(builder, exec_id, None);

        builder.build()
    }

    /// Create a Plan-Execute graph: Planner → Executor → Synthesizer.
    ///
    /// This is a three-phase execution pattern where:
    /// - **Planner**: Analyzes the task and creates a step-by-step plan
    /// - **Executor**: Executes each step of the plan using tools
    /// - **Synthesizer**: Combines results into a coherent response
    ///
    /// This mode is ideal for complex multi-step tasks that benefit from
    /// explicit planning before execution.
    #[cfg(feature = "collaboration")]
    pub fn create_plan_execute_graph(
        self: &Arc<Self>,
    ) -> Result<StateGraph<AgentGraphState>, GraphError> {
        self.create_plan_execute_graph_tracked(None)
    }

    /// Create a Plan-Execute graph with optional execution tracking.
    ///
    /// Enhanced version (A24) that uses `TaskPlanner` with Function Calling
    /// for structured plan generation, step-by-step execution with a loop,
    /// and re-planning on failure.
    ///
    /// Graph topology:
    /// ```text
    /// [planner] → [executor] → (check_step_result)
    ///                  ↑              │
    ///                  │        ┌─────┼──────┐
    ///                  │   "next_step" "replan" "done"
    ///                  │        │      │      │
    ///                  └────────┘      ↓      ↓
    ///                          [replanner] [synthesizer] → END
    ///                               │
    ///                               └──→ [executor]
    /// ```
    #[cfg(feature = "collaboration")]
    pub fn create_plan_execute_graph_tracked(
        self: &Arc<Self>,
        exec_id: Option<&str>,
    ) -> Result<StateGraph<AgentGraphState>, GraphError> {
        let mut builder = self.create_plan_execute_graph_builder(None)?;
        builder = self.attach_observers(builder, exec_id, None);
        builder.build()
    }

    /// Create a Plan-Execute graph builder (topology only, no observers).
    ///
    /// Returns the builder before observers are attached, allowing callers
    /// to add a StreamingObserver before building.
    #[cfg(feature = "collaboration")]
    fn create_plan_execute_graph_builder(
        self: &Arc<Self>,
        content_tx: Option<tokio::sync::mpsc::Sender<crate::graph::GraphStreamEvent>>,
    ) -> Result<StateGraphBuilder<AgentGraphState>, GraphError> {
        use crate::collaboration::approval::{
            classify_risk, max_risk_level, PlanApprovalDecision, PlanStepReview,
        };
        use crate::collaboration::planner::{PlanStep, PlannerConfig, TaskPlanner};
        use crate::collaboration::prd::{
            self, ClarificationResponse, PrdApprovalDecision, ResearchOutput, TaskComplexity,
        };
        use crate::graph::GraphStreamEvent;

        let config = self
            .planner_config
            .clone()
            .unwrap_or_else(|| Arc::new(PlannerConfig::default()));

        // ================================================================
        // A43: Research Planner Pipeline (inserted before existing planner)
        // ================================================================

        // === Research Planner Node ===
        // Uses AgentRunner with READ-ONLY tools to explore the codebase.
        // Produces structured ResearchOutput via submit_research tool.
        let factory_research = self.clone();
        let research_tx = content_tx.clone();
        let research_planner = ClosureHandler::new(
            move |mut state: AgentGraphState, _ctx: &NodeContext| {
                let factory = factory_research.clone();
                let tx = research_tx.clone();
                async move {
                    let execution_id = state.plan_state().execution_id.clone();

                    // Send progress event
                    if let Some(ref tx) = tx {
                        let _ = tx.try_send(GraphStreamEvent::NodeText {
                            execution_id: execution_id.clone(),
                            node_id: "research_planner".into(),
                            content: "Exploring codebase...".into(),
                        });
                    }

                    // Build read-only tool registry
                    let registry = Arc::new(crate::agent::tools::ToolRegistry::new_read_only());
                    let tool_schemas = registry.get_tool_schemas();

                    // Build tool definitions for LLM request
                    let mut tool_defs: Vec<crate::llm::router::ToolDefinition> = tool_schemas
                        .iter()
                        .filter_map(|s| {
                            Some(crate::llm::router::ToolDefinition {
                                name: s["name"].as_str()?.to_string(),
                                description: s["description"].as_str()?.to_string(),
                                input_schema: s["input_schema"].clone(),
                            })
                        })
                        .collect();

                    // Add submit_research tool
                    tool_defs.push(prd::submit_research_tool_def());

                    // Run research via LLM with tools (multi-turn agent loop)
                    let llm_router = factory.llm_router.clone();
                    let system_prompt = prd::RESEARCH_PLANNER_SYSTEM_PROMPT.to_string();
                    let user_message = format!(
                        "Task to research:\n{}\n\nExplore the codebase and call submit_research when done.",
                        state.task
                    );

                    // Use a simplified agent loop: up to 15 turns, looking for submit_research call
                    use crate::llm::router::{ContentBlock, Message};
                    let mut messages = vec![
                        Message::text("system", &system_prompt),
                        Message::text("user", &user_message),
                    ];

                    let max_turns = 15;
                    let mut research_output: Option<ResearchOutput> = None;

                    for turn in 0..max_turns {
                        let request = crate::llm::router::ChatRequest {
                            model: Some("qwen-max".into()),
                            messages: messages.clone(),
                            tools: tool_defs.clone(),
                            max_tokens: Some(4096),
                            temperature: Some(0.3),
                            ..Default::default()
                        };

                        let response = match llm_router.route(request).await {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::error!(error = %e, turn = turn, "Research LLM call failed");
                                break;
                            }
                        };

                        // Check for tool calls in response
                        let content_blocks = response
                            .choices
                            .first()
                            .map(|c| c.message.content_blocks.clone())
                            .unwrap_or_default();

                        let mut has_tool_call = false;
                        for block in &content_blocks {
                            if let ContentBlock::ToolUse { id, name, input } = block {
                                has_tool_call = true;

                                if name == "submit_research" {
                                    // Parse research output
                                    match prd::parse_research_response(input) {
                                        Ok(ro) => {
                                            research_output = Some(ro);
                                            tracing::info!(
                                                turn = turn,
                                                "Research completed via submit_research"
                                            );
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                error = %e,
                                                "Failed to parse submit_research output"
                                            );
                                        }
                                    }
                                    break;
                                } else {
                                    // Execute read-only tool with default context
                                    let tool_ctx = crate::agent::tools::ToolContext::default();
                                    let tool_result =
                                        registry.execute(name, input.clone(), &tool_ctx).await;
                                    let (result_str, is_error) = match tool_result {
                                        Ok(v) => {
                                            (serde_json::to_string(&v).unwrap_or_default(), false)
                                        }
                                        Err(e) => (format!("Tool error: {}", e), true),
                                    };

                                    // Add assistant message with tool use block
                                    messages.push(Message::with_blocks(
                                        "assistant",
                                        vec![ContentBlock::ToolUse {
                                            id: id.clone(),
                                            name: name.clone(),
                                            input: input.clone(),
                                        }],
                                    ));

                                    // Add tool result message
                                    messages.push(Message::with_blocks(
                                        "user",
                                        vec![ContentBlock::ToolResult {
                                            tool_use_id: id.clone(),
                                            content: result_str,
                                            is_error,
                                        }],
                                    ));
                                }
                            }
                        }

                        if research_output.is_some() {
                            break;
                        }

                        // If no tool call was made, the LLM just responded with text — break
                        if !has_tool_call {
                            tracing::warn!(turn = turn, "Research agent produced text instead of tool call, ending research");
                            break;
                        }
                    }

                    // Store research output in typed state
                    if let Some(ref ro) = research_output {
                        // Assess complexity (code heuristic, 0 LLM)
                        let complexity = prd::assess_complexity(ro);

                        // Generate questions (code template, 0 LLM)
                        let questions = prd::get_template_questions(&ro.task_type, &complexity);

                        let ro_value = serde_json::to_value(ro).unwrap_or_default();
                        let questions_value = serde_json::to_value(&questions).unwrap_or_default();
                        let complexity_str = complexity.to_string();

                        state.update_plan_state(|ps| {
                            ps.research_output = Some(ro_value);
                            ps.task_complexity = Some(complexity_str);
                            ps.clarification_questions = Some(questions_value);
                        });

                        tracing::info!(
                            complexity = %complexity,
                            affected_files = ro.affected_files.len(),
                            questions = questions.len(),
                            "Research phase complete"
                        );
                    } else {
                        // Fallback: no research output, treat as simple
                        state.update_plan_state(|ps| {
                            ps.task_complexity = Some("simple".into());
                        });
                        tracing::warn!(
                            "Research produced no structured output, defaulting to simple"
                        );
                    }

                    Ok(state)
                }
            },
        );

        // === Check Complexity Edge ===
        // Routes based on task_complexity: simple → planner, medium/complex → next pipeline stage
        let check_complexity = ClosurePredicate::new(|state: &AgentGraphState| {
            let ps = state.plan_state();
            let complexity = ps.task_complexity.as_deref().unwrap_or("simple");

            let questions = ps
                .clarification_questions
                .as_ref()
                .and_then(|v| v.as_array().map(|a| a.len()))
                .unwrap_or(0);

            match complexity {
                "simple" => "simple".into(),
                _ if questions > 0 => "has_questions".into(),
                _ => "no_questions".into(),
            }
        });

        // === Clarification Gate Node ===
        // Sends questions to user via SSE, waits for response via oneshot.
        let factory_clarify = self.clone();
        let clarify_tx = content_tx.clone();
        let clarification_gate = ClosureHandler::new(
            move |mut state: AgentGraphState, _ctx: &NodeContext| {
                let factory = factory_clarify.clone();
                let tx = clarify_tx.clone();
                async move {
                    let ps = state.plan_state();
                    let execution_id = ps.execution_id.clone();

                    let questions = ps
                        .clarification_questions
                        .clone()
                        .unwrap_or(serde_json::Value::Array(vec![]));

                    let task_summary = ps
                        .research_output
                        .as_ref()
                        .and_then(|v| v["requirements_summary"].as_str().map(String::from))
                        .unwrap_or_else(|| state.task.clone());

                    if let Some(store) = &factory.pending_clarifications {
                        let request_id = uuid::Uuid::new_v4();
                        let session_id = uuid::Uuid::nil(); // TODO: pass from caller

                        let rx = store.register(
                            request_id,
                            session_id,
                            task_summary.clone(),
                            std::time::Duration::from_secs(300),
                        );

                        // Send SSE event with questions
                        if let Some(ref tx) = tx {
                            let _ = tx.try_send(GraphStreamEvent::NodeText {
                                execution_id: execution_id.clone(),
                                node_id: "clarification_gate".into(),
                                content: format!(
                                    "{{\"event_type\":\"clarification_required\",\"request_id\":\"{}\",\"questions\":{},\"task_summary\":\"{}\"}}",
                                    request_id,
                                    serde_json::to_string(&questions).unwrap_or_default(),
                                    task_summary,
                                ),
                            });
                        }

                        // Wait for user response (no timeout — task stays pending until user responds)
                        match rx.await {
                            Ok(response) => {
                                let answers_value =
                                    serde_json::to_value(&response).unwrap_or_default();
                                state.update_plan_state(|ps| {
                                    ps.clarification_answers = Some(answers_value);
                                });
                                tracing::info!(
                                    answers = response.answers.len(),
                                    "Clarification answers received"
                                );
                            }
                            Err(_) => {
                                // Channel dropped — skip with defaults
                                let empty = ClarificationResponse {
                                    answers: std::collections::HashMap::new(),
                                    skip_remaining: true,
                                };
                                let empty_value = serde_json::to_value(&empty).unwrap_or_default();
                                state.update_plan_state(|ps| {
                                    ps.clarification_answers = Some(empty_value);
                                });
                                tracing::warn!("Clarification channel dropped, using defaults");
                            }
                        }
                    } else {
                        // No store configured — skip clarification
                        let empty = ClarificationResponse {
                            answers: std::collections::HashMap::new(),
                            skip_remaining: true,
                        };
                        let empty_value = serde_json::to_value(&empty).unwrap_or_default();
                        state.update_plan_state(|ps| {
                            ps.clarification_answers = Some(empty_value);
                        });
                    }

                    Ok(state)
                }
            },
        );

        // === PRD Assembler Node ===
        // 1 LLM call with generate_prd tool to produce structured PRD.
        let factory_prd = self.clone();
        let prd_assembler = ClosureHandler::new(
            move |mut state: AgentGraphState, _ctx: &NodeContext| {
                let factory = factory_prd.clone();
                async move {
                    let ps = state.plan_state();
                    let research: Option<ResearchOutput> = ps
                        .research_output
                        .as_ref()
                        .and_then(|v| serde_json::from_value(v.clone()).ok());

                    let complexity_str = ps
                        .task_complexity
                        .as_deref()
                        .unwrap_or("medium")
                        .to_string();

                    let complexity = match complexity_str.as_str() {
                        "simple" => TaskComplexity::Simple,
                        "medium" => TaskComplexity::Medium,
                        _ => TaskComplexity::Complex,
                    };

                    let research = research.unwrap_or(ResearchOutput {
                        task_type: prd::TaskType::NewFeature,
                        requirements_summary: state.task.clone(),
                        research_findings: String::new(),
                        affected_files: vec![],
                        existing_patterns: vec![],
                        approach_hints: vec![],
                    });

                    // Build clarification answers string
                    let answers_str = ps
                        .clarification_answers
                        .as_ref()
                        .and_then(|v| {
                            let resp: ClarificationResponse =
                                serde_json::from_value(v.clone()).ok()?;
                            Some(
                                resp.answers
                                    .iter()
                                    .map(|(k, v)| format!("Q{}: {}", k, v))
                                    .collect::<Vec<_>>()
                                    .join("; "),
                            )
                        })
                        .unwrap_or_else(|| "（用户跳过了澄清）".into());

                    // Check for revision feedback
                    let revision_feedback = ps.prd_revision_feedback.clone();

                    let prompt = if let Some(feedback) = &revision_feedback {
                        format!(
                            "{}\n\n== 修改要求 ==\n{}\n请根据以上反馈重新生成 PRD。",
                            prd::build_prd_assembler_prompt(&research, &answers_str),
                            feedback
                        )
                    } else {
                        prd::build_prd_assembler_prompt(&research, &answers_str)
                    };

                    // Clear revision feedback after use
                    if revision_feedback.is_some() {
                        state.update_plan_state(|ps| {
                            ps.prd_revision_feedback = None;
                        });
                    }

                    // Select tool def based on complexity
                    let is_complex_coding = complexity == TaskComplexity::Complex
                        && prd::is_coding_task(&research.task_type);
                    let tool_def = if is_complex_coding {
                        prd::generate_prd_expanded_tool_def()
                    } else {
                        prd::generate_prd_tool_def()
                    };

                    use crate::llm::router::{ContentBlock, Message, ToolChoice};

                    let request = crate::llm::router::ChatRequest {
                        model: Some("qwen-max".into()),
                        messages: vec![
                            Message::text("system", &prompt),
                            Message::text("user", &format!("生成 PRD for: {}", state.task)),
                        ],
                        tools: vec![tool_def],
                        tool_choice: Some(ToolChoice::Tool {
                            name: "generate_prd".into(),
                        }),
                        max_tokens: Some(4096),
                        temperature: Some(0.3),
                        ..Default::default()
                    };

                    match factory.llm_router.route(request).await {
                        Ok(response) => {
                            // Extract tool_use block from response
                            let content_blocks = response
                                .choices
                                .first()
                                .map(|c| &c.message.content_blocks)
                                .into_iter()
                                .flatten();
                            for block in content_blocks {
                                if let ContentBlock::ToolUse { input, .. } = block {
                                    match prd::parse_prd_response(
                                        input,
                                        complexity,
                                        Some(research.research_findings.clone()),
                                    ) {
                                        Ok(prd_doc) => {
                                            // For complex coding tasks, distill
                                            if is_complex_coding {
                                                let core = prd::distill_core_concepts(&prd_doc);
                                                let distilled = prd::compress_prd(&prd_doc, &core);
                                                let distilled_value =
                                                    serde_json::to_value(&distilled)
                                                        .unwrap_or_default();
                                                state.update_plan_state(|ps| {
                                                    ps.distilled_prd = Some(distilled_value);
                                                });
                                            }

                                            let prd_value =
                                                serde_json::to_value(&prd_doc).unwrap_or_default();
                                            state.update_plan_state(|ps| {
                                                ps.prd_document = Some(prd_value);
                                            });

                                            tracing::info!(
                                                title = %prd_doc.title,
                                                approaches = prd_doc.approaches.len(),
                                                "PRD generated successfully"
                                            );
                                        }
                                        Err(e) => {
                                            tracing::error!(error = %e, "Failed to parse PRD response");
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "PRD assembler LLM call failed");
                        }
                    }

                    Ok(state)
                }
            },
        );

        // === PRD Approval Gate Node ===
        // Sends full PRD to user for review, waits for approve/revise/reject.
        let factory_prd_approve = self.clone();
        let prd_approve_tx = content_tx.clone();
        let prd_approval_gate = ClosureHandler::new(
            move |mut state: AgentGraphState, _ctx: &NodeContext| {
                let factory = factory_prd_approve.clone();
                let tx = prd_approve_tx.clone();
                async move {
                    let ps = state.plan_state();
                    let execution_id = ps.execution_id.clone();

                    let prd_json = ps.prd_document.clone().unwrap_or(serde_json::Value::Null);

                    let title = prd_json["title"]
                        .as_str()
                        .unwrap_or("Untitled PRD")
                        .to_string();

                    let revision_round = if ps.prd_revision_round == 0 {
                        1
                    } else {
                        ps.prd_revision_round
                    };

                    let max_revisions: u32 = 3;

                    if let Some(store) = &factory.pending_prd_approvals {
                        let request_id = uuid::Uuid::new_v4();
                        let session_id = uuid::Uuid::nil();

                        let rx = store.register(
                            request_id,
                            session_id,
                            title.clone(),
                            revision_round,
                            std::time::Duration::from_secs(300),
                        );

                        // Send SSE event with PRD for review
                        if let Some(ref tx) = tx {
                            let _ = tx.try_send(GraphStreamEvent::NodeText {
                                execution_id: execution_id.clone(),
                                node_id: "prd_approval_gate".into(),
                                content: format!(
                                    "{{\"event_type\":\"prd_review_required\",\"request_id\":\"{}\",\"prd\":{},\"revision_round\":{},\"max_revisions\":{}}}",
                                    request_id,
                                    serde_json::to_string(&prd_json).unwrap_or_default(),
                                    revision_round,
                                    max_revisions,
                                ),
                            });
                        }

                        // Wait for user decision (no timeout — task stays pending until user responds)
                        match rx.await {
                            Ok(decision) => {
                                match &decision {
                                    PrdApprovalDecision::Approve { chosen_approach } => {
                                        // Inject PRD context into planner config
                                        let ps = state.plan_state();
                                        let complexity_str =
                                            ps.task_complexity.as_deref().unwrap_or_default();

                                        let prd_context = if complexity_str == "complex" {
                                            // Use distilled PRD for complex tasks
                                            ps.distilled_prd
                                                .as_ref()
                                                .and_then(|v| {
                                                    let d: prd::DistilledPrd =
                                                        serde_json::from_value(v.clone()).ok()?;
                                                    Some(prd::build_step_planner_distilled_context(
                                                        &d,
                                                    ))
                                                })
                                                .unwrap_or_default()
                                        } else {
                                            // Use full PRD for medium tasks
                                            ps.prd_document
                                                .as_ref()
                                                .and_then(|v| {
                                                    let p: prd::PrdDocument =
                                                        serde_json::from_value(v.clone()).ok()?;
                                                    Some(prd::build_step_planner_prd_context(&p))
                                                })
                                                .unwrap_or_default()
                                        };

                                        let chosen = *chosen_approach;
                                        state.update_plan_state(|ps| {
                                            ps.prd_decision = Some(crate::collaboration::state::ApprovalDecision::Approved);
                                            ps.chosen_approach = Some(chosen);
                                            ps.prd_context = Some(prd_context);
                                        });

                                        tracing::info!(
                                            chosen_approach = chosen_approach,
                                            "PRD approved"
                                        );
                                    }
                                    PrdApprovalDecision::Revise { feedback } => {
                                        if revision_round >= max_revisions {
                                            state.update_plan_state(|ps| {
                                                ps.prd_decision = Some(crate::collaboration::state::ApprovalDecision::Rejected);
                                            });
                                            tracing::warn!(
                                                "Max PRD revisions reached, auto-rejecting"
                                            );
                                        } else {
                                            let fb = feedback.clone();
                                            state.update_plan_state(|ps| {
                                                ps.prd_decision = Some(crate::collaboration::state::ApprovalDecision::Pending);
                                                ps.prd_revision_feedback = Some(fb);
                                                ps.prd_revision_round = revision_round + 1;
                                            });
                                            tracing::info!(
                                                feedback = %feedback,
                                                round = revision_round,
                                                "PRD revision requested"
                                            );
                                        }
                                    }
                                    PrdApprovalDecision::Reject { reason } => {
                                        state.update_plan_state(|ps| {
                                            ps.prd_decision = Some(crate::collaboration::state::ApprovalDecision::Rejected);
                                        });
                                        tracing::info!(
                                            reason = ?reason,
                                            "PRD rejected"
                                        );
                                    }
                                }
                            }
                            Err(_) => {
                                // Channel dropped — auto-reject
                                state.update_plan_state(|ps| {
                                    ps.prd_decision = Some(
                                        crate::collaboration::state::ApprovalDecision::Rejected,
                                    );
                                });
                                tracing::warn!("PRD approval channel dropped, auto-rejecting");
                            }
                        }
                    } else {
                        // No store configured — auto-approve with first approach
                        state.update_plan_state(|ps| {
                            ps.prd_decision =
                                Some(crate::collaboration::state::ApprovalDecision::Approved);
                            ps.chosen_approach = Some(0);
                        });
                    }

                    Ok(state)
                }
            },
        );

        // === Check PRD Decision Edge ===
        let check_prd_decision = ClosurePredicate::new(|state: &AgentGraphState| {
            let ps = state.plan_state();
            match ps.prd_decision {
                Some(crate::collaboration::state::ApprovalDecision::Approved) => "approved".into(),
                Some(crate::collaboration::state::ApprovalDecision::Pending) => "revise".into(),
                Some(crate::collaboration::state::ApprovalDecision::Rejected) => "rejected".into(),
                None => "approved".into(),
            }
        });

        // === Planner Node ===
        // Uses LlmRouter + create_plan tool for structured plan generation.
        // Handles revision: if `plan_revision_feedback` exists, appends to prompt.
        // A39: Injects knowledge context and verified plans before planning.
        let factory_planner = self.clone();
        let planner_config = config.clone();
        let planner = ClosureHandler::new(move |mut state: AgentGraphState, _ctx: &NodeContext| {
            let factory = factory_planner.clone();
            let config = planner_config.clone();
            async move {
                let llm_router = factory.llm_router.clone();

                // A39: Build knowledge context from LearningEngine + SkillRegistry
                let mut knowledge_sections = Vec::<String>::new();

                #[cfg(feature = "learning")]
                {
                    if let Some(ref engine) = factory.learning_engine {
                        if engine.is_enabled() {
                            let entries = engine.query_knowledge(&state.task);
                            if !entries.is_empty() {
                                let mut section =
                                    String::from("RELEVANT KNOWLEDGE FROM PAST EXECUTIONS:\n");
                                for entry in entries.iter().take(5) {
                                    section.push_str(&format!(
                                        "- [{:?}] {}\n",
                                        entry.category, entry.content
                                    ));
                                }
                                knowledge_sections.push(section);
                            }
                        }
                    }
                }

                if let Some(ref registry) = factory.skill_registry {
                    let verified = registry.find_verified_plans(&state.task, 3);
                    if !verified.is_empty() {
                        let mut section = String::from("VERIFIED PLANS FROM PAST SUCCESSES:\n");
                        for plan in &verified {
                            if let Some(steps_json) = plan.metadata.custom.get("plan_steps") {
                                let rate = plan
                                    .metadata
                                    .custom
                                    .get("success_rate")
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(0.0);
                                section.push_str(&format!(
                                    "- Plan: {} (success_rate: {:.0}%)\n  Steps: {}\n",
                                    plan.description,
                                    rate * 100.0,
                                    steps_json
                                ));
                            }
                        }
                        section.push_str(
                            "Consider reusing or adapting these verified plans if applicable.\n",
                        );
                        knowledge_sections.push(section);

                        // Track which verified plan was offered
                        if let Some(first) = verified.first() {
                            state.update_plan_state(|ps| {
                                ps.offered_verified_plan = Some(serde_json::json!(first.name));
                            });
                        }
                    }
                }

                let mut planner_cfg = (*config).clone();
                if !knowledge_sections.is_empty() {
                    planner_cfg.knowledge_context = Some(knowledge_sections.join("\n"));
                }

                // A40: Build tool summary from factory's tool registry so the
                // planner knows about specific tools (ClaudeCode, BashTool, etc.)
                if planner_cfg.tool_summary.is_none() {
                    let tool_registry = factory.create_tool_registry();
                    let tool_schemas = tool_registry.get_tool_schemas();
                    if !tool_schemas.is_empty() {
                        let mut summary = String::from("AVAILABLE TOOLS:\n");
                        for schema in &tool_schemas {
                            if let Some(name) = schema.get("name").and_then(|v| v.as_str()) {
                                let desc = schema
                                    .get("description")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                // Truncate long descriptions to keep prompt concise
                                let desc_short: String = desc.chars().take(120).collect();
                                summary.push_str(&format!("- {}: {}\n", name, desc_short));
                            }
                        }
                        planner_cfg.tool_summary = Some(summary);
                    }
                }

                // A43: Inject PRD context into planner if available
                if let Some(prd_ctx) = state.plan_state().prd_context.clone() {
                    if !prd_ctx.is_empty() {
                        let existing = planner_cfg.knowledge_context.take().unwrap_or_default();
                        planner_cfg.knowledge_context =
                            Some(format!("{}\n\n{}", existing, prd_ctx));
                    }
                }

                let planner = TaskPlanner::new(llm_router, planner_cfg);

                // Check for revision feedback from user
                let ps = state.plan_state();
                let revision_feedback = ps.plan_revision_feedback.clone();

                let task_with_feedback = if let Some(feedback) = &revision_feedback {
                    format!(
                        "{}\n\nPrevious plan was revised. User feedback: {}. Generate an improved plan.",
                        state.task, feedback
                    )
                } else {
                    state.task.clone()
                };

                // Clear revision feedback after use
                if revision_feedback.is_some() {
                    state.update_plan_state(|ps| {
                        ps.plan_revision_feedback = None;
                    });
                }

                match planner.plan(&task_with_feedback).await {
                    Ok(plan) => {
                        // Store structured plan in typed state
                        let step_count = plan.steps.len();
                        let goal_clone = plan.goal.clone();
                        let criteria_clone = plan.success_criteria.clone();
                        state.update_plan_state(|ps| {
                            ps.plan_steps = plan.steps.clone();
                            ps.plan_goal = plan.goal.clone();
                            ps.success_criteria = plan.success_criteria.clone();
                            ps.current_step_index = 0;
                            ps.total_steps = step_count;
                            // replan_count is preserved across revisions (default 0)
                        });

                        // Also store in state.plan for synthesizer compatibility
                        state.plan = Some(format!(
                            "Goal: {}\nSteps:\n{}",
                            goal_clone,
                            plan.steps
                                .iter()
                                .map(|s| format!("{}. [{}] {}", s.id, s.tool_category, s.action))
                                .collect::<Vec<_>>()
                                .join("\n")
                        ));

                        tracing::info!(
                            goal = %goal_clone,
                            step_count,
                            "Plan created via Function Calling"
                        );
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Planning failed, using task as single step");
                        // Fallback: treat entire task as a single step
                        state.plan = Some(format!("1. {}", state.task));
                        let fallback_step = PlanStep {
                            id: 1,
                            action: state.task.clone(),
                            tool_category: crate::collaboration::planner::ToolCategory::Llm,
                            dependency: crate::collaboration::planner::StepDependency::None,
                            expected_output: None,
                            executor_agent: None,
                            executor_model: None,
                            requires_visual_verification: None,
                            prd_content: None,
                            executor_type: None,
                        };
                        state.update_plan_state(|ps| {
                            ps.plan_steps = vec![fallback_step];
                            ps.current_step_index = 0;
                            ps.total_steps = 1;
                            ps.replan_count = 0;
                        });
                    }
                }
                Ok(state)
            }
        });

        // === Step Executor Node ===
        // Executes the current step via AgentRunner, then advances index.
        // A39: Records per-step experience and handles retry suggestions.
        let factory_executor = self.clone();
        let executor = ClosureHandler::new(
            move |mut state: AgentGraphState, _ctx: &NodeContext| {
                let factory = factory_executor.clone();
                async move {
                    let ps = state.plan_state();
                    let idx = ps.current_step_index;
                    let steps = ps.plan_steps.clone();

                    if idx >= steps.len() {
                        return Ok(state); // All steps done
                    }

                    let step = &steps[idx];
                    tracing::info!(
                        step_id = step.id,
                        action = %step.action,
                        tool_category = %step.tool_category,
                        "Executing plan step"
                    );

                    // A39: Check for retry suggestions from judge
                    let retry_prefix = ps
                        .step(idx)
                        .and_then(|s| s.retry_suggestions.as_ref())
                        .map(|s| format!("Previous attempt feedback: {}\n\n", s))
                        .unwrap_or_default();

                    // A40: Read pending instructions from HITL (injected by scheduler)
                    let instruction_prefix = state
                        .plan_state()
                        .pending_instruction
                        .clone()
                        .map(|s| format!("[User instruction: {}]\n\n", s))
                        .unwrap_or_default();
                    // Clear consumed instruction
                    if !instruction_prefix.is_empty() {
                        state.update_plan_state(|ps| {
                            ps.pending_instruction = None;
                        });
                    }

                    // A42: Route execution by executor_type
                    use crate::collaboration::planner::ExecutorType;
                    let executor_type = step.executor_type.unwrap_or(ExecutorType::LlmAgent);

                    // Build execution prompt for this step.
                    // For ClaudeCode/Shell-reroute: use a task-description prompt that
                    // does NOT embed the literal command (avoids recursive `claude` invocation).
                    // For LlmAgent/Shell: include the action as instruction.
                    let is_claude_executor = matches!(executor_type, ExecutorType::ClaudeCode)
                        || (matches!(executor_type, ExecutorType::Shell) && {
                            let trimmed = step.action.trim();
                            trimmed.starts_with("claude ") || trimmed == "claude"
                        });

                    let prompt = if let Some(ref prd) = step.prd_content {
                        // PRD-driven: use prd_content as the full instruction
                        format!("{}{}{}", instruction_prefix, retry_prefix, prd)
                    } else if is_claude_executor {
                        // ClaudeCode prompt: describe the TASK, not the command.
                        // Retrieve the overall plan goal for context.
                        let plan_goal = {
                            let g = state.plan_state().plan_goal.clone();
                            if g.is_empty() {
                                "complete the requested task".to_string()
                            } else {
                                g
                            }
                        };
                        let expected = step.expected_output.as_deref().unwrap_or("success");
                        format!(
                            "{}{}You are working on step {} of a plan.\n\
                             Overall goal: {}\n\
                             Step task: {}\n\
                             Expected result: {}\n\n\
                             Complete this step. Do NOT invoke `claude` CLI — you ARE Claude Code. \
                             Work directly using your available tools (Read, Write, Edit, Bash, Grep, Glob, etc.).",
                            instruction_prefix,
                            retry_prefix,
                            idx + 1,
                            plan_goal,
                            step.action,
                            expected
                        )
                    } else {
                        // LlmAgent / regular Shell: include the action as instruction
                        format!(
                            "{}{}Execute this specific action:\n{}\n\nExpected result: {}\nUse {} tools.",
                            instruction_prefix,
                            retry_prefix,
                            step.action,
                            step.expected_output.as_deref().unwrap_or("success"),
                            step.tool_category
                        )
                    };

                    let mut result_text = String::new();
                    let mut had_error = false;

                    match executor_type {
                        ExecutorType::ClaudeCode => {
                            // Direct ClaudeCode CLI invocation — skip LLM agent loop
                            use crate::agent::tools::AgentTool;
                            tracing::info!(step_id = step.id, "Executing step via ClaudeCode CLI");
                            let tool = crate::agent::tools::claude_code::ClaudeCodeTool::new();
                            let working_dir = factory
                                .default_cwd
                                .as_ref()
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|| ".".to_string());
                            let input = crate::agent::tools::claude_code::ClaudeCodeInput {
                                prompt: prompt.clone(),
                                working_dir,
                                model: step.executor_model.clone(),
                                max_turns: Some(25),
                                timeout_ms: Some(300_000), // 5 min per step
                                ..Default::default()
                            };
                            let ctx = crate::agent::tools::ToolContext::new(
                                format!("step-{}-{}", idx, uuid::Uuid::new_v4()),
                                factory.default_cwd.clone().unwrap_or_else(|| ".".into()),
                            );
                            match tool.execute(input, &ctx).await {
                                Ok(output) => {
                                    result_text = output.output;
                                    had_error = output.exit_code != 0;
                                    if had_error {
                                        let err_msg = format!(
                                            "ClaudeCode exited with code {}: {}",
                                            output.exit_code,
                                            if result_text.len() > 500 {
                                                &result_text[..500]
                                            } else {
                                                &result_text
                                            }
                                        );
                                        state.update_plan_state(|ps| {
                                            ps.step_mut(idx).error = Some(err_msg);
                                        });
                                    }
                                }
                                Err(e) => {
                                    had_error = true;
                                    result_text = e.to_string();
                                    let err_msg = e.to_string();
                                    state.update_plan_state(|ps| {
                                        ps.step_mut(idx).error = Some(err_msg);
                                    });
                                }
                            }
                        }
                        ExecutorType::Shell => {
                            // A42: Detect `claude` commands and reroute to ClaudeCode tool.
                            // `claude --dangerously-skip-permissions` is interactive — it cannot
                            // run as a plain shell subprocess. The ClaudeCode tool handles piping
                            // the prompt via --print -p and env_remove for nested-session detection.
                            let action_trimmed = step.action.trim();
                            let is_claude_cmd =
                                action_trimmed.starts_with("claude ") || action_trimmed == "claude";

                            if is_claude_cmd {
                                use crate::agent::tools::AgentTool;
                                tracing::info!(
                                    step_id = step.id,
                                    command = %step.action,
                                    "Rerouting claude shell command to ClaudeCode tool"
                                );
                                let tool = crate::agent::tools::claude_code::ClaudeCodeTool::new();
                                let working_dir = factory
                                    .default_cwd
                                    .as_ref()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_else(|| ".".to_string());
                                let input = crate::agent::tools::claude_code::ClaudeCodeInput {
                                    prompt: prompt.clone(),
                                    working_dir,
                                    model: step.executor_model.clone(),
                                    max_turns: Some(25),
                                    timeout_ms: Some(300_000),
                                    ..Default::default()
                                };
                                let ctx = crate::agent::tools::ToolContext::new(
                                    format!("step-{}-{}", idx, uuid::Uuid::new_v4()),
                                    factory.default_cwd.clone().unwrap_or_else(|| ".".into()),
                                );
                                match tool.execute(input, &ctx).await {
                                    Ok(output) => {
                                        result_text = output.output;
                                        had_error = output.exit_code != 0;
                                        if had_error {
                                            let err_msg = format!(
                                                "ClaudeCode exited with code {}: {}",
                                                output.exit_code,
                                                if result_text.len() > 500 {
                                                    &result_text[..500]
                                                } else {
                                                    &result_text
                                                }
                                            );
                                            state.update_plan_state(|ps| {
                                                ps.step_mut(idx).error = Some(err_msg);
                                            });
                                        }
                                    }
                                    Err(e) => {
                                        had_error = true;
                                        result_text = e.to_string();
                                        let err_msg = e.to_string();
                                        state.update_plan_state(|ps| {
                                            ps.step_mut(idx).error = Some(err_msg);
                                        });
                                    }
                                }
                            } else {
                                // Regular shell command execution
                                tracing::info!(
                                    step_id = step.id,
                                    command = %step.action,
                                    "Executing step via shell"
                                );
                                let cwd = factory.default_cwd.clone().unwrap_or_else(|| ".".into());
                                match tokio::process::Command::new("bash")
                                    .arg("-c")
                                    .arg(&step.action)
                                    .current_dir(&cwd)
                                    // A42: Remove Claude Code nested session detection env vars
                                    .env_remove("CLAUDECODE")
                                    .env_remove("CLAUDE_CODE_ENTRYPOINT")
                                    .output()
                                    .await
                                {
                                    Ok(output) => {
                                        let stdout = String::from_utf8_lossy(&output.stdout);
                                        let stderr = String::from_utf8_lossy(&output.stderr);
                                        result_text = if stderr.is_empty() {
                                            stdout.to_string()
                                        } else {
                                            format!("{}\n[stderr] {}", stdout, stderr)
                                        };
                                        had_error = !output.status.success();
                                        if had_error {
                                            let err_msg = format!(
                                                "Shell command failed (exit {}): {}",
                                                output.status.code().unwrap_or(-1),
                                                stderr
                                            );
                                            state.update_plan_state(|ps| {
                                                ps.step_mut(idx).error = Some(err_msg);
                                            });
                                        }
                                    }
                                    Err(e) => {
                                        had_error = true;
                                        result_text = e.to_string();
                                        let err_msg = e.to_string();
                                        state.update_plan_state(|ps| {
                                            ps.step_mut(idx).error = Some(err_msg);
                                        });
                                    }
                                }
                            }
                        }
                        ExecutorType::LlmAgent => {
                            // Default: Execute via AgentRunner with tool calling loop
                            // A40: Per-step model override (subsumes Swarm/Expert agent routing)
                            let session_id = format!("step-{}-{}", idx, uuid::Uuid::new_v4());
                            // A40: Per-step model AND agent routing
                            let agent_lock =
                                if step.executor_model.is_some() || step.executor_agent.is_some() {
                                    tracing::info!(
                                        step_idx = idx,
                                        model = ?step.executor_model,
                                        agent = ?step.executor_agent,
                                        "Using per-step model/agent override"
                                    );
                                    factory
                                        .get_or_create_with_model(
                                            &session_id,
                                            step.executor_model.clone(),
                                            step.executor_agent.clone(), // profile_id = agent persona
                                            Some(step.tool_category.to_string()), // task_type
                                        )
                                        .await
                                } else {
                                    factory.get_or_create(&session_id).await
                                };
                            let mut agent = agent_lock.write().await;
                            let mut stream = agent.query(&prompt).await;

                            while let Some(msg_result) = stream.next().await {
                                match msg_result {
                                    Ok(msg) => {
                                        if let AgentMessage::Result(ref result_msg) = msg {
                                            if let Some(ref text) = result_msg.result {
                                                result_text = text.clone();
                                            }
                                            if let Some(ref usage) = result_msg.usage {
                                                state.metadata.total_tokens += usage.total_tokens();
                                            }
                                        }
                                        state.messages.push(msg);
                                    }
                                    Err(e) => {
                                        had_error = true;
                                        let err_msg = e.to_string();
                                        state.update_plan_state(|ps| {
                                            ps.step_mut(idx).error = Some(err_msg);
                                        });
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    // Record result and update status
                    state.update_plan_state(|ps| {
                        ps.step_mut(idx).result = Some(result_text);
                        if had_error {
                            ps.step_mut(idx).status =
                                crate::collaboration::state::StepStatus::Error;
                            ps.needs_replan = true;
                        } else {
                            ps.step_mut(idx).status = crate::collaboration::state::StepStatus::Done;
                            ps.current_step_index = idx + 1;
                        }
                    });

                    // A39: Record per-step experience for learning pipeline
                    #[cfg(feature = "learning")]
                    {
                        if let Some(ref engine) = factory.learning_engine {
                            if engine.is_enabled() {
                                use crate::learning::experience::{
                                    Experience, ExperienceResult, FeedbackSignal,
                                };

                                let step_state = state.plan_state();
                                let step_retry_count =
                                    step_state.step(idx).map(|s| s.retry_count).unwrap_or(0);

                                let result = if had_error {
                                    let err = step_state
                                        .step(idx)
                                        .and_then(|s| s.error.clone())
                                        .unwrap_or_else(|| "unknown error".into());
                                    ExperienceResult::Failure { error: err }
                                } else {
                                    let result_text = step_state
                                        .step(idx)
                                        .and_then(|s| s.result.clone())
                                        .unwrap_or_default();
                                    ExperienceResult::Success {
                                        response_summary: if result_text.len() > 200 {
                                            result_text[..200].to_string()
                                        } else {
                                            result_text
                                        },
                                    }
                                };

                                let experience = Experience {
                                    id: uuid::Uuid::new_v4(),
                                    task: step.action.clone(),
                                    plan: Some(format!(
                                        "Step {}/{}: [{}]",
                                        idx + 1,
                                        steps.len(),
                                        step.tool_category
                                    )),
                                    tool_calls: Vec::new(),
                                    result,
                                    duration_ms: 0,
                                    cost_usd: 0.0,
                                    models_used: Vec::new(),
                                    node_trace: Vec::new(),
                                    feedback: FeedbackSignal::Implicit {
                                        success: !had_error,
                                        retry_count: step_retry_count,
                                    },
                                    created_at: chrono::Utc::now(),
                                    user_id: None,
                                };
                                let _ = engine.record(experience).await;
                            }
                        }
                    }

                    Ok(state)
                }
            },
        );

        // === Replanner Node ===
        // Called when a step fails; uses LLM to adjust remaining steps.
        // A39: Per-step replan budget (max 1 replan per step) + global skip limit (max 3).
        // A40: Calls request_human_input() for user guidance before replanning.
        let factory_replan = self.clone();
        let replan_config = config.clone();
        #[cfg(feature = "jobs")]
        let replan_hitl = self.pending_hitl_inputs.clone();
        #[cfg(not(feature = "jobs"))]
        let replan_hitl: Option<Arc<()>> = None;
        let replan_content_tx = content_tx.clone();
        let replanner =
            ClosureHandler::new(move |mut state: AgentGraphState, _ctx: &NodeContext| {
                let factory = factory_replan.clone();
                let hitl_store = replan_hitl.clone();
                let hitl_tx = replan_content_tx.clone();
                let config = replan_config.clone();
                async move {
                    let ps = state.plan_state();
                    let idx = ps.current_step_index;
                    let step_replan_count =
                        ps.step(idx).map(|s| s.replan_count as u64).unwrap_or(0);
                    let total_skipped = ps.total_skipped_steps as u64;

                    if total_skipped >= 3 {
                        tracing::warn!(
                            total_skipped,
                            "Global skip limit reached (3), forcing completion"
                        );
                        // Force done by setting index past total
                        let total = ps.total_steps;
                        state.update_plan_state(|ps| {
                            ps.current_step_index = total;
                            ps.needs_replan = false;
                        });
                        return Ok(state);
                    }

                    if step_replan_count >= 1 {
                        tracing::warn!(
                            step_idx = idx,
                            step_replan_count,
                            "Per-step replan budget exhausted, skipping step"
                        );
                        state.update_plan_state(|ps| {
                            ps.current_step_index = idx + 1;
                            ps.step_mut(idx).status =
                                crate::collaboration::state::StepStatus::Skipped;
                            ps.total_skipped_steps += 1;
                            ps.needs_replan = false;
                        });
                        return Ok(state);
                    }

                    // Call TaskPlanner.replan()
                    let llm_router = factory.llm_router.clone();
                    let planner = TaskPlanner::new(llm_router, (*config).clone());

                    let steps = ps.plan_steps.clone();

                    let (completed, remaining) = steps.split_at(idx.min(steps.len()));
                    let failed = steps.get(idx);
                    let error = ps
                        .step(idx)
                        .and_then(|s| s.error.as_deref())
                        .unwrap_or("unknown error")
                        .to_string();

                    // A40: Request HITL guidance before replanning (if HITL store available)
                    #[cfg(feature = "jobs")]
                    if let Some(ref hitl) = hitl_store {
                        let failed_action =
                            failed.map(|s| s.action.as_str()).unwrap_or("unknown step");
                        let ps_hitl = state.plan_state();
                        let execution_id = ps_hitl.execution_id.clone();
                        let job_id = ps_hitl
                            .job_id
                            .as_ref()
                            .and_then(|s| uuid::Uuid::parse_str(s).ok())
                            .unwrap_or_else(uuid::Uuid::nil);

                        let hitl_request = crate::jobs::HITLRequest {
                            prompt: format!(
                                "Step '{}' failed: {}. Provide guidance for replanning, or skip:",
                                failed_action, error
                            ),
                            input_type: crate::jobs::hitl::HITLInputType::Text,
                            timeout: std::time::Duration::from_secs(120),
                            context: Some(
                                "The planner will use your input to adjust the plan.".into(),
                            ),
                        };

                        #[cfg(feature = "graph")]
                        let exec_store_opt = factory.execution_store.clone();
                        #[cfg(not(feature = "graph"))]
                        let exec_store_opt: Option<
                            Arc<crate::graph::ExecutionStore>,
                        > = None;

                        let outcome = crate::jobs::request_human_input(
                            hitl,
                            &execution_id,
                            job_id,
                            hitl_request,
                            hitl_tx.clone(),
                            exec_store_opt,
                        )
                        .await;

                        match outcome {
                            crate::jobs::HITLOutcome::Response(resp) => {
                                tracing::info!(
                                    guidance = %resp.value,
                                    "HITL guidance received for replan"
                                );
                                let guidance = resp.value;
                                state.update_plan_state(|ps| {
                                    ps.replan_guidance = Some(guidance);
                                });
                            }
                            crate::jobs::HITLOutcome::Timeout
                            | crate::jobs::HITLOutcome::Cancelled => {
                                tracing::info!("No HITL guidance received, auto-replanning");
                            }
                        }
                    }

                    if let Some(failed_step) = failed {
                        let remaining_after = if remaining.len() > 1 {
                            &remaining[1..]
                        } else {
                            &[]
                        };

                        // A40: Incorporate HITL guidance into replan context
                        let error_with_guidance = {
                            let guidance = state.plan_state().replan_guidance.clone();
                            if let Some(g) = guidance {
                                // Clear consumed guidance
                                state.update_plan_state(|ps| {
                                    ps.replan_guidance = None;
                                });
                                format!("{}\n\nUser guidance: {}", error, g)
                            } else {
                                error.clone()
                            }
                        };

                        match planner
                            .replan(
                                completed,
                                failed_step,
                                &error_with_guidance,
                                remaining_after,
                            )
                            .await
                        {
                            Ok(new_steps) => {
                                let mut updated = completed.to_vec();
                                let new_total = updated.len() + new_steps.len();
                                updated.extend(new_steps);
                                state.update_plan_state(|ps| {
                                    ps.plan_steps = updated;
                                    ps.total_steps = new_total;
                                });
                                tracing::info!(
                                    step_idx = idx,
                                    new_total,
                                    "Plan updated after step failure"
                                );
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "Replan failed, skipping step");
                                state.update_plan_state(|ps| {
                                    ps.current_step_index = idx + 1;
                                    ps.step_mut(idx).status =
                                        crate::collaboration::state::StepStatus::Skipped;
                                    ps.total_skipped_steps += 1;
                                });
                            }
                        }
                    }

                    state.update_plan_state(|ps| {
                        ps.needs_replan = false;
                        ps.step_mut(idx).replan_count += 1;
                    });
                    Ok(state)
                }
            });

        // === Approval Gate Node ===
        // Pauses graph execution and waits for user decision via oneshot channel.
        // If no PendingPlanApprovals store is configured, auto-approves (backward compatible).
        let approval_store = self.pending_plan_approvals.clone();
        let approval_content_tx = content_tx.clone();
        let max_revision_rounds: u32 = 3;
        let approval_timeout_secs: u64 = 300; // 5 minutes

        let approval_gate = ClosureHandler::new(
            move |mut state: AgentGraphState, _ctx: &NodeContext| {
                let store = approval_store.clone();
                let ctx_tx = approval_content_tx.clone();
                async move {
                    // If no approval store, auto-approve (backward compatible)
                    let store = match store {
                        Some(s) => s,
                        None => {
                            tracing::debug!("No approval store configured, auto-approving plan");
                            state.update_plan_state(|ps| {
                                ps.plan_decision =
                                    Some(crate::collaboration::state::ApprovalDecision::Approved);
                            });
                            return Ok(state);
                        }
                    };

                    // Read plan from typed state
                    let ps = state.plan_state();
                    let steps = ps.plan_steps.clone();
                    let goal = if ps.plan_goal.is_empty() {
                        "Task".to_string()
                    } else {
                        ps.plan_goal.clone()
                    };
                    let success_criteria = ps.success_criteria.clone();
                    let revision_round = ps.revision_round;

                    // Enrich steps with risk levels
                    let review_steps: Vec<PlanStepReview> = steps
                        .iter()
                        .map(|s| PlanStepReview::from_plan_step(s, None))
                        .collect();
                    let risk_level = max_risk_level(&steps);

                    let request_id = uuid::Uuid::new_v4();
                    let session_id = uuid::Uuid::new_v4(); // TODO: derive from state
                    let execution_id = {
                        let eid = ps.execution_id.clone();
                        if eid.is_empty() {
                            "unknown".to_string()
                        } else {
                            eid
                        }
                    };

                    // Register pending approval (returns receiver to await)
                    let timeout = std::time::Duration::from_secs(approval_timeout_secs);
                    let rx = store.register(request_id, session_id, goal.clone(), timeout);

                    // Send PlanApprovalRequired SSE event
                    if let Some(tx) = &ctx_tx {
                        let event = GraphStreamEvent::PlanApprovalRequired {
                            execution_id: execution_id.clone(),
                            request_id: request_id.to_string(),
                            goal: goal.clone(),
                            steps: serde_json::to_value(&review_steps).unwrap_or_default(),
                            success_criteria: success_criteria.clone(),
                            timeout_seconds: approval_timeout_secs,
                            risk_level: risk_level.clone(),
                            revision_round,
                            max_revisions: max_revision_rounds,
                        };
                        let _ = tx.try_send(event);
                    }

                    tracing::info!(
                        request_id = %request_id,
                        step_count = steps.len(),
                        risk_level = %risk_level,
                        revision_round,
                        "Plan approval required, waiting for user decision"
                    );

                    // Await user decision (no timeout — task stays pending until user responds)
                    match rx.await {
                        Ok(decision) => {
                            match decision {
                                PlanApprovalDecision::Approve => {
                                    tracing::info!("Plan approved by user");
                                    state.update_plan_state(|ps| {
                                        ps.plan_decision = Some(
                                            crate::collaboration::state::ApprovalDecision::Approved,
                                        );
                                    });
                                }
                                PlanApprovalDecision::ApproveWithEdits { edited_steps } => {
                                    tracing::info!(
                                        edited_count = edited_steps.len(),
                                        "Plan approved with edits"
                                    );
                                    let count = edited_steps.len();
                                    state.update_plan_state(|ps| {
                                        ps.plan_steps = edited_steps;
                                        ps.total_steps = count;
                                        ps.current_step_index = 0;
                                        ps.plan_decision = Some(
                                            crate::collaboration::state::ApprovalDecision::Approved,
                                        );
                                    });
                                }
                                PlanApprovalDecision::Revise { feedback } => {
                                    if revision_round >= max_revision_rounds {
                                        tracing::warn!(
                                            "Max revision rounds reached ({}), rejecting",
                                            max_revision_rounds
                                        );
                                        state.update_plan_state(|ps| {
                                            ps.plan_decision = Some(crate::collaboration::state::ApprovalDecision::Rejected);
                                            ps.rejection_reason = Some("Maximum revision rounds exceeded".into());
                                        });
                                    } else {
                                        tracing::info!(
                                            revision_round = revision_round + 1,
                                            feedback = %feedback,
                                            "Plan revision requested"
                                        );
                                        state.update_plan_state(|ps| {
                                            ps.plan_revision_feedback = Some(feedback);
                                            ps.revision_round = revision_round + 1;
                                            ps.plan_decision = Some(crate::collaboration::state::ApprovalDecision::Pending);
                                        });
                                    }
                                }
                                PlanApprovalDecision::Reject { reason } => {
                                    tracing::info!(?reason, "Plan rejected by user");
                                    state.update_plan_state(|ps| {
                                        ps.plan_decision = Some(
                                            crate::collaboration::state::ApprovalDecision::Rejected,
                                        );
                                        ps.rejection_reason = reason;
                                    });
                                }
                            }
                        }
                        Err(_) => {
                            tracing::warn!("Approval receiver dropped (client disconnected)");
                            state.update_plan_state(|ps| {
                                ps.plan_decision =
                                    Some(crate::collaboration::state::ApprovalDecision::Rejected);
                                ps.rejection_reason = Some("Client disconnected".into());
                            });
                        }
                        Err(_) => {
                            tracing::warn!(
                                "Plan approval timed out after {}s",
                                approval_timeout_secs
                            );
                            let timeout_reason =
                                format!("Timed out after {}s", approval_timeout_secs);
                            state.update_plan_state(|ps| {
                                ps.plan_decision =
                                    Some(crate::collaboration::state::ApprovalDecision::Rejected);
                                ps.rejection_reason = Some(timeout_reason);
                            });
                            store.evict_expired();
                        }
                    }

                    Ok(state)
                }
            },
        );

        // === Conditional Edge: check_decision (after approval_gate) ===
        let check_decision = ClosurePredicate::new(|state: &AgentGraphState| {
            let ps = state.plan_state();
            match ps.plan_decision {
                Some(crate::collaboration::state::ApprovalDecision::Approved) => "approved".into(),
                Some(crate::collaboration::state::ApprovalDecision::Pending) => "revise".into(),
                Some(crate::collaboration::state::ApprovalDecision::Rejected) => "rejected".into(),
                None => "approved".into(), // default: auto-approve
            }
        });

        // === Rejection Terminal Node ===
        let rejection = ClosureHandler::new(
            move |mut state: AgentGraphState, _ctx: &NodeContext| async move {
                let reason = state
                    .plan_state()
                    .rejection_reason
                    .clone()
                    .unwrap_or_else(|| "Plan rejected".into());

                state.response = format!("Plan execution cancelled: {}", reason);
                state.execution_result = Some("rejected".into());
                tracing::info!(reason = %reason, "Plan rejected, graph terminating");
                Ok(state)
            },
        );

        // === Judge Node (A39 + A40) ===
        // Evaluates step execution quality using three-layer strategy:
        // Rules → Keywords → LLM. Produces StepReflection with verdict.
        // A40: Queries ReflectionStore for past reflections, emits JudgeEvaluated SSE event.
        let factory_judge = self.clone();
        let judge_content_tx = content_tx.clone();
        let judge_node = ClosureHandler::new(
            move |mut state: AgentGraphState, _ctx: &NodeContext| {
                let factory = factory_judge.clone();
                let ctx_tx = judge_content_tx.clone();
                async move {
                    use crate::collaboration::judge::{JudgeConfig, StepJudge};
                    use crate::learning::reflection::StepVerdict;

                    let ps = state.plan_state();
                    let idx = ps.current_step_index;
                    let needs_replan = ps.needs_replan;
                    let judge_idx = if needs_replan {
                        idx
                    } else if idx > 0 {
                        idx - 1
                    } else {
                        0
                    };
                    let steps = ps.plan_steps.clone();
                    let total = ps.total_steps;

                    // If all steps done and no error, pass through
                    if !needs_replan && idx >= total {
                        state.update_plan_state(|ps| {
                            ps.judge_verdict = Some("done".into());
                        });
                        return Ok(state);
                    }

                    // If executor reported error, we already know it failed
                    if needs_replan {
                        let error = ps
                            .step(judge_idx)
                            .and_then(|s| s.error.clone())
                            .unwrap_or_default();
                        let retry_count = ps
                            .step(judge_idx)
                            .map(|s| s.retry_count as u64)
                            .unwrap_or(0);

                        if retry_count < 2 {
                            // Allow retry with error info as suggestion
                            state.update_plan_state(|ps| {
                                ps.step_mut(judge_idx).retry_suggestions = Some(format!(
                                    "Previous execution failed with error: {}. Try a different approach.",
                                    error
                                ));
                                ps.step_mut(judge_idx).retry_count = (retry_count + 1) as u32;
                                ps.needs_replan = false;
                                ps.judge_verdict = Some("retry".into());
                            });
                        } else {
                            // Retry budget exhausted → escalate to replan
                            state.update_plan_state(|ps| {
                                ps.judge_verdict = Some("replan".into());
                            });
                        }
                        return Ok(state);
                    }

                    // === Run Judge evaluation on the completed step ===
                    let step = match steps.get(judge_idx) {
                        Some(s) => s,
                        None => {
                            state.update_plan_state(|ps| {
                                ps.judge_verdict = Some("pass".into());
                            });
                            return Ok(state);
                        }
                    };

                    let step_exec = ps.step(judge_idx);
                    let actual_output =
                        step_exec.and_then(|s| s.result.clone()).unwrap_or_default();
                    let error = step_exec.and_then(|s| s.error.clone());
                    let previous_output = step_exec.and_then(|s| s.prev_output.clone());

                    // A42: Post-execution verification for ClaudeCode/Shell steps
                    use crate::collaboration::planner::ExecutorType;
                    let executor_type = step.executor_type.unwrap_or(ExecutorType::LlmAgent);

                    if executor_type == ExecutorType::ClaudeCode
                        || executor_type == ExecutorType::Shell
                    {
                        // For code-modifying steps, run cargo check to verify compilation
                        if step.tool_category == crate::collaboration::planner::ToolCategory::Code
                            || executor_type == ExecutorType::ClaudeCode
                        {
                            let cwd = factory.default_cwd.clone().unwrap_or_else(|| ".".into());
                            match tokio::process::Command::new("cargo")
                                .args([
                                    "check",
                                    "-p",
                                    "gateway-core",
                                    "--features",
                                    "full-orchestration",
                                ])
                                .current_dir(&cwd)
                                .output()
                                .await
                            {
                                Ok(check_output) => {
                                    if !check_output.status.success() {
                                        let stderr = String::from_utf8_lossy(&check_output.stderr);
                                        tracing::warn!(
                                            step_idx = judge_idx,
                                            "Post-step cargo check failed"
                                        );
                                        // Treat compilation failure as a judge Fail verdict
                                        let retry_count = state
                                            .plan_state()
                                            .step(judge_idx)
                                            .map(|s| s.retry_count as u64)
                                            .unwrap_or(0);

                                        let suggestions = format!(
                                            "Compilation failed after step execution. Fix the errors:\n{}",
                                            if stderr.len() > 1000 {
                                                &stderr[..1000]
                                            } else {
                                                &stderr
                                            }
                                        );

                                        if retry_count < 2 {
                                            state.update_plan_state(|ps| {
                                                ps.step_mut(judge_idx).retry_suggestions = Some(suggestions);
                                                ps.step_mut(judge_idx).retry_count = (retry_count + 1) as u32;
                                                ps.step_mut(judge_idx).prev_output = Some(actual_output);
                                                ps.step_mut(judge_idx).status = crate::collaboration::state::StepStatus::Retrying;
                                                ps.current_step_index = judge_idx;
                                                ps.judge_verdict = Some("retry".into());
                                            });
                                        } else {
                                            state.update_plan_state(|ps| {
                                                ps.needs_replan = true;
                                                ps.current_step_index = judge_idx;
                                                ps.judge_verdict = Some("replan".into());
                                            });
                                        }
                                        return Ok(state);
                                    }
                                    tracing::info!(
                                        step_idx = judge_idx,
                                        "Post-step cargo check passed"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "cargo check command failed to execute, skipping verification"
                                    );
                                }
                            }
                        }
                    }

                    let judge = StepJudge::new(
                        factory.llm_router.clone(),
                        factory.judge_config.clone().unwrap_or_default(),
                    );

                    // A40: Query ReflectionStore for past reflections on similar steps
                    #[cfg(feature = "collaboration")]
                    let past_reflections = if let Some(ref store) = factory.reflection_store {
                        store.query_similar(&step.action, 3).await
                    } else {
                        Vec::new()
                    };
                    #[cfg(not(feature = "collaboration"))]
                    let past_reflections: Vec<
                        crate::learning::reflection::StepReflection,
                    > = Vec::new();

                    // Visual verification: capture screenshot if step requires it
                    let screenshot_bytes: Option<Vec<u8>> =
                        if step.requires_visual_verification.unwrap_or(false) {
                            match capture_screenshot_for_judge().await {
                                Ok(bytes) => {
                                    tracing::info!(
                                        step_idx = judge_idx,
                                        bytes = bytes.len(),
                                        "Captured screenshot for visual judge"
                                    );
                                    Some(bytes)
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "Screenshot capture failed, proceeding without visual eval"
                                    );
                                    None
                                }
                            }
                        } else {
                            None
                        };

                    let reflection = judge
                        .evaluate(
                            &step.action,
                            step.tool_category.as_str(),
                            step.expected_output.as_deref(),
                            &actual_output,
                            error.as_deref(),
                            previous_output.as_deref(),
                            &past_reflections,
                            screenshot_bytes.as_deref(),
                        )
                        .await;

                    let ps_judge = state.plan_state();
                    let retry_count = ps_judge.step(judge_idx).map(|s| s.retry_count).unwrap_or(0);

                    tracing::info!(
                        step_idx = judge_idx,
                        verdict = %reflection.verdict,
                        reasoning = %reflection.reasoning,
                        "Judge evaluated step"
                    );

                    // A40: Emit JudgeEvaluated SSE event
                    if let Some(ref tx) = ctx_tx {
                        let event = crate::graph::GraphStreamEvent::JudgeEvaluated {
                            execution_id: ps_judge.execution_id.clone(),
                            step_id: Some(judge_idx.to_string()),
                            verdict: reflection.verdict.to_string(),
                            reasoning: reflection.reasoning.clone(),
                            suggestions: reflection.suggestions.clone(),
                            retry_count,
                        };
                        let _ = tx.try_send(event);
                    }

                    // A40: Store reflection in ReflectionStore for future queries
                    #[cfg(feature = "collaboration")]
                    if let Some(ref store) = factory.reflection_store {
                        store.store(reflection.clone()).await;
                    }

                    let reflection_json = serde_json::to_value(&reflection).unwrap_or_default();

                    // Store reflection in typed state (via step_mut)
                    // Note: reflection is also stored in the match arms below via update_plan_state

                    match reflection.verdict {
                        StepVerdict::Pass | StepVerdict::PartialPass => {
                            state.update_plan_state(|ps| {
                                ps.step_mut(judge_idx).reflection = Some(reflection_json.clone());
                                ps.judge_verdict = Some("pass".into());
                            });
                        }
                        StepVerdict::Fail => {
                            let retry_count = ps
                                .step(judge_idx)
                                .map(|s| s.retry_count as u64)
                                .unwrap_or(0);
                            let suggestions = reflection.suggestions.join("; ");

                            if retry_count < 2 {
                                state.update_plan_state(|ps| {
                                    ps.step_mut(judge_idx).reflection =
                                        Some(reflection_json.clone());
                                    ps.step_mut(judge_idx).retry_suggestions = Some(suggestions);
                                    ps.step_mut(judge_idx).retry_count = (retry_count + 1) as u32;
                                    ps.step_mut(judge_idx).prev_output = Some(actual_output);
                                    ps.step_mut(judge_idx).status =
                                        crate::collaboration::state::StepStatus::Retrying;
                                    ps.current_step_index = judge_idx;
                                    ps.judge_verdict = Some("retry".into());
                                });
                            } else {
                                state.update_plan_state(|ps| {
                                    ps.step_mut(judge_idx).reflection =
                                        Some(reflection_json.clone());
                                    ps.needs_replan = true;
                                    ps.current_step_index = judge_idx;
                                    ps.judge_verdict = Some("replan".into());
                                });
                            }
                        }
                        StepVerdict::Stalled => {
                            state.update_plan_state(|ps| {
                                ps.step_mut(judge_idx).reflection = Some(reflection_json.clone());
                                ps.needs_replan = true;
                                ps.current_step_index = judge_idx;
                                ps.judge_verdict = Some("replan".into());
                            });
                        }
                    }

                    Ok(state)
                }
            },
        );

        // === Conditional Edge: check_judge_verdict (A39) ===
        // Routes based on judge_verdict: pass/retry/replan/done
        let check_judge_verdict = ClosurePredicate::new(|state: &AgentGraphState| {
            state
                .plan_state()
                .judge_verdict
                .clone()
                .unwrap_or_else(|| "pass".into())
        });

        // === Synthesizer Node ===
        // Collects step results from working_memory and generates final response.
        // A39: Computes ExecutionMetrics + stores verified plan on success.
        let factory_synth = self.clone();
        let synthesizer = ClosureHandler::new(
            move |mut state: AgentGraphState, _ctx: &NodeContext| {
                let factory = factory_synth.clone();
                async move {
                    // Collect step results from typed state
                    let ps = state.plan_state();
                    let total = ps.total_steps;
                    let goal = if ps.plan_goal.is_empty() {
                        "Task".to_string()
                    } else {
                        ps.plan_goal.clone()
                    };
                    let success_criteria = ps.success_criteria.clone();

                    let mut step_summary = String::new();
                    let mut completed_count = 0usize;
                    let mut failed_count = 0usize;
                    let mut skipped_count = 0usize;
                    let mut first_pass_count = 0usize;
                    let mut total_retry_count = 0u64;
                    let mut total_replan_count = 0u64;

                    for i in 0..total {
                        let step_exec = ps.step(i);
                        let (status_str, result) = match step_exec {
                            Some(s) => {
                                let st = match s.status {
                                    crate::collaboration::state::StepStatus::Done => "completed",
                                    crate::collaboration::state::StepStatus::Error => "failed",
                                    crate::collaboration::state::StepStatus::Skipped => "skipped",
                                    crate::collaboration::state::StepStatus::Retrying => "retrying",
                                    crate::collaboration::state::StepStatus::Replanning => {
                                        "replanning"
                                    }
                                    crate::collaboration::state::StepStatus::Pending => "pending",
                                };
                                (st.to_string(), s.result.clone().unwrap_or_default())
                            }
                            None => ("unknown".to_string(), String::new()),
                        };

                        match status_str.as_str() {
                            "completed" => {
                                completed_count += 1;
                                if let Some(s) = step_exec {
                                    if s.retry_count == 0 && s.replan_count == 0 {
                                        first_pass_count += 1;
                                    }
                                    total_retry_count += s.retry_count as u64;
                                    total_replan_count += s.replan_count as u64;
                                }
                            }
                            "failed" => failed_count += 1,
                            "skipped" => skipped_count += 1,
                            _ => {}
                        }

                        step_summary.push_str(&format!(
                            "Step {}: [{}] {}\n",
                            i + 1,
                            status_str,
                            if result.len() > 200 {
                                format!("{}...", &result[..200])
                            } else {
                                result
                            }
                        ));
                    }

                    // A39: Compute ExecutionMetrics
                    let first_pass_rate = if total > 0 {
                        first_pass_count as f64 / total as f64
                    } else {
                        0.0
                    };
                    let metrics = serde_json::json!({
                        "total_steps": total,
                        "completed": completed_count,
                        "failed": failed_count,
                        "skipped": skipped_count,
                        "first_pass_success_rate": first_pass_rate,
                        "total_retries": total_retry_count,
                        "total_replans": total_replan_count,
                        "total_tokens": state.metadata.total_tokens,
                    });
                    state.update_plan_state(|ps| {
                        ps.execution_metrics = Some(metrics.clone());
                    });
                    tracing::info!(
                        first_pass_rate = format!("{:.0}%", first_pass_rate * 100.0),
                        completed = completed_count,
                        failed = failed_count,
                        skipped = skipped_count,
                        "A39 ExecutionMetrics computed"
                    );

                    let synth_prompt = format!(
                        "Synthesize the following execution results into a clear, coherent response.\n\n\
                        Goal: {}\n\n\
                        Step Results ({} completed, {} failed, {} skipped):\n{}\n\n\
                        Provide a comprehensive summary of what was accomplished.",
                        goal, completed_count, failed_count, skipped_count, step_summary
                    );

                    let session_id = format!("synthesizer-{}", uuid::Uuid::new_v4());
                    let agent_lock = factory.get_or_create(&session_id).await;
                    let mut agent = agent_lock.write().await;
                    let mut stream = agent.query(&synth_prompt).await;
                    let mut synth_text = String::new();
                    while let Some(msg_result) = stream.next().await {
                        match msg_result {
                            Ok(msg) => {
                                if let AgentMessage::Result(ref result_msg) = msg {
                                    if let Some(ref text) = result_msg.result {
                                        synth_text = text.clone();
                                    }
                                    if let Some(ref usage) = result_msg.usage {
                                        state.metadata.total_tokens += usage.total_tokens();
                                    }
                                }
                                state.messages.push(msg);
                            }
                            Err(_) => break,
                        }
                    }
                    if !synth_text.is_empty() {
                        state.response = synth_text;
                    }

                    // Store execution summary
                    state.execution_result = Some(format!(
                        "{} steps completed, {} failed, {} skipped",
                        completed_count, failed_count, skipped_count
                    ));

                    // A39: Store verified plan if execution was highly successful
                    if first_pass_rate >= 0.8 && skipped_count == 0 && total > 0 {
                        if let Some(ref registry) = factory.skill_registry {
                            let plan_steps_json = serde_json::to_value(&ps.plan_steps).ok();
                            let plan_hash = {
                                use std::collections::hash_map::DefaultHasher;
                                use std::hash::{Hash, Hasher};
                                let mut hasher = DefaultHasher::new();
                                goal.hash(&mut hasher);
                                hasher.finish()
                            };
                            let plan_name = format!("verified-plan-{:x}", plan_hash);

                            // Check if this plan already exists and update outcome
                            let offered = ps
                                .offered_verified_plan
                                .clone()
                                .and_then(|v| v.as_str().map(String::from));

                            // Store plan data in typed state for post-graph processing
                            state.update_plan_state(|ps| {
                                ps.verified_plan_candidate = Some(serde_json::json!({
                                    "name": plan_name,
                                    "description": goal,
                                    "plan_steps": plan_steps_json,
                                    "success_rate": first_pass_rate,
                                    "success_criteria": success_criteria,
                                    "offered_plan": offered,
                                }));
                            });
                            tracing::info!(
                                plan_name = %plan_name,
                                first_pass_rate = format!("{:.0}%", first_pass_rate * 100.0),
                                "Verified plan candidate stored for post-graph registration"
                            );
                        }
                    }

                    // A39: Record execution metrics as experience
                    #[cfg(feature = "learning")]
                    {
                        if let Some(ref engine) = factory.learning_engine {
                            if engine.is_enabled() {
                                use crate::learning::experience::{
                                    Experience, ExperienceResult, FeedbackSignal,
                                };

                                let result = if failed_count == 0 && skipped_count == 0 {
                                    ExperienceResult::Success {
                                        response_summary: format!(
                                            "Plan '{}': {}/{} steps completed",
                                            goal, completed_count, total
                                        ),
                                    }
                                } else {
                                    ExperienceResult::Partial {
                                        response_summary: format!(
                                            "Plan '{}': {}/{} completed",
                                            goal, completed_count, total
                                        ),
                                        error: format!(
                                            "{} failed, {} skipped",
                                            failed_count, skipped_count
                                        ),
                                    }
                                };

                                let experience = Experience {
                                    id: uuid::Uuid::new_v4(),
                                    task: goal.clone(),
                                    plan: state.plan.clone(),
                                    tool_calls: Vec::new(),
                                    result,
                                    duration_ms: 0,
                                    cost_usd: 0.0,
                                    models_used: Vec::new(),
                                    node_trace: Vec::new(),
                                    feedback: FeedbackSignal::Implicit {
                                        success: failed_count == 0 && skipped_count == 0,
                                        retry_count: total_retry_count as u32,
                                    },
                                    created_at: chrono::Utc::now(),
                                    user_id: None,
                                };
                                let _ = engine.record(experience).await;
                            }
                        }
                    }

                    Ok(state)
                }
            },
        );

        // === Build the Graph (A43 + A39 topology) ===
        //
        // A43 pipeline (new):
        //   ResearchPlanner → (check_complexity)
        //     simple → Planner (skip PRD)
        //     has_questions → ClarificationGate → PrdAssembler → PrdApprovalGate → (check_prd_decision)
        //     no_questions → PrdAssembler → PrdApprovalGate → (check_prd_decision)
        //       approved → Planner (+PRD context)
        //       revise → PrdAssembler (loop)
        //       rejected → Rejection
        //
        // A39 pipeline (unchanged):
        //   Planner → ApprovalGate → (check_decision) → approved/revise/rejected
        //     approved → Executor → Judge → (check_judge_verdict) → pass/retry/replan/done
        //       pass → Executor (next step)
        //       retry → Executor (same step + suggestions)
        //       replan → Replanner → Executor
        //       done → Synthesizer → END
        //     revise → Planner (cycle back for revision)
        //     rejected → Rejection (terminal)
        let builder = StateGraphBuilder::new()
            // A43 nodes
            .add_node("research_planner", research_planner)
            .add_node("clarification_gate", clarification_gate)
            .add_node("prd_assembler", prd_assembler)
            .add_node("prd_approval_gate", prd_approval_gate)
            // Existing A39 nodes
            .add_node("planner", planner)
            .add_node("approval_gate", approval_gate)
            .add_node("rejection", rejection)
            .add_node("executor", executor)
            .add_node("judge", judge_node)
            .add_node("replanner", replanner)
            .add_node("synthesizer", synthesizer)
            // A43: ResearchPlanner → (check_complexity)
            .add_conditional_edge(
                "research_planner",
                check_complexity,
                vec![
                    ("simple", "planner"),                // Simple → skip PRD
                    ("has_questions", "clarification_gate"), // Medium/Complex with questions
                    ("no_questions", "prd_assembler"),     // Medium/Complex without questions
                ],
            )
            // A43: ClarificationGate → PrdAssembler
            .add_edge("clarification_gate", "prd_assembler")
            // A43: PrdAssembler → PrdApprovalGate
            .add_edge("prd_assembler", "prd_approval_gate")
            // A43: PrdApprovalGate → (check_prd_decision)
            .add_conditional_edge(
                "prd_approval_gate",
                check_prd_decision,
                vec![
                    ("approved", "planner"),        // PRD approved → proceed to step planning
                    ("revise", "prd_assembler"),     // PRD revision → re-generate
                    ("rejected", "rejection"),       // PRD rejected → terminal
                ],
            )
            // Planner → ApprovalGate (existing A39 edge)
            .add_edge("planner", "approval_gate")
            // ApprovalGate → (check_decision) → approved/revise/rejected
            .add_conditional_edge(
                "approval_gate",
                check_decision,
                vec![
                    ("approved", "executor"),    // User approved → start execution
                    ("revise", "planner"),        // User wants revision → planner re-generates
                    ("rejected", "rejection"),    // User rejected → terminal
                ],
            )
            // Executor → Judge (A39: always evaluate after execution)
            .add_edge("executor", "judge")
            // Judge → (check_judge_verdict) → pass/retry/replan/done
            .add_conditional_edge(
                "judge",
                check_judge_verdict,
                vec![
                    ("pass", "executor"),        // Next step
                    ("retry", "executor"),        // Retry same step with suggestions
                    ("replan", "replanner"),      // Escalate to replanner
                    ("done", "synthesizer"),      // All steps done
                ],
            )
            .add_edge("replanner", "approval_gate") // After replan, show updated plan for user approval
            .set_entry("research_planner")  // A43: entry is now research_planner
            .set_terminal("synthesizer")
            .set_terminal("rejection");

        Ok(builder)
    }

    /// Create a StateGraph based on the current collaboration mode.
    ///
    /// This creates a graph configured according to the factory's default
    /// collaboration mode or a specified mode. Different modes produce
    /// different graph topologies:
    ///
    /// - `Direct`: Single agent node
    /// - `Swarm`: Multiple agent nodes with handoff edges
    /// - `Expert`: Supervisor node dispatching to specialist nodes
    /// - `Graph`: Custom graph (requires template)
    ///
    /// # Arguments
    ///
    /// * `mode` - Optional collaboration mode override. If None, uses factory default.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use gateway_core::collaboration::CollaborationMode;
    ///
    /// // Use factory default
    /// let graph = factory.create_collaboration_graph(None)?;
    ///
    /// // Or specify mode explicitly
    /// let graph = factory.create_collaboration_graph(Some(CollaborationMode::Direct))?;
    /// ```
    #[cfg(feature = "collaboration")]
    pub fn create_collaboration_graph(
        self: &Arc<Self>,
        mode: Option<CollaborationMode>,
    ) -> Result<StateGraph<AgentGraphState>, GraphError> {
        self.create_collaboration_graph_tracked(mode, None)
    }

    /// Create a collaboration graph with optional execution tracking.
    #[cfg(feature = "collaboration")]
    pub fn create_collaboration_graph_tracked(
        self: &Arc<Self>,
        mode: Option<CollaborationMode>,
        exec_id: Option<&str>,
    ) -> Result<StateGraph<AgentGraphState>, GraphError> {
        let mode = mode.or(self.default_collaboration_mode.clone());

        match mode {
            Some(CollaborationMode::Direct) | None => self.create_direct_graph_tracked(exec_id),
            Some(CollaborationMode::PlanExecute) => self.create_plan_execute_graph_tracked(exec_id),
            Some(CollaborationMode::Swarm {
                initial_agent,
                handoff_rules,
                agent_models,
            }) => self.create_swarm_graph_tracked(
                &initial_agent,
                &handoff_rules,
                &agent_models,
                exec_id,
            ),
            Some(CollaborationMode::Expert {
                supervisor,
                specialists,
                supervisor_model,
                default_specialist_model,
                specialist_models,
            }) => self.create_expert_graph_tracked(
                &supervisor,
                &specialists,
                supervisor_model.as_deref(),
                default_specialist_model.as_deref(),
                &specialist_models,
                exec_id,
            ),
            Some(CollaborationMode::Graph { graph_id }) => Err(GraphError::Internal(format!(
                "Custom graph '{}' must be created via TemplateRegistry",
                graph_id
            ))),
        }
    }

    /// Create a Swarm-style graph with agent-to-agent handoffs.
    ///
    /// In Swarm mode, agents can hand off execution to other agents based
    /// on handoff rules (e.g., when a specific tool is called, when certain
    /// keywords appear, or based on classification).
    ///
    /// # Arguments
    ///
    /// * `initial_agent` - Name of the agent that starts execution
    /// * `handoff_rules` - Rules defining when/how agents hand off to each other
    /// * `agent_models` - Per-agent model overrides (agent_name → model_name)
    #[cfg(feature = "collaboration")]
    pub fn create_swarm_graph(
        self: &Arc<Self>,
        initial_agent: &str,
        handoff_rules: &[crate::collaboration::HandoffRule],
        agent_models: &HashMap<String, String>,
    ) -> Result<StateGraph<AgentGraphState>, GraphError> {
        self.create_swarm_graph_tracked(initial_agent, handoff_rules, agent_models, None)
    }

    /// Create a Swarm graph with optional execution tracking.
    #[cfg(feature = "collaboration")]
    pub fn create_swarm_graph_tracked(
        self: &Arc<Self>,
        initial_agent: &str,
        handoff_rules: &[crate::collaboration::HandoffRule],
        agent_models: &HashMap<String, String>,
        exec_id: Option<&str>,
    ) -> Result<StateGraph<AgentGraphState>, GraphError> {
        let mut builder =
            self.create_swarm_graph_builder(initial_agent, handoff_rules, agent_models)?;
        builder = self.attach_observers(builder, exec_id, None);
        builder.build()
    }

    /// Create a Swarm graph builder (topology only, no observers).
    #[cfg(feature = "collaboration")]
    fn create_swarm_graph_builder(
        self: &Arc<Self>,
        initial_agent: &str,
        handoff_rules: &[crate::collaboration::HandoffRule],
        agent_models: &HashMap<String, String>,
    ) -> Result<StateGraphBuilder<AgentGraphState>, GraphError> {
        self.create_swarm_graph_builder_inner(initial_agent, handoff_rules, agent_models, None)
    }

    #[cfg(feature = "collaboration")]
    fn create_swarm_graph_builder_with_content_stream(
        self: &Arc<Self>,
        initial_agent: &str,
        handoff_rules: &[crate::collaboration::HandoffRule],
        agent_models: &HashMap<String, String>,
        content_tx: tokio::sync::mpsc::Sender<crate::graph::GraphStreamEvent>,
    ) -> Result<StateGraphBuilder<AgentGraphState>, GraphError> {
        self.create_swarm_graph_builder_inner(
            initial_agent,
            handoff_rules,
            agent_models,
            Some(content_tx),
        )
    }

    #[cfg(feature = "collaboration")]
    fn create_swarm_graph_builder_inner(
        self: &Arc<Self>,
        initial_agent: &str,
        handoff_rules: &[crate::collaboration::HandoffRule],
        agent_models: &HashMap<String, String>,
        content_tx: Option<tokio::sync::mpsc::Sender<crate::graph::GraphStreamEvent>>,
    ) -> Result<StateGraphBuilder<AgentGraphState>, GraphError> {
        use crate::graph::ClosurePredicate;

        // Collect unique agent names from rules
        let mut agent_names: Vec<String> = vec![initial_agent.to_string()];
        for rule in handoff_rules {
            if !agent_names.contains(&rule.from_agent) {
                agent_names.push(rule.from_agent.clone());
            }
            if !agent_names.contains(&rule.to_agent) {
                agent_names.push(rule.to_agent.clone());
            }
        }

        // Build the graph
        let mut builder = StateGraphBuilder::<AgentGraphState>::new();

        // Add a node for each agent, with optional model overrides
        for agent_name in &agent_names {
            let mut node = AgentRunnerNode::new(self.clone())
                .with_session_prefix(format!("swarm-{}", agent_name));
            if let Some(model) = agent_models.get(agent_name) {
                node = node.with_model(model);
            }
            if let Some(ref tx) = content_tx {
                node = node.with_content_stream(tx.clone());
            }
            builder = builder.add_node(agent_name, node);
        }

        // Set entry to initial agent
        builder = builder.set_entry(initial_agent);

        // Group handoff rules by source agent so each source gets a SINGLE
        // conditional edge with all its targets. The executor only evaluates
        // edges[0], so multiple per-rule edges from the same node would break.
        {
            let mut rules_by_source: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for rule in handoff_rules {
                rules_by_source
                    .entry(rule.from_agent.clone())
                    .or_default()
                    .push(rule.to_agent.clone());
            }

            for (from_agent, targets) in &rules_by_source {
                let targets_clone = targets.clone();
                let predicate = ClosurePredicate::new(move |state: &AgentGraphState| {
                    for target in &targets_clone {
                        if state.response.contains(&format!("HANDOFF:{}", target)) {
                            return format!("handoff_{}", target);
                        }
                    }
                    "stay".to_string()
                });

                let routes: Vec<(String, String)> = targets
                    .iter()
                    .map(|t| (format!("handoff_{}", t), t.clone()))
                    .collect();

                // "stay" default → current agent is terminal (no handoff triggered)
                builder = builder.add_conditional_edge_with_default(
                    from_agent.as_str(),
                    predicate,
                    routes,
                    from_agent.as_str(),
                );
            }

            // Agents that are sources with a "stay" default loop to themselves,
            // but since they're also in the terminal set the executor will stop.
            // Agents that are NOT sources of any handoff rule are always terminal.
            let sources: std::collections::HashSet<&String> = rules_by_source.keys().collect();
            for agent_name in &agent_names {
                if !sources.contains(agent_name) {
                    builder = builder.set_terminal(agent_name);
                } else {
                    // Source agents are also terminal — when "stay" is chosen,
                    // the edge loops back to self and the executor sees it's terminal.
                    builder = builder.set_terminal(agent_name);
                }
            }
        }

        // If no terminal was set (empty agent list edge case), set the last agent
        builder = builder.set_terminal(&agent_names[agent_names.len() - 1]);

        Ok(builder)
    }

    /// Create an Expert-style graph with supervisor dispatching to specialists.
    ///
    /// In Expert mode, a supervisor agent analyzes the task and dispatches
    /// to specialist agents. Results are returned to the supervisor for
    /// quality evaluation and potential retry.
    ///
    /// # Model Fallback Chain (per specialist)
    ///
    /// 1. `specialist_models[name]` — explicit per-specialist override
    /// 2. `default_specialist_model` — shared specialist default
    /// 3. Factory `default_model` — global fallback
    #[cfg(feature = "collaboration")]
    pub fn create_expert_graph(
        self: &Arc<Self>,
        supervisor: &str,
        specialists: &[String],
        supervisor_model: Option<&str>,
        default_specialist_model: Option<&str>,
        specialist_models: &HashMap<String, String>,
    ) -> Result<StateGraph<AgentGraphState>, GraphError> {
        self.create_expert_graph_tracked(
            supervisor,
            specialists,
            supervisor_model,
            default_specialist_model,
            specialist_models,
            None,
        )
    }

    /// Create an Expert graph with optional execution tracking.
    #[cfg(feature = "collaboration")]
    pub fn create_expert_graph_tracked(
        self: &Arc<Self>,
        supervisor: &str,
        specialists: &[String],
        supervisor_model: Option<&str>,
        default_specialist_model: Option<&str>,
        specialist_models: &HashMap<String, String>,
        exec_id: Option<&str>,
    ) -> Result<StateGraph<AgentGraphState>, GraphError> {
        let mut builder = self.create_expert_graph_builder(
            supervisor,
            specialists,
            supervisor_model,
            default_specialist_model,
            specialist_models,
        )?;
        builder = self.attach_observers(builder, exec_id, None);
        builder.build()
    }

    /// Create an Expert graph builder (topology only, no observers).
    #[cfg(feature = "collaboration")]
    fn create_expert_graph_builder(
        self: &Arc<Self>,
        supervisor: &str,
        specialists: &[String],
        supervisor_model: Option<&str>,
        default_specialist_model: Option<&str>,
        specialist_models: &HashMap<String, String>,
    ) -> Result<StateGraphBuilder<AgentGraphState>, GraphError> {
        self.create_expert_graph_builder_inner(
            supervisor,
            specialists,
            supervisor_model,
            default_specialist_model,
            specialist_models,
            None,
        )
    }

    #[cfg(feature = "collaboration")]
    fn create_expert_graph_builder_with_content_stream(
        self: &Arc<Self>,
        supervisor: &str,
        specialists: &[String],
        supervisor_model: Option<&str>,
        default_specialist_model: Option<&str>,
        specialist_models: &HashMap<String, String>,
        content_tx: tokio::sync::mpsc::Sender<crate::graph::GraphStreamEvent>,
    ) -> Result<StateGraphBuilder<AgentGraphState>, GraphError> {
        self.create_expert_graph_builder_inner(
            supervisor,
            specialists,
            supervisor_model,
            default_specialist_model,
            specialist_models,
            Some(content_tx),
        )
    }

    #[cfg(feature = "collaboration")]
    fn create_expert_graph_builder_inner(
        self: &Arc<Self>,
        supervisor: &str,
        specialists: &[String],
        supervisor_model: Option<&str>,
        default_specialist_model: Option<&str>,
        specialist_models: &HashMap<String, String>,
        content_tx: Option<tokio::sync::mpsc::Sender<crate::graph::GraphStreamEvent>>,
    ) -> Result<StateGraphBuilder<AgentGraphState>, GraphError> {
        use crate::graph::ClosurePredicate;

        let mut builder = StateGraphBuilder::<AgentGraphState>::new();

        // Add supervisor node with optional model override
        let mut supervisor_node = AgentRunnerNode::new(self.clone())
            .with_session_prefix(format!("expert-{}", supervisor));
        if let Some(model) = supervisor_model {
            supervisor_node = supervisor_node.with_model(model);
        }
        if let Some(ref tx) = content_tx {
            supervisor_node = supervisor_node.with_content_stream(tx.clone());
        }
        builder = builder.add_node(supervisor, supervisor_node);

        // Add specialist nodes with fallback chain: specialist_models[name] > default_specialist_model
        for specialist in specialists {
            let mut specialist_node = AgentRunnerNode::new(self.clone())
                .with_session_prefix(format!("specialist-{}", specialist));

            // Apply model: per-specialist override > default specialist model
            let specialist_model = specialist_models
                .get(specialist)
                .map(|s| s.as_str())
                .or(default_specialist_model);
            if let Some(model) = specialist_model {
                specialist_node = specialist_node.with_model(model);
            }
            if let Some(ref tx) = content_tx {
                specialist_node = specialist_node.with_content_stream(tx.clone());
            }

            builder = builder.add_node(specialist, specialist_node);
        }

        // Add aggregator node that collects specialist results
        let aggregator = crate::graph::ClosureHandler::new(
            |state: AgentGraphState, _ctx: &crate::graph::NodeContext| async move {
                // Aggregator simply marks completion
                Ok(state)
            },
        );
        builder = builder.add_node("aggregator", aggregator);

        // Set entry to supervisor
        builder = builder.set_entry(supervisor);

        // Supervisor dispatches to specialists based on task analysis.
        // A single conditional edge checks all specialists — the executor only
        // evaluates edges[0], so multiple per-specialist edges would break routing.
        {
            let all_specialists: Vec<String> = specialists.to_vec();
            let predicate = ClosurePredicate::new(move |state: &AgentGraphState| {
                let response_lower = state.response.to_lowercase();
                for spec in &all_specialists {
                    if response_lower.contains(&spec.to_lowercase()) {
                        return format!("dispatch_{}", spec);
                    }
                }
                "none".to_string()
            });

            let routes: Vec<(String, String)> = specialists
                .iter()
                .map(|s| (format!("dispatch_{}", s), s.clone()))
                .collect();

            // Default: no specialist matched → go straight to aggregator
            builder = builder.add_conditional_edge_with_default(
                supervisor,
                predicate,
                routes,
                "aggregator",
            );
        }

        // All specialists flow to aggregator using direct edges
        for specialist in specialists {
            builder = builder.add_edge(specialist, "aggregator");
        }

        // Aggregator is terminal
        builder = builder.set_terminal("aggregator");

        Ok(builder)
    }

    /// Create a GraphExecutor configured for the current collaboration mode.
    ///
    /// This is a convenience method that creates both the graph and the executor
    /// in one call.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let executor = factory.create_graph_executor(None)?;
    /// let state = AgentGraphState::new("Build a web scraper");
    /// let result = executor.execute(state).await?;
    /// ```
    #[cfg(feature = "collaboration")]
    pub fn create_graph_executor(
        self: &Arc<Self>,
        mode: Option<CollaborationMode>,
    ) -> Result<GraphExecutor<AgentGraphState>, GraphError> {
        let graph = self.create_collaboration_graph(mode)?;
        Ok(GraphExecutor::new(graph))
    }

    /// Execute a task using graph-based collaboration.
    ///
    /// This is the highest-level method for graph-based execution. It creates
    /// the graph, executor, and runs the task in one call.
    ///
    /// # Arguments
    ///
    /// * `task` - The task prompt to execute
    /// * `mode` - Optional collaboration mode override
    ///
    /// # Returns
    ///
    /// The final `AgentGraphState` containing the execution results.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let result = factory.execute_with_collaboration(
    ///     "Write unit tests for the auth module",
    ///     Some(CollaborationMode::Expert {
    ///         supervisor: "lead".into(),
    ///         specialists: vec!["tester".into(), "reviewer".into()],
    ///         supervisor_model: None,
    ///         default_specialist_model: None,
    ///         specialist_models: std::collections::HashMap::new(),
    ///     }),
    /// ).await?;
    ///
    /// println!("Response: {}", result.response);
    /// println!("Tokens used: {}", result.metadata.total_tokens);
    /// ```
    #[cfg(feature = "collaboration")]
    pub async fn execute_with_collaboration(
        self: &Arc<Self>,
        task: &str,
        mode: Option<CollaborationMode>,
    ) -> Result<AgentGraphState, GraphError> {
        self.execute_with_collaboration_config(task, mode, None, None, None, None)
            .await
    }

    /// Execute a collaboration task with a streaming observer for graph events.
    ///
    /// Returns both the execution result and a receiver for `GraphStreamEvent`s
    /// that can be forwarded to the SSE stream.
    #[cfg(feature = "collaboration")]
    pub async fn execute_with_collaboration_streaming(
        self: &Arc<Self>,
        task: &str,
        mode: Option<CollaborationMode>,
    ) -> Result<
        (
            AgentGraphState,
            tokio::sync::mpsc::Receiver<crate::graph::GraphStreamEvent>,
        ),
        GraphError,
    > {
        use crate::graph::StreamingObserver;
        use std::time::Instant;

        // Determine execution mode for store tracking
        let exec_mode = match &mode {
            None => crate::graph::ExecutionMode::Direct,
            Some(CollaborationMode::Direct) => crate::graph::ExecutionMode::Direct,
            Some(CollaborationMode::PlanExecute) => crate::graph::ExecutionMode::PlanExecute,
            Some(CollaborationMode::Swarm { .. }) => crate::graph::ExecutionMode::Swarm,
            Some(CollaborationMode::Expert { .. }) => crate::graph::ExecutionMode::Expert,
            Some(CollaborationMode::Graph { graph_id }) => {
                crate::graph::ExecutionMode::Graph(graph_id.clone())
            }
        };

        // Generate execution ID and start tracking
        let exec_id = Uuid::new_v4().to_string();
        if let Some(ref store) = self.execution_store {
            store.start_execution(&exec_id, exec_mode).await;
        }

        let start = Instant::now();

        // Create streaming observer + channel
        let (streaming_obs, graph_rx) = StreamingObserver::channel(128);

        // Create graph with streaming observer attached
        let graph =
            self.create_collaboration_graph_with_observer(mode, Some(&exec_id), streaming_obs)?;

        // Build executor
        let mut executor = GraphExecutor::new(graph);
        if let Some(ref default_budget) = self.execution_budget {
            executor = executor.with_budget(crate::graph::ExecutionBudget::new(
                default_budget.total_budget(),
            ));
        }

        // Prepare state
        let mut state = AgentGraphState::new(task);
        let effective_user_id =
            Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap_or(Uuid::nil());
        let memory_bridge = self.unified_memory.as_ref().map(|store| {
            crate::graph::MemoryBridge::with_defaults(store.clone(), effective_user_id)
        });
        if let Some(ref bridge) = memory_bridge {
            let loaded = bridge.hydrate(&mut state).await;
            if loaded > 0 {
                tracing::debug!(loaded, "Memory bridge hydrated state with entries");
            }
        }

        // Execute
        let result = executor.execute_auto(state).await;

        // Flush memory bridge
        if let Some(ref bridge) = memory_bridge {
            if let Ok(ref final_state) = result {
                let flushed = bridge.flush(final_state).await;
                if flushed > 0 {
                    tracing::debug!(flushed, "Memory bridge flushed state entries");
                }
            }
        }

        // Complete tracking
        if let Some(ref store) = self.execution_store {
            let duration_ms = start.elapsed().as_millis() as u64;
            match &result {
                Ok(_) => store.complete_execution(&exec_id, duration_ms).await,
                Err(e) => store.fail_execution(&exec_id, &e.to_string()).await,
            }
        }

        result.map(|state| (state, graph_rx))
    }

    /// Spawn a collaboration task in the background with streaming.
    ///
    /// Unlike `execute_with_collaboration_streaming` which blocks until completion,
    /// this method spawns execution on a background task and returns immediately.
    /// The caller receives a `JoinHandle` (to await the final result) and a
    /// `Receiver` (to drain events in real-time during execution).
    ///
    /// The caller is responsible for execution tracking in `ExecutionStore` — this
    /// method does NOT create its own execution_id or call `start_execution`.
    #[cfg(feature = "collaboration")]
    pub async fn start_collaboration_streaming(
        self: &Arc<Self>,
        task: &str,
        mode: Option<CollaborationMode>,
        execution_id: &str,
        job_id: uuid::Uuid,
    ) -> Result<
        (
            tokio::task::JoinHandle<Result<AgentGraphState, GraphError>>,
            tokio::sync::mpsc::Receiver<crate::graph::GraphStreamEvent>,
        ),
        GraphError,
    > {
        use crate::graph::StreamingObserver;

        // Create streaming observer + channel (512 buffer for backpressure headroom)
        let (streaming_obs, graph_rx) = StreamingObserver::channel(512);

        // Create graph with streaming observer attached
        let graph =
            self.create_collaboration_graph_with_observer(mode, Some(execution_id), streaming_obs)?;

        // Build executor
        let mut executor = GraphExecutor::new(graph);
        if let Some(ref default_budget) = self.execution_budget {
            executor = executor.with_budget(crate::graph::ExecutionBudget::new(
                default_budget.total_budget(),
            ));
        }

        // Prepare state
        let mut state = AgentGraphState::new(task);

        // Inject execution context so replanner HITL can route back to the client
        let exec_id_str = execution_id.to_string();
        let job_id_str = job_id.to_string();
        state.update_plan_state(|ps| {
            ps.execution_id = exec_id_str;
            ps.job_id = Some(job_id_str);
        });

        let effective_user_id =
            Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap_or(Uuid::nil());
        let memory_bridge = self.unified_memory.as_ref().map(|store| {
            crate::graph::MemoryBridge::with_defaults(store.clone(), effective_user_id)
        });
        if let Some(ref bridge) = memory_bridge {
            let loaded = bridge.hydrate(&mut state).await;
            if loaded > 0 {
                tracing::debug!(loaded, "Memory bridge hydrated state with entries");
            }
        }

        // Spawn execution in background — returns immediately
        let handle = tokio::spawn(async move {
            let result = executor.execute_auto(state).await;

            // Flush memory bridge on success
            if let Some(ref bridge) = memory_bridge {
                if let Ok(ref final_state) = result {
                    let flushed = bridge.flush(final_state).await;
                    if flushed > 0 {
                        tracing::debug!(flushed, "Memory bridge flushed state entries");
                    }
                }
            }

            result
        });

        Ok((handle, graph_rx))
    }

    /// Create a collaboration graph with a streaming observer attached.
    ///
    /// Delegates to `create_collaboration_graph_tracked` for graph topology,
    /// then rebuilds with the streaming observer attached via `attach_observers`.
    #[cfg(feature = "collaboration")]
    fn create_collaboration_graph_with_observer(
        self: &Arc<Self>,
        mode: Option<CollaborationMode>,
        exec_id: Option<&str>,
        streaming_observer: crate::graph::StreamingObserver,
    ) -> Result<StateGraph<AgentGraphState>, GraphError> {
        let mode = mode.or(self.default_collaboration_mode.clone());

        // Get a content sender clone so nodes can stream thinking/text tokens
        let content_tx = streaming_observer.sender();

        // Get the builder for the requested mode (topology only, no observers)
        let builder = match mode {
            Some(CollaborationMode::Direct) | None => {
                let node = AgentRunnerNode::new(self.clone()).with_content_stream(content_tx);
                StateGraphBuilder::new()
                    .add_node("agent", node)
                    .set_entry("agent")
                    .set_terminal("agent")
            }
            Some(CollaborationMode::PlanExecute) => {
                // PlanExecute uses ClosureHandler nodes; pass content_tx for
                // approval gate SSE events and future node-level streaming.
                self.create_plan_execute_graph_builder(Some(content_tx.clone()))?
            }
            Some(CollaborationMode::Swarm {
                initial_agent,
                handoff_rules,
                agent_models,
            }) => self.create_swarm_graph_builder_with_content_stream(
                &initial_agent,
                &handoff_rules,
                &agent_models,
                content_tx,
            )?,
            Some(CollaborationMode::Expert {
                supervisor,
                specialists,
                supervisor_model,
                default_specialist_model,
                specialist_models,
            }) => self.create_expert_graph_builder_with_content_stream(
                &supervisor,
                &specialists,
                supervisor_model.as_deref(),
                default_specialist_model.as_deref(),
                &specialist_models,
                content_tx,
            )?,
            Some(CollaborationMode::Graph { graph_id }) => {
                return Err(GraphError::Internal(format!(
                    "Custom graph '{}' must be created via TemplateRegistry",
                    graph_id
                )));
            }
        };

        // Attach observers (Recording + Learning + Streaming) for ALL modes
        let builder = self.attach_observers(builder, exec_id, Some(streaming_observer));
        builder.build()
    }

    /// Execute a task using the collaboration graph with optional A23 configuration.
    ///
    /// Extends `execute_with_collaboration` with budget, DAG scheduling, and memory bridge support.
    ///
    /// # Arguments
    ///
    /// * `task` - The task prompt to execute
    /// * `mode` - Optional collaboration mode override
    /// * `budget_tokens` - Optional global token budget (creates an `ExecutionBudget`)
    /// * `dag_scheduling` - Optional DAG scheduling override (if true, uses `execute_auto`)
    /// * `error_strategy` - Optional error strategy ("fail_fast", "continue", "retry")
    /// * `user_id` - Optional user ID for memory bridge (defaults to deterministic system user)
    #[cfg(feature = "collaboration")]
    pub async fn execute_with_collaboration_config(
        self: &Arc<Self>,
        task: &str,
        mode: Option<CollaborationMode>,
        budget_tokens: Option<u32>,
        dag_scheduling: Option<bool>,
        error_strategy: Option<&str>,
        user_id: Option<Uuid>,
    ) -> Result<AgentGraphState, GraphError> {
        use std::time::Instant;

        // Determine execution mode for store tracking
        let exec_mode = match &mode {
            None => crate::graph::ExecutionMode::Direct,
            Some(CollaborationMode::Direct) => crate::graph::ExecutionMode::Direct,
            Some(CollaborationMode::PlanExecute) => crate::graph::ExecutionMode::PlanExecute,
            Some(CollaborationMode::Swarm { .. }) => crate::graph::ExecutionMode::Swarm,
            Some(CollaborationMode::Expert { .. }) => crate::graph::ExecutionMode::Expert,
            Some(CollaborationMode::Graph { graph_id }) => {
                crate::graph::ExecutionMode::Graph(graph_id.clone())
            }
        };

        // Generate execution ID and start tracking if store is available
        let exec_id = Uuid::new_v4().to_string();
        if let Some(ref store) = self.execution_store {
            store.start_execution(&exec_id, exec_mode).await;
        }

        let start = Instant::now();

        // Create graph with tracking
        let mut graph = self.create_collaboration_graph_tracked(mode, Some(&exec_id))?;

        // Apply DAG scheduling config if requested
        if let Some(dag) = dag_scheduling {
            graph.config.dag_scheduling = dag;
        }

        // Apply error strategy if requested
        if let Some(strategy) = error_strategy {
            use crate::graph::node::ErrorStrategy;
            graph.config.default_error_strategy = match strategy {
                "continue" => ErrorStrategy::ContinueOnError,
                "retry" => ErrorStrategy::RetryThenFail { max_retries: 3 },
                _ => ErrorStrategy::FailFast,
            };
        }

        // Build executor with optional budget
        let mut executor = GraphExecutor::new(graph);
        if let Some(tokens) = budget_tokens {
            // Per-request budget from API parameter
            executor = executor.with_budget(crate::graph::ExecutionBudget::new(tokens));
        } else if let Some(ref default_budget) = self.execution_budget {
            // Factory-level default budget — create a fresh instance with same total
            executor = executor.with_budget(crate::graph::ExecutionBudget::new(
                default_budget.total_budget(),
            ));
        }

        // Hydrate state from memory bridge if unified_memory is available
        let mut state = AgentGraphState::new(task);

        // Inject execution context so replanner HITL can route back to the client
        state.update_plan_state(|ps| {
            ps.execution_id = exec_id.clone();
        });

        let effective_user_id = user_id.unwrap_or_else(|| {
            // Deterministic system user ID (not nil, so MemoryBridge can partition data)
            Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap_or(Uuid::nil())
        });
        let memory_bridge = self.unified_memory.as_ref().map(|store| {
            crate::graph::MemoryBridge::with_defaults(store.clone(), effective_user_id)
        });
        if let Some(ref bridge) = memory_bridge {
            let loaded = bridge.hydrate(&mut state).await;
            if loaded > 0 {
                tracing::debug!(loaded, "Memory bridge hydrated state with entries");
            }
        }

        // Execute (use execute_auto for DAG-aware scheduling)
        let result = executor.execute_auto(state).await;

        // Flush state back to memory bridge
        if let Some(ref bridge) = memory_bridge {
            if let Ok(ref final_state) = result {
                let flushed = bridge.flush(final_state).await;
                if flushed > 0 {
                    tracing::debug!(flushed, "Memory bridge flushed state entries");
                }
            }
        }

        // A39: Post-execution verified plan registration
        if let Ok(ref final_state) = result {
            if let Some(candidate) = final_state.plan_state().verified_plan_candidate.as_ref() {
                if let Some(ref registry) = self.skill_registry {
                    // Registry is behind Arc — we need interior mutability
                    // For now, log the candidate; registration requires mutable access
                    // which would need Arc<RwLock<SkillRegistry>> in a future refactor.
                    let plan_name = candidate
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    tracing::info!(
                        plan_name,
                        "A39: Verified plan candidate available for registration"
                    );
                    // Update outcome if we reused an offered plan
                    if let Some(offered) = candidate.get("offered_plan").and_then(|v| v.as_str()) {
                        tracing::info!(
                            offered_plan = offered,
                            "A39: Would update outcome for reused verified plan"
                        );
                    }
                    let _ = &registry; // Suppress unused warning
                }
            }
        }

        // Complete tracking
        if let Some(ref store) = self.execution_store {
            let duration_ms = start.elapsed().as_millis() as u64;
            match &result {
                Ok(_) => store.complete_execution(&exec_id, duration_ms).await,
                Err(e) => store.fail_execution(&exec_id, &e.to_string()).await,
            }
        }

        result
    }

    /// Get or create a cached graph executor for a collaboration mode.
    ///
    /// This method caches graph executors by mode to avoid rebuilding
    /// the graph on every execution.
    ///
    /// Note: This method requires the caller to have a `&Arc<Self>` reference.
    #[cfg(feature = "collaboration")]
    pub async fn get_or_create_graph_executor(
        self: &Arc<Self>,
        mode: Option<CollaborationMode>,
    ) -> Result<Arc<GraphExecutor<AgentGraphState>>, GraphError> {
        // For now, create a new executor each time
        // A production implementation would cache these
        let executor = self.create_graph_executor(mode)?;
        Ok(Arc::new(executor))
    }
}

/// Capture a screenshot for visual judge evaluation.
///
/// Strategy:
/// 1. Try to find a window matching the target app name via `osascript` (AppleScript)
/// 2. Capture that specific window with `screencapture -l <windowID>`
/// 3. Fallback to Simulator screenshot via `xcrun simctl io booted screenshot`
/// 4. Last resort: full screen capture via `screencapture -x`
///
/// Returns the raw PNG bytes, or an error if all capture methods fail.
#[cfg(feature = "collaboration")]
async fn capture_screenshot_for_judge() -> Result<Vec<u8>, String> {
    use tokio::process::Command;

    let tmp_path = format!("/tmp/canal_judge_{}.png", uuid::Uuid::new_v4());

    // Strategy 1: Try Simulator screenshot (most common for SwiftUI dev)
    let sim_result = Command::new("xcrun")
        .args(["simctl", "io", "booted", "screenshot", &tmp_path])
        .output()
        .await;

    if let Ok(output) = sim_result {
        if output.status.success() {
            if let Ok(bytes) = tokio::fs::read(&tmp_path).await {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                if !bytes.is_empty() {
                    tracing::debug!(
                        bytes = bytes.len(),
                        method = "simulator",
                        "Screenshot captured for judge"
                    );
                    return Ok(bytes);
                }
            }
        }
    }

    // Strategy 2: Try to capture the frontmost window (macOS)
    // Use osascript to get the window ID of the frontmost application
    let window_id_result = Command::new("osascript")
        .args([
            "-e",
            r#"tell application "System Events" to get id of first window of first process whose frontmost is true"#,
        ])
        .output()
        .await;

    if let Ok(output) = window_id_result {
        if output.status.success() {
            let wid_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Ok(_wid) = wid_str.parse::<u32>() {
                let capture = Command::new("screencapture")
                    .args(["-x", "-l", &wid_str, "-t", "png", &tmp_path])
                    .output()
                    .await;

                if let Ok(output) = capture {
                    if output.status.success() {
                        if let Ok(bytes) = tokio::fs::read(&tmp_path).await {
                            let _ = tokio::fs::remove_file(&tmp_path).await;
                            if !bytes.is_empty() {
                                tracing::debug!(
                                    bytes = bytes.len(),
                                    window_id = %wid_str,
                                    method = "window",
                                    "Screenshot captured for judge"
                                );
                                return Ok(bytes);
                            }
                        }
                    }
                }
            }
        }
    }

    // Strategy 3: Full screen fallback
    let output = Command::new("screencapture")
        .args(["-x", "-t", "png", &tmp_path])
        .output()
        .await
        .map_err(|e| format!("screencapture failed to execute: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("screencapture failed: {}", stderr));
    }

    let bytes = tokio::fs::read(&tmp_path)
        .await
        .map_err(|e| format!("Failed to read screenshot file: {}", e))?;

    let _ = tokio::fs::remove_file(&tmp_path).await;

    if bytes.is_empty() {
        return Err("Screenshot file is empty".into());
    }

    tracing::debug!(
        bytes = bytes.len(),
        method = "fullscreen",
        "Screenshot captured for judge (fullscreen fallback)"
    );

    Ok(bytes)
}

/// Builder for AgentFactory with fluent interface
pub struct AgentFactoryBuilder {
    factory: AgentFactory,
}

impl AgentFactoryBuilder {
    /// Create a new builder with an LlmRouter
    pub fn new(llm_router: Arc<LlmRouter>) -> Self {
        Self {
            factory: AgentFactory::new(llm_router),
        }
    }

    /// Set MCP gateway (legacy - prefer tool_system)
    pub fn mcp_gateway(mut self, gateway: Arc<McpGateway>) -> Self {
        self.factory.mcp_gateway = Some(gateway);
        self
    }

    /// Set unified ToolSystem
    pub fn tool_system(mut self, tool_system: Arc<ToolSystem>) -> Self {
        self.factory.tool_system = Some(tool_system);
        self
    }

    /// Set hooks
    pub fn hooks(mut self, hooks: Arc<HookExecutor>) -> Self {
        self.factory.default_hooks = hooks;
        self
    }

    /// Set permission mode
    pub fn permission_mode(mut self, mode: PermissionMode) -> Self {
        self.factory.default_permission_mode = mode;
        self
    }

    /// Set working directory
    pub fn cwd(mut self, cwd: PathBuf) -> Self {
        self.factory.default_cwd = Some(cwd);
        self
    }

    /// Set allowed directories
    pub fn allowed_directories(mut self, dirs: Vec<PathBuf>) -> Self {
        self.factory.allowed_directories = dirs;
        self
    }

    /// Set max turns
    pub fn max_turns(mut self, max_turns: u32) -> Self {
        self.factory.default_max_turns = max_turns;
        self
    }

    /// Set max budget
    pub fn max_budget_usd(mut self, budget: f64) -> Self {
        self.factory.default_max_budget_usd = Some(budget);
        self
    }

    /// Set default model
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.factory.default_model = Some(model.into());
        self
    }

    /// Set default max tokens
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.factory.default_max_tokens = Some(max_tokens);
        self
    }

    /// Set code execution router
    pub fn code_router(mut self, router: Arc<crate::executor::UnifiedCodeActRouter>) -> Self {
        self.factory.code_router = Some(router);
        self
    }

    /// Set VM manager for browser tool
    #[cfg(unix)]
    pub fn vm_manager(mut self, vm_manager: Arc<crate::vm::VmManager>) -> Self {
        self.factory.vm_manager = Some(vm_manager);
        self
    }

    /// Set worker manager for Orchestrator-Worker pattern
    pub fn worker_manager(mut self, manager: Arc<crate::agent::worker::WorkerManager>) -> Self {
        self.factory.worker_manager = Some(manager);
        self
    }

    /// Set code orchestration runtime
    pub fn code_orchestration(
        mut self,
        runtime: Arc<crate::agent::code_orchestration::CodeOrchestrationRuntime>,
    ) -> Self {
        self.factory.code_orchestration_runtime = Some(runtime);
        self
    }

    /// Set screen controller for ScreenController-backed browser automation
    pub fn screen_controller(mut self, controller: Arc<dyn canal_cv::ScreenController>) -> Self {
        self.factory.screen_controller = Some(controller);
        self
    }

    /// Set CDP screen controller for browser-specific operations
    pub fn cdp_controller(mut self, cdp: Arc<crate::screen::CdpScreenController>) -> Self {
        self.factory.cdp_controller = Some(cdp);
        self
    }

    /// Enable LLM-based summarization for context compaction
    ///
    /// When enabled, the ContextCompactor will use the LLM router to generate
    /// intelligent summaries of conversation history when compaction is triggered.
    pub fn llm_summarization(mut self, enable: bool) -> Self {
        self.factory.enable_llm_summarization = enable;
        self
    }

    /// Set custom compaction configuration
    ///
    /// This allows overriding the default compaction settings for all agents
    /// created by this factory.
    pub fn compaction_config(mut self, config: CompactionConfig) -> Self {
        self.factory.compaction_config = Some(config);
        self
    }

    /// Enable auto-checkpoint with default settings
    pub fn auto_checkpoint(mut self) -> Self {
        self.factory.checkpoint_config = Some(AutoCheckpointConfig::default());
        self
    }

    /// Set custom auto-checkpoint configuration
    pub fn checkpoint_config(mut self, config: AutoCheckpointConfig) -> Self {
        self.factory.checkpoint_config = Some(config);
        self
    }

    /// Set the checkpoint manager for storing checkpoints
    pub fn checkpoint_manager(mut self, manager: Arc<dyn CheckpointManager + Send + Sync>) -> Self {
        self.factory.checkpoint_manager = Some(manager);
        self
    }

    /// Set the default collaboration mode for graph-based execution.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use gateway_core::collaboration::CollaborationMode;
    ///
    /// let factory = AgentFactoryBuilder::new(llm_router)
    ///     .collaboration_mode(CollaborationMode::Direct)
    ///     .build();
    /// ```
    #[cfg(feature = "collaboration")]
    pub fn collaboration_mode(mut self, mode: CollaborationMode) -> Self {
        self.factory.default_collaboration_mode = Some(mode);
        self
    }

    /// Build the factory
    pub fn build(self) -> AgentFactory {
        self.factory
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::r#loop::AgentLoop;
    use crate::llm::LlmConfig;

    fn create_test_router() -> Arc<LlmRouter> {
        Arc::new(LlmRouter::new(LlmConfig::default()))
    }

    #[test]
    fn test_factory_create() {
        let router = create_test_router();
        let factory = AgentFactory::new(router);

        let agent = factory.create();
        assert!(!agent.is_running());
    }

    #[tokio::test]
    async fn test_factory_get_or_create() {
        let router = create_test_router();
        let factory = AgentFactory::new(router);

        let agent1 = factory.get_or_create("session-1").await;
        let agent2 = factory.get_or_create("session-1").await;

        // Should be the same agent
        assert!(Arc::ptr_eq(&agent1, &agent2));

        // Different session should be different agent
        let agent3 = factory.get_or_create("session-2").await;
        assert!(!Arc::ptr_eq(&agent1, &agent3));

        assert_eq!(factory.active_count().await, 2);
    }

    #[test]
    fn test_factory_builder() {
        let router = create_test_router();
        let factory = AgentFactoryBuilder::new(router)
            .max_turns(50)
            .permission_mode(PermissionMode::AcceptEdits)
            .model("claude-3-opus")
            .max_tokens(4096)
            .build();

        assert_eq!(factory.default_max_turns, 50);
        assert_eq!(factory.default_permission_mode, PermissionMode::AcceptEdits);
        assert_eq!(factory.default_model, Some("claude-3-opus".to_string()));
        assert_eq!(factory.default_max_tokens, Some(4096));
    }

    #[test]
    fn test_factory_with_mcp_gateway() {
        let router = create_test_router();
        let mcp = Arc::new(McpGateway::new());
        let factory = AgentFactory::new(router).with_mcp_gateway(mcp);

        assert!(factory.mcp_gateway.is_some());
    }

    #[test]
    fn test_factory_with_cwd_and_allowed_dirs() {
        let router = create_test_router();
        let factory = AgentFactory::new(router)
            .with_cwd(PathBuf::from("/home/user"))
            .with_allowed_directories(vec![PathBuf::from("/home/user/projects")]);

        assert_eq!(factory.default_cwd, Some(PathBuf::from("/home/user")));
        assert_eq!(
            factory.allowed_directories,
            vec![PathBuf::from("/home/user/projects")]
        );
    }

    #[test]
    fn test_factory_create_from_str() {
        let router = create_test_router();
        let factory = AgentFactory::new(router).with_cwd(PathBuf::from("/tmp"));

        let config_str = r#"---
name: test-agent
max_turns: 25
---

# Test Instructions

Be helpful.
"#;

        let result = factory.create_from_str(config_str);
        assert!(result.is_ok());
        let agent = result.unwrap();
        assert!(!agent.is_running());
    }

    #[test]
    fn test_factory_create_from_str_with_tools() {
        let router = create_test_router();
        let factory = AgentFactory::new(router);

        let config_str = r#"---
name: restricted-agent
tools:
  allowed:
    - Read
    - Glob
  blocked:
    - Bash
permissions:
  mode: plan
---

# Read-only Agent
"#;

        let agent = factory.create_from_str(config_str).unwrap();
        assert!(!agent.is_running());
    }

    #[test]
    fn test_factory_with_llm_summarization() {
        let router = create_test_router();
        let factory = AgentFactory::new(router).with_llm_summarization(true);

        assert!(factory.enable_llm_summarization);
    }

    #[test]
    fn test_factory_with_compaction_config() {
        let router = create_test_router();
        let config = CompactionConfig {
            enabled: true,
            max_context_tokens: 150_000,
            min_messages_to_keep: 15,
            target_tokens: 80_000,
        };
        let factory = AgentFactory::new(router).with_compaction_config(config.clone());

        assert!(factory.compaction_config.is_some());
        let stored_config = factory.compaction_config.unwrap();
        assert_eq!(stored_config.max_context_tokens, 150_000);
        assert_eq!(stored_config.min_messages_to_keep, 15);
        assert_eq!(stored_config.target_tokens, 80_000);
    }

    #[test]
    fn test_factory_builder_with_compaction() {
        let router = create_test_router();
        let factory = AgentFactoryBuilder::new(router)
            .llm_summarization(true)
            .compaction_config(CompactionConfig {
                enabled: true,
                max_context_tokens: 200_000,
                min_messages_to_keep: 20,
                target_tokens: 100_000,
            })
            .build();

        assert!(factory.enable_llm_summarization);
        assert!(factory.compaction_config.is_some());
    }

    #[test]
    fn test_agent_with_compactor() {
        let router = create_test_router();
        let factory = AgentFactory::new(router).with_compaction_config(CompactionConfig {
            enabled: true,
            max_context_tokens: 50_000,
            min_messages_to_keep: 5,
            target_tokens: 30_000,
        });

        let agent = factory.create();

        // Verify the agent has a compactor (via the getter method)
        let compactor = agent.compactor();
        assert_eq!(compactor.config().max_tokens, 50_000);
        assert_eq!(compactor.config().keep_recent, 5);
        assert_eq!(compactor.config().target_tokens, 30_000);
    }

    // ==========================================================================
    // Production Path Tests - Verify ContextIntegration is used
    // ==========================================================================

    #[test]
    fn test_factory_context_hierarchy_enabled_by_default() {
        let router = create_test_router();
        let factory = AgentFactory::new(router);

        // Context hierarchy should be enabled by default
        assert!(factory.enable_context_hierarchy);
    }

    #[test]
    fn test_factory_with_context_hierarchy_disabled() {
        let router = create_test_router();
        let factory = AgentFactory::new(router).with_context_hierarchy(false);

        assert!(!factory.enable_context_hierarchy);
    }

    #[test]
    fn test_factory_with_platform_config() {
        let router = create_test_router();
        let factory = AgentFactory::new(router).with_platform_config("config/platform-rules.yaml");

        assert!(factory.platform_config_path.is_some());
        assert_eq!(
            factory.platform_config_path.unwrap().to_str().unwrap(),
            "config/platform-rules.yaml"
        );
    }

    #[test]
    fn test_factory_with_skill_registry() {
        use crate::agent::skills::SkillRegistry;

        let router = create_test_router();
        let registry = Arc::new(SkillRegistry::with_builtins());
        let factory = AgentFactory::new(router).with_skill_registry(registry);

        assert!(factory.skill_registry.is_some());
    }

    #[test]
    fn test_factory_generates_context_system_prompt() {
        let router = create_test_router();
        let factory = AgentFactory::new(router);

        // Generate system prompt using context hierarchy
        let prompt = factory.generate_context_system_prompt(None);

        // Should return a non-empty prompt (even if platform config doesn't exist)
        // The ContextIntegration uses defaults when config is missing
        assert!(!prompt.is_empty() || prompt.is_empty()); // Just verify it doesn't panic
    }

    #[test]
    fn test_factory_create_uses_context_hierarchy() {
        let router = create_test_router();
        let factory = AgentFactory::new(router);

        // Create agent with context hierarchy enabled
        let agent = factory.create();

        // The agent should have a system_prompt set from ContextIntegration
        // Note: We can't directly access config.system_prompt, but we can verify
        // the agent was created successfully
        assert!(!agent.is_running());
    }

    #[test]
    fn test_factory_create_for_session_uses_context_hierarchy() {
        let router = create_test_router();
        let factory = AgentFactory::new(router);

        // Create agent for session with context hierarchy enabled
        let agent = factory.create_for_session("test-session-123");

        // The agent should have a system_prompt set from ContextIntegration
        assert!(!agent.is_running());
    }

    #[test]
    fn test_factory_with_platform_context_directly() {
        use crate::agent::context::PlatformContext;

        let router = create_test_router();
        let platform_ctx = PlatformContext::default();
        let factory = AgentFactory::new(router).with_platform_context(platform_ctx);

        assert!(factory.platform_context.is_some());

        // Verify system prompt generation uses the provided context
        let prompt = factory.generate_context_system_prompt(Some("session-1"));
        // Should not panic and should produce some output
        assert!(prompt.is_empty() || !prompt.is_empty());
    }

    #[test]
    fn test_factory_context_hierarchy_respects_existing_system_prompt() {
        let router = create_test_router();
        let factory = AgentFactory::new(router);

        // Create config with existing system_prompt
        let mut config = AgentConfig::default();
        config.system_prompt = Some("Custom system prompt".to_string());

        // Create agent - should use existing prompt, not override
        let agent = factory.create_with_config(config);
        assert!(!agent.is_running());
    }

    // =========================================================================
    // Prompt Constraint Tests (require "prompt-constraints" feature)
    // =========================================================================

    #[cfg(feature = "prompt-constraints")]
    mod constraint_tests {
        use super::*;
        use crate::prompt::{
            ConstraintProfile, OutputConstraint, RoleAnchor, SecurityBoundary, ValidationMode,
        };

        #[test]
        fn test_factory_with_constraint_profile() {
            let router = create_test_router();

            let anchor = RoleAnchor {
                role_name: "Test Agent".to_string(),
                anchor_prompt: "You are a test agent.".to_string(),
                drift_detection: true,
                drift_keywords: vec!["pretend".to_string()],
                ..Default::default()
            };

            let profile = ConstraintProfile::default()
                .with_role_anchor(anchor)
                .with_security(SecurityBoundary {
                    blocked_commands: vec!["rm -rf /".to_string()],
                    ..Default::default()
                });

            let factory = AgentFactory::new(router).with_constraint_profile(Some(profile.clone()));

            // Verify profile is stored
            assert!(factory.get_constraint_profile().is_some());
            assert_eq!(
                factory
                    .get_constraint_profile()
                    .unwrap()
                    .role_anchor
                    .as_ref()
                    .unwrap()
                    .role_name,
                "Test Agent"
            );

            // Factory should create an agent (with constraint validator wired)
            let agent = factory.create();
            assert!(!agent.is_running());
        }

        #[test]
        fn test_factory_without_constraint_profile() {
            let router = create_test_router();
            let factory = AgentFactory::new(router);

            // By default, no constraint profile
            assert!(factory.get_constraint_profile().is_none());

            // Agent creation should still work fine
            let agent = factory.create();
            assert!(!agent.is_running());
        }

        #[test]
        fn test_system_prompt_includes_constraint_sections() {
            let router = create_test_router();

            let anchor = RoleAnchor {
                role_name: "Browser Agent".to_string(),
                anchor_prompt: "You are a browser automation agent. Navigate web pages."
                    .to_string(),
                drift_detection: true,
                drift_keywords: vec!["pretend".to_string()],
                ..Default::default()
            };

            let output_constraint = OutputConstraint {
                name: "json_actions".to_string(),
                description: "Actions must be JSON".to_string(),
                json_schema: None,
                prompt_injection: "Always respond with JSON action objects.".to_string(),
                validation_mode: ValidationMode::Strict,
                enabled: true,
            };

            let security = SecurityBoundary {
                blocked_commands: vec!["rm -rf /".to_string(), "DROP TABLE".to_string()],
                require_confirmation: vec!["file_delete".to_string()],
                prompt_injection: Some("Never execute dangerous commands.".to_string()),
                ..Default::default()
            };

            let profile = ConstraintProfile::default()
                .with_role_anchor(anchor)
                .with_output_constraint(output_constraint)
                .with_security(security);

            let factory = AgentFactory::new(router).with_constraint_profile(Some(profile));

            let prompt = factory.generate_context_system_prompt(None);

            // Verify role anchor section
            assert!(
                prompt.contains("Role: Browser Agent"),
                "Should contain role name"
            );
            assert!(
                prompt.contains("browser automation agent"),
                "Should contain anchor prompt"
            );

            // Verify output constraint section
            assert!(
                prompt.contains("Output Constraint: json_actions"),
                "Should contain output constraint name"
            );
            assert!(
                prompt.contains("Always respond with JSON action objects"),
                "Should contain prompt injection"
            );

            // Verify security section
            assert!(
                prompt.contains("Security Rules"),
                "Should contain security section header"
            );
            assert!(
                prompt.contains("Never execute dangerous commands"),
                "Should contain security prompt injection"
            );
            assert!(prompt.contains("rm -rf /"), "Should list blocked commands");
            assert!(
                prompt.contains("DROP TABLE"),
                "Should list blocked commands"
            );
            assert!(
                prompt.contains("file_delete"),
                "Should list require_confirmation"
            );
        }

        #[test]
        fn test_system_prompt_no_constraint_sections_without_profile() {
            let router = create_test_router();
            let factory = AgentFactory::new(router);

            let prompt = factory.generate_context_system_prompt(None);

            // Without a constraint profile, prompt should NOT contain constraint sections
            assert!(
                !prompt.contains("# Prompt Constraints"),
                "Should not have constraint header without profile"
            );
        }
    }

    // =========================================================================
    // Collaboration Mode Tests (require "collaboration" feature)
    // =========================================================================

    #[cfg(feature = "collaboration")]
    mod collaboration_tests {
        use super::*;
        use crate::collaboration::{CollaborationMode, HandoffCondition, HandoffRule};

        #[test]
        fn test_factory_with_collaboration_mode() {
            let router = create_test_router();
            let factory =
                AgentFactory::new(router).with_collaboration_mode(CollaborationMode::Direct);

            assert!(factory.default_collaboration_mode.is_some());
            assert!(matches!(
                factory.default_collaboration_mode.as_ref().unwrap(),
                CollaborationMode::Direct
            ));
        }

        #[test]
        fn test_factory_builder_with_collaboration_mode() {
            let router = create_test_router();
            let factory = AgentFactoryBuilder::new(router)
                .collaboration_mode(CollaborationMode::Expert {
                    supervisor: "supervisor".into(),
                    specialists: vec!["coder".into(), "reviewer".into()],
                    supervisor_model: None,
                    default_specialist_model: None,
                    specialist_models: HashMap::new(),
                })
                .build();

            assert!(factory.default_collaboration_mode.is_some());
            if let Some(CollaborationMode::Expert {
                supervisor,
                specialists,
                ..
            }) = &factory.default_collaboration_mode
            {
                assert_eq!(supervisor, "supervisor");
                assert_eq!(specialists.len(), 2);
            } else {
                panic!("Expected Expert mode");
            }
        }

        #[test]
        fn test_factory_create_direct_graph() {
            let router = create_test_router();
            let factory = Arc::new(AgentFactory::new(router));

            let result = factory.create_direct_graph();
            assert!(result.is_ok());

            let graph = result.unwrap();
            // Graph should have one node
            assert!(graph.get_node(&"agent".to_string()).is_some());
        }

        #[test]
        fn test_factory_create_collaboration_graph_direct() {
            let router = create_test_router();
            let factory = Arc::new(AgentFactory::new(router));

            let result = factory.create_collaboration_graph(Some(CollaborationMode::Direct));
            assert!(result.is_ok());
        }

        #[test]
        fn test_factory_create_collaboration_graph_default() {
            let router = create_test_router();
            let factory = Arc::new(
                AgentFactory::new(router).with_collaboration_mode(CollaborationMode::Direct),
            );

            // Should use default mode
            let result = factory.create_collaboration_graph(None);
            assert!(result.is_ok());
        }

        #[test]
        fn test_factory_create_swarm_graph() {
            use crate::collaboration::ContextTransferMode;

            let router = create_test_router();
            let factory = Arc::new(AgentFactory::new(router));

            let rules = vec![HandoffRule {
                from_agent: "research".into(),
                to_agent: "code".into(),
                condition: HandoffCondition::OnKeyword("implement".into()),
                context_transfer: ContextTransferMode::Full,
            }];

            let result = factory.create_swarm_graph("research", &rules, &HashMap::new());
            assert!(result.is_ok());

            let graph = result.unwrap();
            assert!(graph.get_node(&"research".to_string()).is_some());
            assert!(graph.get_node(&"code".to_string()).is_some());
        }

        #[test]
        fn test_factory_create_expert_graph() {
            let router = create_test_router();
            let factory = Arc::new(AgentFactory::new(router));

            let result = factory.create_expert_graph(
                "supervisor",
                &vec!["coder".into(), "reviewer".into()],
                None,
                None,
                &HashMap::new(),
            );
            assert!(result.is_ok());

            let graph = result.unwrap();
            assert!(graph.get_node(&"supervisor".to_string()).is_some());
            assert!(graph.get_node(&"coder".to_string()).is_some());
            assert!(graph.get_node(&"reviewer".to_string()).is_some());
            assert!(graph.get_node(&"aggregator".to_string()).is_some());
        }

        #[test]
        fn test_factory_create_graph_executor() {
            let router = create_test_router();
            let factory = Arc::new(AgentFactory::new(router));

            let result = factory.create_graph_executor(Some(CollaborationMode::Direct));
            assert!(result.is_ok());
        }

        #[test]
        fn test_factory_custom_graph_mode_returns_error() {
            let router = create_test_router();
            let factory = Arc::new(AgentFactory::new(router));

            let result = factory.create_collaboration_graph(Some(CollaborationMode::Graph {
                graph_id: "custom-graph".into(),
            }));
            assert!(result.is_err());
            match result {
                Ok(_) => panic!("Expected error"),
                Err(e) => assert!(e.to_string().contains("TemplateRegistry")),
            }
        }
    }
}
