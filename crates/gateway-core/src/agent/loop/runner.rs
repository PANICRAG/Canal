//! Agent Runner - Core execution logic

use super::{
    config::{AgentConfig, CompactionConfig},
    state::AgentState,
    AgentError, AgentLoop,
};
use crate::agent::automation::{
    AutomationRequest, AutomationResult, BrowserAutomationOrchestrator,
    OrchestratorConfig as AutomationConfig, RouteAnalysis,
};
use crate::agent::hooks::HookExecutor;
use crate::agent::memory::{ToolCallRecord, ToolState, WorkingMemory};
use crate::agent::session::{
    AutoCheckpointConfig, AutoCheckpointTrigger, Checkpoint, CheckpointManager, CheckpointTrigger,
    CompactTrigger, ContextCompactor, ContextState,
};
use crate::agent::skills::SkillRegistry;
use crate::agent::tools::ToolContext;
use crate::agent::types::{
    AgentMessage, AssistantMessage, ContentBlock, HookContext, HookEvent, HookResult,
    MessageContent, PermissionBehavior, PermissionContext, PermissionMode, PermissionRequest,
    PermissionResult, PermissionRule, ResultMessage, ResultSubtype, SystemMessage, Usage,
    UserMessage,
};
use async_trait::async_trait;
use chrono::Local;
use futures::future::join_all;
use futures::Stream;
use std::path::Path;
use std::pin::Pin;
use std::process::Command;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// Default usage for sync access
static DEFAULT_USAGE: Usage = Usage {
    input_tokens: 0,
    output_tokens: 0,
    cache_creation_input_tokens: 0,
    cache_read_input_tokens: 0,
};

/// Agent runner implements the core agentic loop
pub struct AgentRunner {
    /// Configuration
    config: AgentConfig,
    /// Runtime state
    state: Arc<AgentState>,
    /// Hook executor
    hooks: Arc<HookExecutor>,
    /// LLM client (placeholder - will be connected to actual LLM)
    llm: Option<Arc<dyn LlmClient>>,
    /// Tool executor (placeholder)
    tools: Option<Arc<dyn ToolExecutor>>,
    /// Permission context for tool execution checks
    permission_context: Option<PermissionContext>,
    /// Context compactor for managing conversation length.
    /// Wrapped in `Arc` so the spawned agent-loop task can share it
    /// without borrowing `&mut self` (A41 non-blocking streaming fix).
    compactor: Arc<ContextCompactor>,
    /// Skills registry for managing available skills
    skills: Arc<SkillRegistry>,
    /// Auto-checkpoint configuration
    checkpoint_config: Option<AutoCheckpointConfig>,
    /// Checkpoint manager for saving/loading checkpoints
    checkpoint_manager: Option<Arc<dyn CheckpointManager + Send + Sync>>,
    /// Auto-checkpoint trigger for tracking when to create checkpoints.
    /// Wrapped in `Arc` so the spawned agent-loop task can share it
    /// without borrowing `&mut self` (A41 non-blocking streaming fix).
    checkpoint_trigger: Arc<Mutex<Option<AutoCheckpointTrigger>>>,
    /// Working memory for tracking task state and passing data between tool calls
    /// This is bound to the session lifecycle and persisted with the session
    working_memory: Arc<RwLock<WorkingMemory>>,
    /// Browser automation orchestrator (five-layer architecture)
    /// When enabled, automatically routes large data browser tasks through
    /// explore → generate → execute pipeline for massive token savings
    automation_orchestrator: Option<Arc<BrowserAutomationOrchestrator>>,
    /// Configuration for automation detection
    automation_config: AutomationConfig,
    /// Constraint validator for pre-flight and post-flight validation (optional).
    /// When set, validates user input before sending to LLM and LLM output before returning.
    /// Wrapped in `Arc` so the spawned agent-loop task can share it (A41 fix).
    #[cfg(feature = "prompt-constraints")]
    constraint_validator: Option<Arc<crate::prompt::ConstraintValidator>>,
    /// Agent observer for conversation tracing and monitoring
    /// When set, receives callbacks at key execution points
    #[cfg(feature = "context-engineering")]
    observer: Option<Arc<dyn crate::agent::context::AgentObserver>>,
    /// RTE delegation context for native client tool execution (A28).
    /// When set, tool calls are checked against client capabilities
    /// and eligible tools are delegated to the native client via SSE.
    rte_delegation: Option<crate::rte::RteDelegationContext>,
}

/// LLM client trait (placeholder for actual implementation)
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn generate(
        &self,
        messages: Vec<AgentMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, AgentError>;
}

/// LLM response
#[derive(Clone)]
pub struct LlmResponse {
    pub content: Vec<ContentBlock>,
    pub model: String,
    pub usage: Usage,
    pub stop_reason: StopReason,
}

/// Stop reason
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
}

/// Tool executor trait (placeholder)
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(
        &self,
        tool_name: &str,
        tool_input: serde_json::Value,
        context: &ToolContext,
    ) -> Result<serde_json::Value, AgentError>;

    fn get_tool_schemas(&self) -> Vec<serde_json::Value>;

    /// Get tool schemas filtered by task context.
    ///
    /// This method filters tools based on the current task to reduce token consumption:
    /// - Core tools (Read, Write, Edit, Bash, Glob, Grep) are always included
    /// - Browser tools only included for browser-related tasks
    /// - Orchestrate tool only when workers are enabled
    ///
    /// Default implementation returns all tools (no filtering).
    fn get_filtered_tool_schemas(&self, context: &ToolFilterContext) -> Vec<serde_json::Value> {
        let _ = context; // Suppress unused warning in default impl
        self.get_tool_schemas()
    }
}

/// Re-export ToolFilterContext for use with ToolExecutor trait
pub use crate::agent::tools::ToolFilterContext;

impl AgentRunner {
    /// Create a new agent runner
    pub fn new(config: AgentConfig) -> Self {
        let session_id = uuid::Uuid::new_v4().to_string();
        let compactor = Arc::new(Self::create_compactor(&config.compaction));
        let skills = Arc::new(SkillRegistry::with_builtins());
        Self {
            state: Arc::new(AgentState::with_permission_mode(
                &session_id,
                config.permission_mode,
            )),
            config,
            hooks: Arc::new(HookExecutor::new()),
            llm: None,
            tools: None,
            permission_context: None,
            compactor,
            skills,
            checkpoint_config: None,
            checkpoint_manager: None,
            checkpoint_trigger: Arc::new(Mutex::new(None)),
            working_memory: Arc::new(RwLock::new(WorkingMemory::new())),
            automation_orchestrator: None,
            automation_config: AutomationConfig::default(),
            #[cfg(feature = "prompt-constraints")]
            constraint_validator: None,
            #[cfg(feature = "context-engineering")]
            observer: None,
            rte_delegation: None,
        }
    }

    /// Create with a specific session ID
    pub fn with_session_id(config: AgentConfig, session_id: impl Into<String>) -> Self {
        let session_id = session_id.into();
        let compactor = Arc::new(Self::create_compactor(&config.compaction));
        let skills = Arc::new(SkillRegistry::with_builtins());
        Self {
            state: Arc::new(AgentState::with_permission_mode(
                &session_id,
                config.permission_mode,
            )),
            config,
            hooks: Arc::new(HookExecutor::new()),
            llm: None,
            tools: None,
            permission_context: None,
            compactor,
            skills,
            checkpoint_config: None,
            checkpoint_manager: None,
            checkpoint_trigger: Arc::new(Mutex::new(None)),
            working_memory: Arc::new(RwLock::new(WorkingMemory::new())),
            automation_orchestrator: None,
            automation_config: AutomationConfig::default(),
            #[cfg(feature = "prompt-constraints")]
            constraint_validator: None,
            #[cfg(feature = "context-engineering")]
            observer: None,
            rte_delegation: None,
        }
    }

    /// Create a compactor from configuration
    fn create_compactor(config: &CompactionConfig) -> ContextCompactor {
        ContextCompactor::new()
            .max_tokens(config.max_context_tokens)
            .target_tokens(config.target_tokens)
            .keep_recent(config.min_messages_to_keep)
    }

    /// Set LLM client
    pub fn with_llm(mut self, llm: Arc<dyn LlmClient>) -> Self {
        self.llm = Some(llm);
        self
    }

    /// Set tool executor
    pub fn with_tools(mut self, tools: Arc<dyn ToolExecutor>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Set hook executor
    pub fn with_hooks(mut self, hooks: Arc<HookExecutor>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Set RTE delegation context for native client tool execution (A28).
    ///
    /// When set, eligible tool calls are delegated to the native client
    /// instead of executing on the server.
    pub fn with_rte_delegation(mut self, ctx: crate::rte::RteDelegationContext) -> Self {
        self.rte_delegation = Some(ctx);
        self
    }

    /// Set skills registry
    pub fn with_skills(mut self, skills: Arc<SkillRegistry>) -> Self {
        self.skills = skills;
        self
    }

    /// Set working memory with an existing instance
    ///
    /// This allows restoring working memory from a persisted session
    /// or sharing working memory between related agent instances.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use gateway_core::agent::memory::WorkingMemory;
    ///
    /// // Restore from persisted state
    /// let working_memory = Arc::new(RwLock::new(restored_memory));
    /// let runner = AgentRunner::new(config)
    ///     .with_working_memory(working_memory);
    /// ```
    pub fn with_working_memory(mut self, working_memory: Arc<RwLock<WorkingMemory>>) -> Self {
        self.working_memory = working_memory;
        self
    }

    /// Set browser automation orchestrator (five-layer architecture)
    ///
    /// When enabled, the agent will analyze tasks and automatically route
    /// large data browser operations through the five-layer pipeline:
    /// 1. Intent Router - Analyzes task and selects optimal path
    /// 2. CV Explorer - Screenshots → PageSchema (fixed ~3000-5000 tokens)
    /// 3. Code Generator - Schema → Playwright scripts (fixed ~500-1000 tokens)
    /// 4. Script Executor - Runs scripts locally (0 tokens)
    /// 5. Asset Store - Caches scripts for reuse
    ///
    /// This provides massive token savings for large data operations:
    /// - Pure CV: ~4,100,000 tokens for 1000 rows
    /// - Automation: ~6,000 tokens for 1000 rows (99.85% savings)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let orchestrator = BrowserAutomationOrchestrator::builder()
    ///     .with_browser_router(browser_router)
    ///     .with_llm_router(llm_router)
    ///     .build()?;
    ///
    /// let runner = AgentRunner::new(config)
    ///     .with_automation_orchestrator(Arc::new(orchestrator));
    /// ```
    pub fn with_automation_orchestrator(
        mut self,
        orchestrator: Arc<BrowserAutomationOrchestrator>,
    ) -> Self {
        self.automation_orchestrator = Some(orchestrator);
        self
    }

    /// Set automation configuration
    ///
    /// Controls when the automation orchestrator is triggered.
    pub fn with_automation_config(mut self, config: AutomationConfig) -> Self {
        self.automation_config = config;
        self
    }

    /// Check if automation orchestrator is available
    pub fn has_automation(&self) -> bool {
        self.automation_orchestrator.is_some()
    }

    /// Get automation orchestrator reference
    pub fn automation_orchestrator(&self) -> Option<&Arc<BrowserAutomationOrchestrator>> {
        self.automation_orchestrator.as_ref()
    }

    /// Analyze a task to determine if it should use the automation pipeline
    ///
    /// Returns a RouteAnalysis with the recommended path and estimated token costs.
    /// This is useful for showing the user what path will be taken before execution.
    pub async fn analyze_for_automation(
        &self,
        task: &str,
        data_count: Option<usize>,
    ) -> Option<RouteAnalysis> {
        if let Some(orchestrator) = &self.automation_orchestrator {
            orchestrator.analyze(task, data_count).await.ok()
        } else {
            None
        }
    }

    /// Execute a task through the automation orchestrator
    ///
    /// This bypasses the normal LLM loop and directly executes through the
    /// five-layer pipeline. Only use this when you know the task is suitable
    /// for automation (large data browser operations).
    ///
    /// Returns the automation result with execution stats and token savings.
    pub async fn execute_automation(
        &self,
        request: AutomationRequest,
    ) -> Result<AutomationResult, AgentError> {
        let orchestrator = self.automation_orchestrator.as_ref().ok_or_else(|| {
            AgentError::ConfigError("Automation orchestrator not configured".to_string())
        })?;

        orchestrator
            .execute(request)
            .await
            .map_err(|e| AgentError::ToolError(format!("Automation failed: {}", e)))
    }

    /// Get the working memory (for external access and persistence)
    ///
    /// Use this to access working memory for:
    /// - Persisting session state
    /// - Inspecting current task status
    /// - Reading/writing variables between tool calls
    pub fn working_memory(&self) -> &Arc<RwLock<WorkingMemory>> {
        &self.working_memory
    }

    /// Store a variable in working memory for passing data between tools
    ///
    /// Variables are accessible by any subsequent tool call in the session.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // In a tool, store a result for later use
    /// runner.set_variable("file_list", serde_json::json!(["a.txt", "b.txt"])).await;
    ///
    /// // In another tool, retrieve it
    /// if let Some(files) = runner.get_variable("file_list").await {
    ///     // use files
    /// }
    /// ```
    pub async fn set_variable(&self, name: impl Into<String>, value: serde_json::Value) {
        self.working_memory.write().await.set_variable(name, value);
    }

    /// Get a variable from working memory
    ///
    /// Returns None if the variable does not exist.
    pub async fn get_variable(&self, name: &str) -> Option<serde_json::Value> {
        self.working_memory.read().await.get_variable(name).cloned()
    }

    /// Remove a variable from working memory
    pub async fn remove_variable(&self, name: &str) -> Option<serde_json::Value> {
        self.working_memory.write().await.variables.remove(name)
    }

    /// Get all variables from working memory
    pub async fn get_all_variables(&self) -> std::collections::HashMap<String, serde_json::Value> {
        self.working_memory.read().await.variables.clone()
    }

    /// Start a new task in working memory
    ///
    /// Returns the task ID which can be used to track progress and record tool calls.
    pub async fn start_task(&self, description: impl Into<String>) -> String {
        self.working_memory.write().await.start_task(description)
    }

    /// Complete a task in working memory
    pub async fn complete_task(&self, task_id: &str, result: serde_json::Value) {
        self.working_memory
            .write()
            .await
            .complete_task(task_id, crate::agent::memory::TaskResult::Success(result));
    }

    /// Fail a task in working memory
    pub async fn fail_task(&self, task_id: &str, error: impl Into<String>) {
        self.working_memory.write().await.fail_task(task_id, error);
    }

    /// Get working memory status summary for context injection
    ///
    /// This generates an XML-formatted summary that can be injected into
    /// the system prompt or context to inform the LLM about current task state.
    pub async fn working_memory_summary(&self) -> String {
        self.working_memory.read().await.generate_status_summary()
    }

    /// Clear working memory for a new task (keeps checkpoints)
    pub async fn clear_working_memory(&self) {
        self.working_memory.write().await.clear();
    }

    /// Set a custom context compactor
    ///
    /// This allows configuring a compactor with LLM-based summarization
    /// or custom settings. If not set, a default compactor is created
    /// from the AgentConfig's compaction settings.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use gateway_core::agent::session::ContextCompactor;
    ///
    /// let compactor = ContextCompactor::builder()
    ///     .max_tokens(100_000)
    ///     .threshold_ratio(0.8)
    ///     .keep_recent(10)
    ///     .with_llm_router(llm_router)
    ///     .build();
    ///
    /// let runner = AgentRunner::new(config)
    ///     .with_compactor(compactor);
    /// ```
    pub fn with_compactor(mut self, compactor: ContextCompactor) -> Self {
        self.compactor = Arc::new(compactor);
        self
    }

    /// Set the constraint validator for pre-flight and post-flight validation.
    ///
    /// When set, the agent will:
    /// - Pre-flight: validate user input for blocked commands and role drift
    /// - Post-flight: validate LLM output for JSON format and length
    #[cfg(feature = "prompt-constraints")]
    pub fn with_constraint_validator(
        mut self,
        validator: Option<crate::prompt::ConstraintValidator>,
    ) -> Self {
        self.constraint_validator = validator.map(Arc::new);
        self
    }

    /// Set the agent observer for conversation tracing and monitoring.
    ///
    /// The observer receives callbacks at key execution points:
    /// - `on_prompt_constructed`: after system prompt assembly
    /// - `on_llm_request`/`on_llm_response`: before/after LLM calls
    /// - `on_preflight_check`/`on_postflight_check`: constraint validation results
    /// - `on_tool_call`: after each tool execution
    /// - `on_turn_complete`: at end of each agent loop iteration
    #[cfg(feature = "context-engineering")]
    pub fn with_observer(
        mut self,
        observer: Arc<dyn crate::agent::context::AgentObserver>,
    ) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Get the current context compactor
    ///
    /// This can be used to check context statistics or manually trigger compaction.
    pub fn compactor(&self) -> &Arc<ContextCompactor> {
        &self.compactor
    }

    /// Configure auto-checkpoint with default settings
    ///
    /// This enables automatic checkpointing with sensible defaults:
    /// - Checkpoint before dangerous operations (Bash, Write, Edit)
    /// - Periodic checkpoints every 10 turns
    /// - Checkpoint before context compaction
    ///
    /// Requires a CheckpointManager to be set via `with_checkpoint_manager()`.
    pub fn with_auto_checkpoint(mut self) -> Self {
        let config = AutoCheckpointConfig::default();
        let trigger = AutoCheckpointTrigger::new(config.clone());
        self.checkpoint_config = Some(config);
        self.checkpoint_trigger = Arc::new(Mutex::new(Some(trigger)));
        self
    }

    /// Configure auto-checkpoint with custom settings
    pub fn with_auto_checkpoint_config(mut self, config: AutoCheckpointConfig) -> Self {
        let trigger = AutoCheckpointTrigger::new(config.clone());
        self.checkpoint_config = Some(config);
        self.checkpoint_trigger = Arc::new(Mutex::new(Some(trigger)));
        self
    }

    /// Set the checkpoint manager for storing checkpoints
    pub fn with_checkpoint_manager(
        mut self,
        manager: Arc<dyn CheckpointManager + Send + Sync>,
    ) -> Self {
        self.checkpoint_manager = Some(manager);
        self
    }

    /// Get the current checkpoint configuration
    pub fn checkpoint_config(&self) -> Option<&AutoCheckpointConfig> {
        self.checkpoint_config.as_ref()
    }

    /// Get the checkpoint manager
    pub fn checkpoint_manager(&self) -> Option<&Arc<dyn CheckpointManager + Send + Sync>> {
        self.checkpoint_manager.as_ref()
    }

    /// Create a checkpoint manually
    pub async fn create_checkpoint(
        &self,
        trigger: CheckpointTrigger,
        label: Option<&str>,
    ) -> Option<String> {
        let manager = self.checkpoint_manager.as_ref()?;
        let messages = self.state.messages().await;
        let usage = self.state.usage().await;
        let context_state = ContextState {
            cwd: self
                .config
                .cwd
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            env: std::collections::HashMap::new(),
            estimated_tokens: self.compactor.estimate_tokens(&messages),
            turn_count: self.state.turn(),
            usage,
            total_cost_usd: self.state.total_cost_usd().await,
            custom: std::collections::HashMap::new(),
        };
        let mut checkpoint =
            Checkpoint::new(&self.state.session_id, messages, context_state).with_trigger(trigger);
        if let Some(label) = label {
            checkpoint = checkpoint.with_label(label);
        }
        match manager.save(&checkpoint).await {
            Ok(id) => {
                tracing::info!(checkpoint_id = %id, session_id = %self.state.session_id, trigger = ?trigger, "Checkpoint created");
                Some(id)
            }
            Err(e) => {
                tracing::error!(session_id = %self.state.session_id, error = %e, "Failed to create checkpoint");
                None
            }
        }
    }

    /// Load additional skills from a directory (e.g., .claude/commands/)
    pub fn load_skills_from_dir(&mut self, dir: &Path) -> Result<usize, AgentError> {
        if !dir.exists() {
            return Ok(0);
        }

        let mut count = 0;

        // Get a mutable reference to the registry
        if let Some(registry) = Arc::get_mut(&mut self.skills) {
            // Load skills from .md files in the directory
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map(|e| e == "md").unwrap_or(false) {
                        if let Ok(skill) = crate::agent::skills::SkillParser::parse_file(&path) {
                            let _ = registry.register(skill);
                            count += 1;
                            tracing::debug!(
                                path = %path.display(),
                                "Loaded skill from file"
                            );
                        }
                    }
                }
            }
        }

        Ok(count)
    }

    /// Get the skills registry
    pub fn skills(&self) -> &Arc<SkillRegistry> {
        &self.skills
    }

    /// Set permission context for tool execution checks
    pub fn with_permission_context(mut self, ctx: PermissionContext) -> Self {
        self.permission_context = Some(ctx);
        self
    }

    /// Check if a tool is allowed to execute based on permission context
    ///
    /// Returns the permission result which indicates whether the tool should:
    /// - `Allow`: Proceed with execution (possibly with modified input)
    /// - `Deny`: Return error result to LLM
    /// - `Ask`: Creates a permission request for user approval
    pub fn check_tool_permission(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> PermissionResult {
        match &self.permission_context {
            Some(ctx) => ctx.check_tool(tool_name, input),
            None => {
                // No permission context configured - allow by default
                PermissionResult::allow()
            }
        }
    }

    /// Get the agent state (for external access to pending permissions)
    pub fn state(&self) -> &Arc<AgentState> {
        &self.state
    }

    /// Check if the agent is waiting for permission responses
    pub fn is_waiting_for_permission(&self) -> bool {
        self.state.is_waiting_for_permission()
    }

    /// Process a permission response from the user
    ///
    /// This updates the pending permission state and potentially adds permission rules
    /// based on the user's selection (e.g., "always allow").
    pub async fn process_permission_response(
        &mut self,
        response: crate::agent::types::PermissionResponse,
    ) -> Result<(), AgentError> {
        let request_id = response.request_id.clone();

        // Get the pending permission to extract tool info
        let pending = self
            .state
            .get_pending_permission(&request_id)
            .await
            .ok_or_else(|| {
                AgentError::SessionError(format!("Permission request {} not found", request_id))
            })?;

        // Submit the response to update state
        self.state
            .submit_permission_response(response.clone())
            .await
            .map_err(|e| AgentError::SessionError(e))?;

        // If "always allow" or "always deny" was selected, update permission rules
        if response.is_always_allow() {
            self.add_permission_rule(&pending.request.tool_name, PermissionBehavior::Allow);
            tracing::info!(
                tool_name = %pending.request.tool_name,
                session_id = %self.state.session_id,
                "Added 'always allow' rule for tool"
            );
        } else if response.is_always_deny() {
            self.add_permission_rule(&pending.request.tool_name, PermissionBehavior::Deny);
            tracing::info!(
                tool_name = %pending.request.tool_name,
                session_id = %self.state.session_id,
                "Added 'always deny' rule for tool"
            );
        }

        Ok(())
    }

    /// Add a permission rule for a tool
    fn add_permission_rule(&mut self, tool_name: &str, behavior: PermissionBehavior) {
        let rule = PermissionRule::tool(tool_name);

        if let Some(ref mut ctx) = self.permission_context {
            ctx.rules.push((rule, behavior));
        } else {
            // Create a new permission context if none exists
            let mut ctx = PermissionContext {
                mode: self.config.permission_mode,
                session_id: Some(self.state.session_id.clone()),
                ..Default::default()
            };
            ctx.rules.push((rule, behavior));
            self.permission_context = Some(ctx);
        }
    }

    /// Get all pending permission requests
    pub async fn get_pending_permissions(&self) -> Vec<crate::agent::types::PendingPermission> {
        self.state.get_all_pending_permissions().await
    }

    /// Cancel all pending permission requests
    pub async fn cancel_pending_permissions(&self) {
        self.state.cancel_pending_permissions().await;
    }

    /// Load CLAUDE.md from project root if it exists
    ///
    /// Searches for CLAUDE.md in the current directory and parent directories,
    /// also checking .claude/CLAUDE.md at each level. Returns the content if found,
    /// or None if the file doesn't exist (silently ignored per Claude Code behavior).
    fn load_claude_md(&self) -> Option<String> {
        let cwd = self
            .config
            .cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())?;

        // Check for CLAUDE.md in current directory and parent directories
        let mut current = cwd.as_path();
        loop {
            // Check for CLAUDE.md in current directory
            let claude_md_path = current.join("CLAUDE.md");
            if claude_md_path.exists() {
                tracing::debug!(
                    path = %claude_md_path.display(),
                    "Found CLAUDE.md"
                );
                return std::fs::read_to_string(&claude_md_path).ok();
            }

            // Also check for .claude/CLAUDE.md
            let alt_path = current.join(".claude").join("CLAUDE.md");
            if alt_path.exists() {
                tracing::debug!(
                    path = %alt_path.display(),
                    "Found .claude/CLAUDE.md"
                );
                return std::fs::read_to_string(&alt_path).ok();
            }

            // Move to parent directory
            match current.parent() {
                Some(parent) => current = parent,
                None => break,
            }
        }

        tracing::debug!("No CLAUDE.md found in project hierarchy");
        None
    }

    /// Build environment context similar to Claude Code
    /// Returns an XML-formatted string with environment information
    fn build_environment_context(&self) -> String {
        // Get current working directory
        let cwd = self
            .config
            .cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Check if it's a git repository
        let cwd_path = Path::new(&cwd);
        let is_git_repo = cwd_path.join(".git").exists() || {
            // R1-H15: Use Stdio::null() to minimize blocking; this is sync but fast
            Command::new("git")
                .args(["rev-parse", "--git-dir"])
                .current_dir(&cwd)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };

        // Get platform
        let platform = std::env::consts::OS;

        // Get OS version
        let os_version = Self::get_os_version();

        // Get today's date
        let today = Local::now().format("%Y-%m-%d").to_string();

        // Build the basic environment context
        let mut env_context = format!(
            r#"<env>
Working directory: {}
Is directory a git repo: {}
Platform: {}
OS Version: {}
Today's date: {}
</env>"#,
            cwd,
            if is_git_repo { "Yes" } else { "No" },
            platform,
            os_version,
            today
        );

        // If it's a git repo, add git-specific information
        if is_git_repo {
            let git_info = Self::get_git_info(&cwd);
            if !git_info.is_empty() {
                env_context = format!(
                    r#"<env>
Working directory: {}
Is directory a git repo: Yes
Platform: {}
OS Version: {}
Today's date: {}
</env>

{}"#,
                    cwd, platform, os_version, today, git_info
                );
            }
        }

        env_context
    }

    /// Get OS version string
    fn get_os_version() -> String {
        #[cfg(target_os = "macos")]
        {
            Command::new("uname")
                .arg("-rs")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "Darwin".to_string())
        }

        #[cfg(target_os = "linux")]
        {
            Command::new("uname")
                .arg("-rs")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "Linux".to_string())
        }

        #[cfg(target_os = "windows")]
        {
            Command::new("cmd")
                .args(["/C", "ver"])
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "Windows".to_string())
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            std::env::consts::OS.to_string()
        }
    }

    /// Get git repository information
    fn get_git_info(cwd: &str) -> String {
        let mut info_parts = Vec::new();

        // Get current branch
        if let Some(branch) = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
        {
            info_parts.push(format!("Current branch: {}", branch));
        }

        // Determine main branch (check for main, then master)
        let main_branch = Command::new("git")
            .args(["rev-parse", "--verify", "refs/heads/main"])
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|_| "main".to_string())
            .or_else(|| {
                Command::new("git")
                    .args(["rev-parse", "--verify", "refs/heads/master"])
                    .current_dir(cwd)
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|_| "master".to_string())
            });

        if let Some(main) = main_branch {
            info_parts.push(format!(
                "Main branch (you will usually use this for PRs): {}",
                main
            ));
        }

        // Get short git status
        if let Some(status) = Command::new("git")
            .args(["status", "--short", "--branch"])
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
        {
            if !status.is_empty() {
                // Limit the status output to avoid too much noise
                let status_lines: Vec<&str> = status.lines().take(20).collect();
                let truncated = if status.lines().count() > 20 {
                    format!("{}\n... (truncated)", status_lines.join("\n"))
                } else {
                    status_lines.join("\n")
                };
                info_parts.push(format!("Status:\n{}", truncated));
            }
        }

        if info_parts.is_empty() {
            return String::new();
        }

        format!(
            "gitStatus: This is the git status at the start of the conversation.\n{}",
            info_parts.join("\n\n")
        )
    }

    /// Default system prompt for agent behavior
    fn default_system_prompt(&self) -> String {
        let base_prompt = r#"You are Canal Agent, an intelligent AI assistant that uses tools to accomplish tasks for users. You are NOT a coding tutor - you are an autonomous agent that takes action to get things done.

## Your Role

You help users accomplish their goals by:
- Using browser automation to interact with web services (Gmail, Twitter, LinkedIn, etc.)
- Writing and executing Python/Bash code to perform actions
- Reading and modifying files
- Running commands and scripts
- Automating repetitive tasks

Think of yourself as a capable assistant who uses the right tool for each task.

## Core Principles

1. **Action-Oriented**: When a user asks for something, DO IT. Don't explain how to do it - actually do it.

2. **Browser-First for Web Tasks**: For any task involving web services (email, social media, web apps):
   - Use browser automation tools (browser_navigate, browser_click, browser_fill, etc.)
   - The user's browser is connected - you can control it directly
   - This works for Gmail, Twitter, LinkedIn, banking sites, and any web application
   - The user is likely already logged in to their accounts

3. **Code as Action**: Use Python or Bash code for:
   - Data processing and analysis
   - File operations
   - API integrations (when APIs are available)
   - System automation

4. **Autonomous Execution**:
   - Break complex tasks into steps
   - Execute each step using the appropriate tool
   - Check results and adjust if needed
   - Continue until the task is complete

5. **Show Results, Not Process**:
   - Users care about outcomes, not implementation details
   - Execute tools and show the results
   - Only explain your approach if asked or if something unusual happens

## Browser Automation Tools (CRITICAL)

You have direct access to the user's browser. Use these tools for web-based tasks:

### Primary Tools (Use These First!)

- **browser_snapshot**: Get the page accessibility tree with ref IDs (PRIMARY TOOL!)
  - Returns all interactive elements with unique ref IDs like e15, e23
  - Very efficient: only 2-5K tokens vs 20K for screenshots
  - Use this INSTEAD of browser_screenshot to observe page state

- **browser_click**: Click on an element using ref ID (preferred) or CSS selector
  - With ref: browser_click with ref parameter set to the element ID
  - With selector: browser_click with selector parameter

- **browser_fill**: Fill a form field using ref ID (preferred) or CSS selector
  - With ref: browser_fill with ref and text parameters
  - With selector: browser_fill with selector and text parameters

- **browser_navigate**: Navigate to a URL
  Example: Navigate to mail.google.com

### Secondary Tools

- **browser_get_page_text**: Get page text content (very low tokens)
- **browser_find**: Find element by natural language description
- **browser_execute_script**: Execute JavaScript for complex interactions

### Avoid Unless Necessary

- **browser_screenshot**: AVOID - uses 20K tokens and can overflow context!
  Only use when you specifically need visual information that accessibility tree cannot provide.

### Browser Automation Workflow

For web tasks like sending an email or posting on social media:

1. **Navigate**: Use browser_navigate to go to the website
2. **Observe**: Use browser_snapshot to get the accessibility tree with ref IDs
3. **Interact**: Use browser_click and browser_fill with ref parameter for reliable interaction
4. **Verify**: Use browser_snapshot or browser_get_page_text to verify
5. **Repeat**: Continue until the task is complete

### Example Workflow: Sending an Email

1. browser_navigate to the email service
2. browser_snapshot to see the inbox (returns elements with ref IDs)
3. browser_click with ref pointing to the Compose button
4. browser_snapshot to see the compose dialog
5. browser_fill with ref pointing to To field, text set to recipient
6. browser_fill with ref pointing to Subject field, text set to subject
7. browser_fill with ref pointing to message body, text set to email content
8. browser_click with ref pointing to Send button
9. browser_snapshot to verify the email was sent

## Code Execution

You also have access to code execution for non-browser tasks:

```python
# Example: Process data
import pandas as pd
data = pd.read_csv("data.csv")
print(data.describe())
```

```bash
# Example: List files
ls -la /workspace
```

## Tool Selection Guidelines

| Task Type | Primary Tools |
|-----------|--------------|
| Web services (Gmail, Twitter, etc.) | browser_* tools |
| File operations | Read, Write, Edit |
| Data processing | Python code execution |
| System commands | Bash |
| API integrations | Python requests or MCP tools |

## Important Guidelines

- **Browser for web**: Always use browser tools for web-based tasks, not APIs (unless specifically asked)
- **Be proactive**: Don't ask "should I do X?" - just do it if it makes sense
- **Handle errors gracefully**: If something fails, try an alternative approach
- **Respect privacy**: Don't access sensitive data without explicit permission
- **Verify actions**: Always verify that browser actions completed successfully

## Iterative Problem Solving (CRITICAL)

You MUST follow this workflow for every task:
1. **Plan** what you need to do
2. **Execute** using the appropriate tool (browser tools for web, code for processing)
3. **Verify** the result - did it work? Did you get what was expected?
4. **If NOT done**: Call the next tool to continue. Do NOT stop after one tool call.
5. **If there's an error**: Try a different approach. Do NOT give up after one attempt.
6. **Only respond with text (no tool calls) when the task is fully complete** and you have verified the result.

NEVER stop after executing just one tool unless the task is trivially simple. For any non-trivial task, you should make MULTIPLE tool calls to plan, execute, verify, and refine."#;

        // Build dynamic skills list from registry
        let skills_list = self.build_skills_list();
        let base_prompt = base_prompt.replace("$SKILLS_LIST", &skills_list);

        // Build environment context and append to system prompt
        let env_context = self.build_environment_context();

        let mut prompt = format!("{}\n\n{}", base_prompt, env_context);

        // Append CLAUDE.md if it exists
        if let Some(claude_md) = self.load_claude_md() {
            prompt.push_str("\n\n<project-rules>\n");
            prompt.push_str(&claude_md);
            prompt.push_str("\n</project-rules>");
        }

        prompt
    }

    /// Build a formatted list of available skills for the system prompt
    fn build_skills_list(&self) -> String {
        let skills = self.skills.list();
        if skills.is_empty() {
            return "No skills available.".to_string();
        }

        let mut lines = Vec::new();
        for skill in skills {
            let arg_hint = skill
                .argument_hint
                .as_ref()
                .map(|h| format!(" {}", h))
                .unwrap_or_default();
            lines.push(format!(
                "- `/{}{}`  - {}",
                skill.name, arg_hint, skill.description
            ));
        }
        lines.join("\n")
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// A41: AgentLoopContext — non-blocking streaming fix
//
// The agent loop previously captured `&mut self` via the `try_stream!` macro,
// which held a mutable borrow for the entire stream lifetime. In PlanExecute
// mode this caused deadlocks because the step executor held `agent.write()`
// for the entire multi-turn loop.
//
// AgentLoopContext is a lightweight snapshot of AgentRunner fields (all Arc
// or Clone), created at the start of each `query()` call. The `try_stream!`
// captures this context by move instead of borrowing `&mut AgentRunner`,
// releasing the mutable borrow immediately when `query()` returns.
// ═══════════════════════════════════════════════════════════════════════════════

/// Lightweight context for running the agent loop without borrowing `&mut AgentRunner`.
///
/// Created at the start of each `query()` call by cloning Arc references from
/// `AgentRunner`. The resulting `try_stream!` captures this struct by move,
/// so the stream does NOT hold a mutable borrow on the runner. This is the core
/// of the A41 non-blocking streaming fix.
struct AgentLoopContext {
    config: AgentConfig,
    state: Arc<AgentState>,
    hooks: Arc<HookExecutor>,
    llm: Option<Arc<dyn LlmClient>>,
    tools: Option<Arc<dyn ToolExecutor>>,
    permission_context: Option<PermissionContext>,
    compactor: Arc<ContextCompactor>,
    checkpoint_manager: Option<Arc<dyn CheckpointManager + Send + Sync>>,
    checkpoint_trigger: Arc<Mutex<Option<AutoCheckpointTrigger>>>,
    working_memory: Arc<RwLock<WorkingMemory>>,
    rte_delegation: Option<crate::rte::RteDelegationContext>,
    /// Pre-computed system prompt (built from config, skills, env, CLAUDE.md).
    system_prompt: String,
    #[cfg(feature = "prompt-constraints")]
    constraint_validator: Option<Arc<crate::prompt::ConstraintValidator>>,
    #[cfg(feature = "context-engineering")]
    observer: Option<Arc<dyn crate::agent::context::AgentObserver>>,
}

impl AgentLoopContext {
    fn hook_context(&self) -> HookContext {
        HookContext {
            session_id: self.state.session_id.clone(),
            cwd: self
                .config
                .cwd
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            env: None,
            metadata: None,
        }
    }

    fn tool_context(&self) -> ToolContext {
        ToolContext::new(
            &self.state.session_id,
            self.config
                .cwd
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
        )
        .with_permission_mode(self.config.permission_mode)
        .with_timeout(self.config.tool_timeout_secs)
    }

    async fn create_checkpoint(
        &self,
        trigger: CheckpointTrigger,
        label: Option<&str>,
    ) -> Option<String> {
        let manager = self.checkpoint_manager.as_ref()?;
        let messages = self.state.messages().await;
        let usage = self.state.usage().await;
        let context_state = ContextState {
            cwd: self
                .config
                .cwd
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            env: std::collections::HashMap::new(),
            estimated_tokens: self.compactor.estimate_tokens(&messages),
            turn_count: self.state.turn(),
            usage,
            total_cost_usd: self.state.total_cost_usd().await,
            custom: std::collections::HashMap::new(),
        };
        let mut checkpoint =
            Checkpoint::new(&self.state.session_id, messages, context_state).with_trigger(trigger);
        if let Some(label) = label {
            checkpoint = checkpoint.with_label(label);
        }
        match manager.save(&checkpoint).await {
            Ok(id) => {
                tracing::info!(checkpoint_id = %id, session_id = %self.state.session_id, trigger = ?trigger, "Checkpoint created");
                Some(id)
            }
            Err(e) => {
                tracing::error!(session_id = %self.state.session_id, error = %e, "Failed to create checkpoint");
                None
            }
        }
    }

    async fn maybe_checkpoint_before_tool(&self, tool_name: &str, tool_input: &serde_json::Value) {
        if self.checkpoint_manager.is_none() {
            return;
        }
        let trigger = {
            let guard = self.checkpoint_trigger.lock().await;
            if let Some(ref trigger) = *guard {
                trigger.should_checkpoint_before_tool(tool_name, tool_input)
            } else {
                None
            }
        };
        if let Some(checkpoint_trigger) = trigger {
            let label = format!("Before {} tool execution", tool_name);
            self.create_checkpoint(checkpoint_trigger, Some(&label))
                .await;
        }
    }

    async fn maybe_checkpoint_periodic(&self) {
        if self.checkpoint_manager.is_none() {
            return;
        }
        let trigger = {
            let mut guard = self.checkpoint_trigger.lock().await;
            if let Some(ref mut trigger) = *guard {
                trigger.should_checkpoint_periodic()
            } else {
                None
            }
        };
        if let Some(checkpoint_trigger) = trigger {
            self.create_checkpoint(checkpoint_trigger, Some("Periodic checkpoint"))
                .await;
        }
    }

    async fn maybe_checkpoint_before_compaction(&self) {
        if self.checkpoint_manager.is_none() {
            return;
        }
        let trigger = {
            let guard = self.checkpoint_trigger.lock().await;
            if let Some(ref trigger) = *guard {
                trigger.should_checkpoint_before_compaction()
            } else {
                None
            }
        };
        if let Some(checkpoint_trigger) = trigger {
            self.create_checkpoint(checkpoint_trigger, Some("Before context compaction"))
                .await;
        }
    }

    async fn maybe_compact_context(&self) -> Result<bool, AgentError> {
        if !self.config.compaction.enabled {
            return Ok(false);
        }

        let messages = self.state.messages().await;

        if !self.compactor.needs_compaction(&messages) {
            return Ok(false);
        }

        let estimated_tokens = self.compactor.estimate_tokens(&messages);

        tracing::info!(
            session_id = %self.state.session_id,
            message_count = messages.len(),
            estimated_tokens = estimated_tokens,
            max_tokens = self.config.compaction.max_context_tokens,
            "Context compaction triggered"
        );

        let pre_compact_data = serde_json::json!({
            "session_id": &self.state.session_id,
            "message_count": messages.len(),
            "estimated_tokens": estimated_tokens,
            "trigger": "token_limit",
        });

        let (hook_result, _) = self
            .hooks
            .execute_and_aggregate(
                HookEvent::PreCompact,
                pre_compact_data,
                &self.hook_context(),
                None,
            )
            .await;

        if hook_result.is_cancel() {
            tracing::info!(
                session_id = %self.state.session_id,
                "Context compaction cancelled by PreCompact hook"
            );
            return Ok(false);
        }

        let compact_result = self
            .compactor
            .compact(&messages, CompactTrigger::TokenLimit(estimated_tokens))
            .await
            .map_err(|e| AgentError::SessionError(format!("Compaction failed: {}", e)))?;

        self.state.replace_messages(compact_result.messages).await;

        tracing::info!(
            session_id = %self.state.session_id,
            tokens_before = compact_result.tokens_before,
            tokens_after = compact_result.tokens_after,
            messages_removed = compact_result.messages_removed,
            "Context compacted successfully"
        );

        let post_compact_data = serde_json::json!({
            "session_id": &self.state.session_id,
            "tokens_before": compact_result.tokens_before,
            "tokens_after": compact_result.tokens_after,
            "messages_removed": compact_result.messages_removed,
            "summary": compact_result.summary,
        });

        self.hooks
            .execute(
                HookEvent::PostCompact,
                post_compact_data,
                &self.hook_context(),
            )
            .await;

        Ok(true)
    }
}

#[async_trait]
impl AgentLoop for AgentRunner {
    /// Run the agent with a prompt, returning a stream of messages.
    ///
    /// **A41 non-blocking streaming fix**: The returned stream does NOT hold a
    /// mutable borrow on `self`. Instead, all needed fields are cloned into an
    /// `AgentLoopContext` which the stream captures by move. This allows the
    /// caller (e.g. PlanExecute step executor) to release the write lock on
    /// the agent immediately after calling `query()`, preventing deadlocks.
    async fn query(
        &mut self,
        prompt: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<AgentMessage, AgentError>> + Send + 'static>> {
        let prompt = prompt.to_string();

        // Pre-compute system prompt while we still have &self access.
        // default_system_prompt() reads skills, env context, CLAUDE.md — all read-only.
        let system_prompt = self
            .config
            .system_prompt
            .clone()
            .unwrap_or_else(|| self.default_system_prompt());

        // Snapshot all fields into a lightweight context (cheap Arc clones).
        let ctx = AgentLoopContext {
            config: self.config.clone(),
            state: self.state.clone(),
            hooks: self.hooks.clone(),
            llm: self.llm.clone(),
            tools: self.tools.clone(),
            permission_context: self.permission_context.clone(),
            compactor: self.compactor.clone(),
            checkpoint_manager: self.checkpoint_manager.clone(),
            checkpoint_trigger: self.checkpoint_trigger.clone(),
            working_memory: self.working_memory.clone(),
            rte_delegation: self.rte_delegation.clone(),
            system_prompt,
            #[cfg(feature = "prompt-constraints")]
            constraint_validator: self.constraint_validator.clone(),
            #[cfg(feature = "context-engineering")]
            observer: self.observer.clone(),
        };

        // The stream captures `ctx` by move — no &mut self borrow held.
        Box::pin(async_stream::try_stream! {
            // Mark as running
            ctx.state.set_running(true);
            ctx.state.clear_interrupt();

            // Take the permission response receiver for inline waiting (used if tools need approval)
            let mut permission_rx = ctx.state.take_permission_receiver().await;

            // Session start hook
            ctx.hooks.execute(
                HookEvent::SessionStart,
                serde_json::json!({"prompt": &prompt}),
                &ctx.hook_context(),
            ).await;

            // Add system message with agent instructions (only on first query of session)
            if ctx.state.message_count().await == 0 {
                let system_message = AgentMessage::System(SystemMessage {
                    subtype: "agent_instructions".to_string(),
                    data: serde_json::Value::String(ctx.system_prompt.clone()),
                });
                ctx.state.add_message(system_message.clone()).await;
                yield system_message;
            }

            // Pre-flight constraint validation
            #[cfg(feature = "prompt-constraints")]
            if let Some(ref validator) = ctx.constraint_validator {
                let validation = validator.validate_input(&prompt);
                if !validation.is_valid() {
                    let issues: Vec<String> = validation.hard_issues()
                        .map(|i| i.message.clone()).collect();
                    tracing::warn!(
                        session_id = %ctx.state.session_id,
                        issues = ?issues,
                        "Pre-flight constraint violation: input blocked"
                    );
                    yield AgentMessage::System(SystemMessage {
                        subtype: "constraint_violation".to_string(),
                        data: serde_json::json!({
                            "type": "constraint_violation",
                            "issues": issues,
                        }),
                    });
                    ctx.state.set_running(false);
                    return;
                }
                // Yield warnings as system messages (non-blocking)
                if validation.has_warnings() {
                    let warnings: Vec<_> = validation.soft_issues()
                        .map(|i| serde_json::json!({
                            "message": i.message,
                            "suggestion": i.suggestion,
                        })).collect();
                    yield AgentMessage::System(SystemMessage {
                        subtype: "constraint_warnings".to_string(),
                        data: serde_json::json!({
                            "type": "constraint_warnings",
                            "warnings": warnings,
                        }),
                    });
                }

                // Observer: pre-flight check
                #[cfg(feature = "context-engineering")]
                if let Some(ref observer) = ctx.observer {
                    let all_issues: Vec<String> = validation.hard_issues()
                        .chain(validation.soft_issues())
                        .map(|i| i.message.clone())
                        .collect();
                    observer.on_preflight_check(validation.is_valid(), &all_issues).await;
                }
            }

            // Add user message
            let message_uuid = uuid::Uuid::new_v4();
            let user_message = AgentMessage::User(UserMessage {
                content: MessageContent::text(&prompt),
                uuid: Some(message_uuid),
                parent_tool_use_id: None,
                tool_use_result: None,
            });
            ctx.state.add_message(user_message.clone()).await;
            yield user_message.clone();

            // UserPromptSubmit hook - triggered after user message is added to state
            // Hook execution is non-blocking: failures are logged but don't stop the flow
            let user_prompt_hook_data = serde_json::json!({
                "prompt": &prompt,
                "session_id": &ctx.state.session_id,
                "message_uuid": message_uuid.to_string(),
                "turn": ctx.state.turn(),
            });
            // Execute hook with timeout to prevent blocking main flow
            let hooks_clone = ctx.hooks.clone();
            let hook_ctx = ctx.hook_context();
            let hook_future = async move {
                hooks_clone.execute(
                    HookEvent::UserPromptSubmit,
                    user_prompt_hook_data,
                    &hook_ctx,
                ).await
            };
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                hook_future
            ).await {
                Ok(_) => {
                    tracing::debug!(
                        session_id = %ctx.state.session_id,
                        "UserPromptSubmit hook executed successfully"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        session_id = %ctx.state.session_id,
                        "UserPromptSubmit hook timed out, continuing execution"
                    );
                }
            }

            // Main loop
            loop {
                // Check for interrupt
                if ctx.state.is_interrupted() {
                    let result = ResultMessage {
                        subtype: ResultSubtype::Interrupted,
                        duration_ms: ctx.state.elapsed_ms(),
                        duration_api_ms: 0,
                        is_error: false,
                        num_turns: ctx.state.turn(),
                        session_id: ctx.state.session_id.clone(),
                        total_cost_usd: Some(ctx.state.total_cost_usd().await),
                        usage: Some(ctx.state.usage().await),
                        result: None,
                        structured_output: None,
                    };
                    yield AgentMessage::Result(result);
                    break;
                }

                // Check max turns
                let turn = ctx.state.increment_turn();
                if turn > ctx.config.max_turns {
                    let result = ResultMessage {
                        subtype: ResultSubtype::ErrorMaxTurns,
                        duration_ms: ctx.state.elapsed_ms(),
                        duration_api_ms: 0,
                        is_error: true,
                        num_turns: turn,
                        session_id: ctx.state.session_id.clone(),
                        total_cost_usd: Some(ctx.state.total_cost_usd().await),
                        usage: Some(ctx.state.usage().await),
                        result: Some(format!("Exceeded maximum turns: {}", ctx.config.max_turns)),
                        structured_output: None,
                    };
                    yield AgentMessage::Result(result);
                    break;
                }

                // Check max budget
                if let Some(max_budget) = ctx.config.max_budget_usd {
                    if ctx.state.total_cost_usd().await > max_budget {
                        let result = ResultMessage {
                            subtype: ResultSubtype::ErrorMaxBudgetUsd,
                            duration_ms: ctx.state.elapsed_ms(),
                            duration_api_ms: 0,
                            is_error: true,
                            num_turns: turn,
                            session_id: ctx.state.session_id.clone(),
                            total_cost_usd: Some(ctx.state.total_cost_usd().await),
                            usage: Some(ctx.state.usage().await),
                            result: Some(format!("Exceeded maximum budget: ${:.2}", max_budget)),
                            structured_output: None,
                        };
                        yield AgentMessage::Result(result);
                        break;
                    }
                }

                // Check for periodic checkpoint
                ctx.maybe_checkpoint_periodic().await;

                // Check if context compaction is needed before calling LLM
                // This ensures the agent can run for unlimited turns by keeping context size manageable
                // First, create a checkpoint before compaction if configured
                ctx.maybe_checkpoint_before_compaction().await;
                if let Err(e) = ctx.maybe_compact_context().await {
                    tracing::warn!(
                        session_id = %ctx.state.session_id,
                        error = %e,
                        "Context compaction failed, continuing without compaction"
                    );
                }

                // Call LLM (placeholder - returns mock response without actual LLM)
                let llm = match &ctx.llm {
                    Some(llm) => llm,
                    None => {
                        // No LLM configured - return placeholder
                        let result = ResultMessage {
                            subtype: ResultSubtype::Success,
                            duration_ms: ctx.state.elapsed_ms(),
                            duration_api_ms: 0,
                            is_error: false,
                            num_turns: turn,
                            session_id: ctx.state.session_id.clone(),
                            total_cost_usd: None,
                            usage: Some(ctx.state.usage().await),
                            result: Some("LLM not configured".to_string()),
                            structured_output: None,
                        };
                        yield AgentMessage::Result(result);
                        break;
                    }
                };

                // Dynamic tool loading: detect task type from messages and filter tools
                let messages = ctx.state.messages().await;
                let tool_schemas = ctx.tools.as_ref()
                    .map(|t| {
                        // Check ALL user messages for browser task keywords (not just the latest)
                        // This ensures browser tools stay available throughout a browser automation session
                        let is_browser_task = messages.iter().any(|msg| {
                            match msg {
                                AgentMessage::User(user_msg) => {
                                    ToolFilterContext::detect_browser_task(&user_msg.content.to_string_content())
                                }
                                // Also check if browser tools were already used in this session
                                AgentMessage::Assistant(assistant_msg) => {
                                    assistant_msg.content.iter().any(|block| {
                                        if let ContentBlock::ToolUse { name, .. } = block {
                                            name.starts_with("computer_") || name.starts_with("browser_")
                                        } else {
                                            false
                                        }
                                    })
                                }
                                _ => false,
                            }
                        });

                        // Detect browser task and update filter context
                        let filter_context = ToolFilterContext::new()
                            .browser_task(is_browser_task);

                        tracing::debug!(
                            session_id = %ctx.state.session_id,
                            is_browser_task = filter_context.is_browser_task,
                            "Dynamic tool loading: detected task type"
                        );

                        // Use filtered schemas to reduce token usage
                        t.get_filtered_tool_schemas(&filter_context)
                    })
                    .unwrap_or_default();

                {
                    let sid = ctx.state.session_id.clone();
                    let mc = ctx.state.message_count().await;
                    tracing::info!(
                        session_id = %sid,
                        turn = turn,
                        message_count = mc,
                        "Agent loop: calling LLM"
                    );
                }

                // Observer: LLM request
                #[cfg(feature = "context-engineering")]
                let llm_call_start = std::time::Instant::now();
                #[cfg(feature = "context-engineering")]
                if let Some(ref observer) = ctx.observer {
                    let msg_count = ctx.state.message_count().await;
                    observer.on_llm_request("pending", msg_count, 0).await;
                }

                let response = llm.generate(
                    messages,
                    tool_schemas,
                ).await?;

                // Update usage
                ctx.state.add_usage(&response.usage).await;

                // Observer: LLM response
                #[cfg(feature = "context-engineering")]
                if let Some(ref observer) = ctx.observer {
                    let duration_ms = llm_call_start.elapsed().as_millis() as u64;
                    let output_tokens = response.usage.output_tokens as usize;
                    observer.on_llm_response(&response.model, duration_ms, output_tokens).await;
                }

                // Check for tool use
                let has_tool_use = response.content.iter().any(|b| b.is_tool_use());

                {
                    let sid = ctx.state.session_id.clone();
                    let tool_names_str: String = response.content.iter()
                        .filter_map(|b| if let ContentBlock::ToolUse { name, .. } = b { Some(name.as_str()) } else { None })
                        .collect::<Vec<_>>()
                        .join(", ");
                    tracing::info!(
                        session_id = %sid,
                        turn = turn,
                        has_tool_use = has_tool_use,
                        tool_names = %tool_names_str,
                        "Agent loop: LLM response received"
                    );
                }

                // Post-flight constraint validation on LLM output
                #[cfg(feature = "prompt-constraints")]
                if let Some(ref validator) = ctx.constraint_validator {
                    let text_content: String = response.content.iter()
                        .filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text_content.is_empty() {
                        let validation = validator.validate_output(&text_content);
                        if !validation.is_valid() {
                            tracing::warn!(
                                session_id = %ctx.state.session_id,
                                issues = ?validation.issues.iter().map(|i| &i.message).collect::<Vec<_>>(),
                                "Post-flight constraint violation in LLM output"
                            );
                            // For now: log and continue (WarnOnly behavior)
                            // Future: implement RepairAndRetry loop
                        }
                        if validation.has_warnings() {
                            tracing::debug!(
                                session_id = %ctx.state.session_id,
                                warnings = ?validation.soft_issues().map(|i| &i.message).collect::<Vec<_>>(),
                                "Post-flight constraint warnings in LLM output"
                            );
                        }

                        // Observer: post-flight check
                        #[cfg(feature = "context-engineering")]
                        if let Some(ref observer) = ctx.observer {
                            observer.on_postflight_check(validation.is_valid(), false).await;
                        }
                    }
                }

                // Yield assistant message
                let assistant_message = AgentMessage::Assistant(AssistantMessage {
                    content: response.content.clone(),
                    model: response.model.clone(),
                    parent_tool_use_id: None,
                    error: None,
                });
                ctx.state.add_message(assistant_message.clone()).await;
                yield assistant_message;

                // Process tool uses in parallel
                if has_tool_use {
                    // Step 1: Collect all tool uses from the response
                    let tool_uses: Vec<_> = response.content.iter()
                        .filter_map(|block| {
                            if let ContentBlock::ToolUse { id, name, input } = block {
                                Some((id.clone(), name.clone(), input.clone()))
                            } else {
                                None
                            }
                        })
                        .collect();

                    // Step 1.5: Check if we should create a checkpoint before any dangerous tool
                    // This is done before parallel execution starts
                    for (_, name, input) in &tool_uses {
                        ctx.maybe_checkpoint_before_tool(name, input).await;
                    }

                    // Step 2: Create futures for parallel execution
                    let tools_ref = ctx.tools.clone();
                    let hooks_ref = ctx.hooks.clone();
                    let hook_context = ctx.hook_context();
                    let tool_context = ctx.tool_context();
                    let permission_context = ctx.permission_context.clone();
                    let working_memory_ref = ctx.working_memory.clone();
                    let rte_delegation_ref = ctx.rte_delegation.clone();
                    #[cfg(feature = "context-engineering")]
                    let observer_ref = ctx.observer.clone();

                    // Belt-and-suspenders: capture config permission mode so the
                    // bypass cannot be lost even if PermissionContext state diverges.
                    let config_bypass = ctx.config.permission_mode == PermissionMode::BypassPermissions;

                    let futures: Vec<_> = tool_uses.iter().map(|(id, name, input)| {
                        let id = id.clone();
                        let name = name.clone();
                        let input = input.clone();
                        let tools = tools_ref.clone();
                        let hooks = hooks_ref.clone();
                        let hook_ctx = hook_context.clone();
                        let tool_ctx = tool_context.clone();
                        let perm_ctx = permission_context.clone();
                        let working_memory = working_memory_ref.clone();
                        let rte_delegation = rte_delegation_ref.clone();
                        #[cfg(feature = "context-engineering")]
                        let observer = observer_ref.clone();

                        async move {
                            let tool_use_id = id.clone();
                            let start_time = std::time::Instant::now();

                            // Record tool state as executing in working memory
                            {
                                let mut wm = working_memory.write().await;
                                let mut tool_state = ToolState::new(&name);
                                tool_state.set_executing(format!("tool_use_id: {}", tool_use_id));
                                wm.set_tool_state(&name, tool_state);
                            }

                            // Check permissions before executing tool.
                            // Config-level bypass takes absolute precedence to prevent
                            // any PermissionContext state issue from blocking execution.
                            let permission_result = if config_bypass {
                                PermissionResult::allow()
                            } else {
                                match &perm_ctx {
                                    Some(ctx) => ctx.check_tool(&name, &input),
                                    None => PermissionResult::allow(),
                                }
                            };

                            match permission_result {
                                PermissionResult::Allow { updated_input, .. } => {
                                    // Use updated input if provided by permission check
                                    let permission_input = updated_input.unwrap_or_else(|| input.clone());

                                    // ==========================================
                                    // PreToolUse Hook - triggered before each tool execution
                                    // Hook execution is non-blocking: failures are logged but don't stop the flow
                                    // ==========================================
                                    let pre_hook_data = serde_json::json!({
                                        "tool_name": &name,
                                        "tool_input": &permission_input,
                                        "tool_use_id": &tool_use_id,
                                        "session_id": &hook_ctx.session_id,
                                    });

                                    tracing::debug!(
                                        tool_name = %name,
                                        tool_use_id = %tool_use_id,
                                        "Executing PreToolUse hook"
                                    );

                                    // Execute PreToolUse hook with timeout to prevent blocking
                                    let pre_hook_result = tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        hooks.execute_and_aggregate(
                                            HookEvent::PreToolUse,
                                            pre_hook_data,
                                            &hook_ctx,
                                            Some(&name),
                                        )
                                    ).await;

                                    let (pre_result, modified_input) = match pre_hook_result {
                                        Ok(result) => {
                                            tracing::debug!(
                                                tool_name = %name,
                                                tool_use_id = %tool_use_id,
                                                "PreToolUse hook executed successfully"
                                            );
                                            result
                                        }
                                        Err(_) => {
                                            // Hook timed out - log warning and continue with original input
                                            tracing::warn!(
                                                tool_name = %name,
                                                tool_use_id = %tool_use_id,
                                                "PreToolUse hook timed out, continuing with original input"
                                            );
                                            (HookResult::continue_(), None)
                                        }
                                    };

                                    // Check if hook cancelled the operation
                                    if pre_result.is_cancel() {
                                        tracing::info!(
                                            tool_name = %name,
                                            tool_use_id = %tool_use_id,
                                            "Tool execution cancelled by PreToolUse hook"
                                        );
                                        let error = serde_json::json!({"error": "Cancelled by pre-tool-use hook"});
                                        return (id, name, input, error, true);
                                    }

                                    let actual_input = modified_input.unwrap_or(permission_input);

                                    // RTE delegation check (A28): try delegating to native client
                                    if let Some(ref rte) = rte_delegation {
                                        match rte.try_delegate(&name, actual_input.clone()).await {
                                            crate::rte::DelegationResult::Completed(rte_result) => {
                                                let is_error = !rte_result.success;
                                                let result = if is_error {
                                                    serde_json::json!({
                                                        "error": rte_result.error.unwrap_or_else(|| "RTE execution failed".to_string())
                                                    })
                                                } else {
                                                    rte_result.result
                                                };
                                                let duration_ms = rte_result.execution_time_ms;
                                                // Record in working memory
                                                {
                                                    let mut wm = working_memory.write().await;
                                                    if let Some(tool_state) = wm.tool_states.get_mut(&name) {
                                                        tool_state.record_execution(!is_error);
                                                    }
                                                    if let Some(task_id) = wm.task_tree.current_id.clone() {
                                                        let tool_call = ToolCallRecord::new(&name, actual_input.clone())
                                                            .with_result(result.clone(), !is_error);
                                                        wm.record_tool_call(&task_id, tool_call);
                                                    }
                                                }
                                                tracing::info!(
                                                    tool_name = %name,
                                                    tool_use_id = %tool_use_id,
                                                    duration_ms = duration_ms,
                                                    success = !is_error,
                                                    "Tool executed via RTE (native client)"
                                                );
                                                return (id, name, input, result, is_error);
                                            }
                                            crate::rte::DelegationResult::TimedOut(fallback) => {
                                                match fallback {
                                                    crate::rte::FallbackStrategy::Error => {
                                                        let error = serde_json::json!({"error": "RTE tool execution timed out"});
                                                        return (id, name, input, error, true);
                                                    }
                                                    crate::rte::FallbackStrategy::Skip => {
                                                        let skip = serde_json::json!({"skipped": "RTE timeout, tool skipped"});
                                                        return (id, name, input, skip, false);
                                                    }
                                                    // CloudExecution or Retry — fall through to local execution
                                                    _ => {
                                                        tracing::info!(
                                                            tool_name = %name,
                                                            "RTE timed out, falling back to cloud execution"
                                                        );
                                                    }
                                                }
                                            }
                                            crate::rte::DelegationResult::NotDelegated => {
                                                // Not eligible for RTE — execute locally
                                            }
                                        }
                                    }

                                    // Execute tool (local / cloud execution)
                                    let (result, is_error) = if let Some(ref tools) = tools {
                                        match tools.execute(&name, actual_input.clone(), &tool_ctx).await {
                                            Ok(result) => (result, false),
                                            Err(e) => {
                                                tracing::error!(
                                                    tool_name = %name,
                                                    tool_use_id = %tool_use_id,
                                                    error = %e,
                                                    "Tool execution failed"
                                                );
                                                (serde_json::json!({"error": e.to_string()}), true)
                                            }
                                        }
                                    } else {
                                        (serde_json::json!({"error": "No tool executor configured"}), true)
                                    };

                                    let duration_ms = start_time.elapsed().as_millis() as u64;

                                    // Observer: tool call completed
                                    #[cfg(feature = "context-engineering")]
                                    if let Some(ref obs) = observer {
                                        obs.on_tool_call(&name, duration_ms, !is_error).await;
                                    }

                                    // Record tool call result in working memory
                                    {
                                        let mut wm = working_memory.write().await;
                                        // Update tool state
                                        if let Some(tool_state) = wm.tool_states.get_mut(&name) {
                                            if is_error {
                                                tool_state.set_error(
                                                    result.get("error")
                                                        .and_then(|e| e.as_str())
                                                        .unwrap_or("Unknown error")
                                                );
                                            } else {
                                                tool_state.set_idle();
                                            }
                                        }

                                        // Record tool call if there's a current task
                                        if let Some(task_id) = wm.task_tree.current_id.clone() {
                                            let tool_call = ToolCallRecord::new(&name, actual_input.clone())
                                                .with_result(result.clone(), !is_error);
                                            wm.record_tool_call(&task_id, tool_call);
                                        }
                                    }

                                    // ==========================================
                                    // PostToolUse Hook - triggered after each tool execution
                                    // Hook execution is non-blocking: failures are logged but don't stop the flow
                                    // ==========================================
                                    let post_hook_data = serde_json::json!({
                                        "tool_name": &name,
                                        "tool_input": &actual_input,
                                        "tool_result": &result,
                                        "tool_use_id": &tool_use_id,
                                        "is_error": is_error,
                                        "duration_ms": duration_ms,
                                        "session_id": &hook_ctx.session_id,
                                    });

                                    tracing::debug!(
                                        tool_name = %name,
                                        tool_use_id = %tool_use_id,
                                        is_error = is_error,
                                        duration_ms = duration_ms,
                                        "Executing PostToolUse hook"
                                    );

                                    // Execute PostToolUse hook with timeout to prevent blocking
                                    match tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        hooks.execute(HookEvent::PostToolUse, post_hook_data, &hook_ctx)
                                    ).await {
                                        Ok(_) => {
                                            tracing::debug!(
                                                tool_name = %name,
                                                tool_use_id = %tool_use_id,
                                                "PostToolUse hook executed successfully"
                                            );
                                        }
                                        Err(_) => {
                                            tracing::warn!(
                                                tool_name = %name,
                                                tool_use_id = %tool_use_id,
                                                "PostToolUse hook timed out, continuing execution"
                                            );
                                        }
                                    }

                                    (id, name, actual_input, result, is_error)
                                }
                                PermissionResult::Deny { message, .. } => {
                                    // Permission denied - return error to LLM
                                    tracing::info!(
                                        tool_name = %name,
                                        tool_use_id = %tool_use_id,
                                        message = %message,
                                        "Tool execution denied by permission check"
                                    );
                                    let error = serde_json::json!({
                                        "error": format!("Permission denied: {}", message)
                                    });
                                    (id, name, input, error, true)
                                }
                                PermissionResult::Ask { question, .. } => {
                                    // Ask requires user approval - return a special marker
                                    // The outer loop will handle creating the permission request
                                    tracing::info!(
                                        tool_name = %name,
                                        tool_use_id = %tool_use_id,
                                        question = %question,
                                        "Tool execution requires user approval"
                                    );
                                    // Return a special result that indicates permission is needed
                                    // We use a specific error format that the outer loop can detect
                                    let pending = serde_json::json!({
                                        "permission_required": true,
                                        "question": question,
                                        "tool_name": name,
                                        "tool_input": input,
                                    });
                                    (id, name, input, pending, true)
                                }
                            }
                        }
                    }).collect();

                    // Step 3: Execute all tools in parallel
                    {
                        let sid = ctx.state.session_id.clone();
                        let fc = futures.len();
                        tracing::info!(
                            session_id = %sid,
                            turn = turn,
                            tool_count = fc,
                            "Agent loop: executing tools in parallel"
                        );
                    }
                    let results = join_all(futures).await;
                    {
                        let sid = ctx.state.session_id.clone();
                        let rc = results.len();
                        tracing::info!(
                            session_id = %sid,
                            turn = turn,
                            result_count = rc,
                            "Agent loop: all tools completed"
                        );
                    }

                    // Step 4: Collect results and create tool result messages
                    // Separate permission-required tools from completed tools
                    let mut pending_tools: Vec<(String, String, serde_json::Value, String)> = Vec::new(); // (id, name, input, question)

                    for (id, name, input, result, is_error) in results {
                        // Check if this is a permission request marker
                        if result.get("permission_required").and_then(|v| v.as_bool()) == Some(true) {
                            let question = result.get("question")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Allow this tool to execute?")
                                .to_string();

                            let permission_request = PermissionRequest::new(
                                &name,
                                input.clone(),
                                &question,
                                &ctx.state.session_id,
                            ).with_tool_use_id(&id);

                            // Store the pending permission in state
                            ctx.state.add_pending_permission(permission_request.clone()).await;

                            // Yield the permission request message to the frontend
                            yield AgentMessage::PermissionRequest(permission_request);

                            // Collect for later execution (don't add fake result to conversation)
                            pending_tools.push((id, name, input, question));
                        } else {
                            // Normal tool result — yield immediately
                            let tool_content = if !is_error {
                                extract_image_content_from_result(&name, &result)
                            } else {
                                crate::agent::types::ToolResultContent::Text(
                                    serde_json::to_string(&result).unwrap_or_default()
                                )
                            };

                            let result_message = AgentMessage::User(UserMessage {
                                content: MessageContent::Blocks(vec![
                                    ContentBlock::ToolResult {
                                        tool_use_id: id.clone(),
                                        content: Some(tool_content),
                                        is_error: Some(is_error),
                                    }
                                ]),
                                uuid: Some(uuid::Uuid::new_v4()),
                                parent_tool_use_id: Some(id),
                                tool_use_result: Some(result),
                            });
                            ctx.state.add_message(result_message.clone()).await;
                            yield result_message;
                        }
                    }

                    // If tools need permission, wait for user responses inline (keep SSE stream open)
                    if !pending_tools.is_empty() {
                        tracing::info!(
                            session_id = %ctx.state.session_id,
                            pending_count = pending_tools.len(),
                            "Waiting for permission responses inline"
                        );

                        // Wait for each pending tool's permission response via the channel
                        if let Some(ref mut rx) = permission_rx {
                            let mut resolved = 0;
                            let target = pending_tools.len();
                            // Collect responses: request_id -> granted
                            let mut responses: std::collections::HashMap<String, bool> = std::collections::HashMap::new();

                            while resolved < target {
                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(300), // 5 min timeout per response
                                    rx.recv()
                                ).await {
                                    Ok(Some(response)) => {
                                        resolved += 1;
                                        tracing::info!(
                                            request_id = %response.request_id,
                                            granted = response.granted,
                                            "Permission response received"
                                        );
                                        responses.insert(response.request_id.clone(), response.granted);
                                    }
                                    Ok(None) => {
                                        tracing::warn!("Permission channel closed unexpectedly");
                                        break;
                                    }
                                    Err(_) => {
                                        tracing::warn!("Permission response timeout after 300s");
                                        break;
                                    }
                                }
                            }

                            ctx.state.set_waiting_for_permission(false);

                            // Process each pending tool based on its permission response
                            for (tool_id, tool_name, tool_input, _question) in pending_tools {
                                // Find the permission request_id for this tool
                                let pending_perm = ctx.state.get_all_pending_permissions().await
                                    .into_iter()
                                    .find(|p| p.request.tool_use_id.as_deref() == Some(&tool_id));

                                let granted = pending_perm
                                    .as_ref()
                                    .and_then(|p| responses.get(&p.request.request_id))
                                    .copied()
                                    .unwrap_or(false); // Default deny if not found

                                if granted {
                                    // Tool approved — execute it now
                                    tracing::info!(tool_name = %tool_name, "Executing approved tool");
                                    let (exec_result, exec_is_error) = if let Some(ref tools) = tools_ref {
                                        match tools.execute(&tool_name, tool_input.clone(), &tool_context).await {
                                            Ok(r) => (r, false),
                                            Err(e) => (serde_json::json!({"error": e.to_string()}), true),
                                        }
                                    } else {
                                        (serde_json::json!({"error": "No tool executor available"}), true)
                                    };

                                    let tool_content = if !exec_is_error {
                                        extract_image_content_from_result(&tool_name, &exec_result)
                                    } else {
                                        crate::agent::types::ToolResultContent::Text(
                                            serde_json::to_string(&exec_result).unwrap_or_default()
                                        )
                                    };

                                    let result_message = AgentMessage::User(UserMessage {
                                        content: MessageContent::Blocks(vec![
                                            ContentBlock::ToolResult {
                                                tool_use_id: tool_id.clone(),
                                                content: Some(tool_content),
                                                is_error: Some(exec_is_error),
                                            }
                                        ]),
                                        uuid: Some(uuid::Uuid::new_v4()),
                                        parent_tool_use_id: Some(tool_id),
                                        tool_use_result: Some(exec_result),
                                    });
                                    ctx.state.add_message(result_message.clone()).await;
                                    yield result_message;
                                } else {
                                    // Tool denied — yield error result to LLM
                                    tracing::info!(tool_name = %tool_name, "Tool denied by user");
                                    let error = serde_json::json!({
                                        "error": format!("Permission denied: user declined to allow '{}'", tool_name)
                                    });
                                    let result_message = AgentMessage::User(UserMessage {
                                        content: MessageContent::Blocks(vec![
                                            ContentBlock::ToolResult {
                                                tool_use_id: tool_id.clone(),
                                                content: Some(crate::agent::types::ToolResultContent::Text(
                                                    serde_json::to_string(&error).unwrap_or_default()
                                                )),
                                                is_error: Some(true),
                                            }
                                        ]),
                                        uuid: Some(uuid::Uuid::new_v4()),
                                        parent_tool_use_id: Some(tool_id),
                                        tool_use_result: Some(error),
                                    });
                                    ctx.state.add_message(result_message.clone()).await;
                                    yield result_message;
                                }
                            }
                        } else {
                            // No permission receiver — auto-deny all pending tools
                            tracing::warn!("No permission receiver available, denying all pending tools");
                            for (tool_id, tool_name, _tool_input, _question) in pending_tools {
                                let error = serde_json::json!({
                                    "error": format!("Permission system unavailable for '{}'", tool_name)
                                });
                                let result_message = AgentMessage::User(UserMessage {
                                    content: MessageContent::Blocks(vec![
                                        ContentBlock::ToolResult {
                                            tool_use_id: tool_id.clone(),
                                            content: Some(crate::agent::types::ToolResultContent::Text(
                                                serde_json::to_string(&error).unwrap_or_default()
                                            )),
                                            is_error: Some(true),
                                        }
                                    ]),
                                    uuid: Some(uuid::Uuid::new_v4()),
                                    parent_tool_use_id: Some(tool_id),
                                    tool_use_result: Some(error),
                                });
                                ctx.state.add_message(result_message.clone()).await;
                                yield result_message;
                            }
                        }
                    }

                    // Observer: turn complete (with tool use)
                    #[cfg(feature = "context-engineering")]
                    if let Some(ref observer) = ctx.observer {
                        let usage = ctx.state.usage().await;
                        let total_tokens = (usage.input_tokens + usage.output_tokens) as usize;
                        observer.on_turn_complete(turn, total_tokens).await;
                    }

                    // Tool execution completed, continuing agent loop for next LLM call
                    {
                        let sid = ctx.state.session_id.clone();
                        tracing::info!(
                            session_id = %sid,
                            turn = turn,
                            "Agent loop: tool execution complete, continuing to next turn"
                        );
                    }
                } else {
                    // Observer: turn complete (no tool use — final turn)
                    #[cfg(feature = "context-engineering")]
                    if let Some(ref observer) = ctx.observer {
                        let usage = ctx.state.usage().await;
                        let total_tokens = (usage.input_tokens + usage.output_tokens) as usize;
                        observer.on_turn_complete(turn, total_tokens).await;
                    }

                    // No tool use - we're done
                    let result = ResultMessage {
                        subtype: ResultSubtype::Success,
                        duration_ms: ctx.state.elapsed_ms(),
                        duration_api_ms: 0,
                        is_error: false,
                        num_turns: turn,
                        session_id: ctx.state.session_id.clone(),
                        total_cost_usd: Some(ctx.state.total_cost_usd().await),
                        usage: Some(ctx.state.usage().await),
                        result: response.content.iter()
                            .filter_map(|b| b.as_text())
                            .collect::<Vec<_>>()
                            .join("")
                            .into(),
                        structured_output: None,
                    };
                    yield AgentMessage::Result(result);
                    break;
                }
            }

            // Session end hook
            ctx.hooks.execute(
                HookEvent::SessionEnd,
                serde_json::json!({
                    "num_turns": ctx.state.turn(),
                    "duration_ms": ctx.state.elapsed_ms(),
                }),
                &ctx.hook_context(),
            ).await;

            ctx.state.set_running(false);
        })
    }

    async fn interrupt(&mut self) -> Result<(), AgentError> {
        self.state.interrupt();
        Ok(())
    }

    async fn set_permission_mode(&mut self, mode: PermissionMode) -> Result<(), AgentError> {
        self.state.set_permission_mode(mode).await;
        self.config.permission_mode = mode;
        Ok(())
    }

    fn session_id(&self) -> &str {
        &self.state.session_id
    }

    fn usage(&self) -> &Usage {
        // Note: This is a simplified version. In production, we'd want async access.
        &DEFAULT_USAGE
    }

    fn is_running(&self) -> bool {
        self.state.is_running()
    }
}

/// Known screenshot/image tool names that return base64 image data
const SCREENSHOT_TOOL_NAMES: &[&str] = &[
    "computer_screenshot",
    "browser_screenshot",
    "mac_screenshot",
    "screenshot",
    "take_screenshot",
];

/// Extract image content from a tool result if it's a screenshot tool.
///
/// Screenshot tools return JSON like `{"base64":"iVBOR...","format":"jpeg",...}`.
/// This function detects such results and constructs `ToolResultContent::Blocks`
/// with both a text summary and an Image block so vision models can see the image.
fn extract_image_content_from_result(
    tool_name: &str,
    result: &serde_json::Value,
) -> crate::agent::types::ToolResultContent {
    use crate::agent::types::{ImageSource, ToolResultBlock, ToolResultContent};

    // Only process known screenshot tools
    let is_screenshot_tool = SCREENSHOT_TOOL_NAMES
        .iter()
        .any(|&name| tool_name.eq_ignore_ascii_case(name));

    if !is_screenshot_tool {
        return ToolResultContent::Text(serde_json::to_string(result).unwrap_or_default());
    }

    // Try to extract base64 image data from the result
    let base64_data = result
        .get("base64")
        .or_else(|| result.get("image"))
        .or_else(|| result.get("data"))
        .and_then(|v| v.as_str());

    let base64_data = match base64_data {
        Some(data) if !data.is_empty() => data,
        _ => {
            // No image data found, fall back to text
            tracing::debug!(
                tool_name = %tool_name,
                "Screenshot tool result has no base64 image data, falling back to text"
            );
            return ToolResultContent::Text(serde_json::to_string(result).unwrap_or_default());
        }
    };

    // Determine media type from format field
    let format = result
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("png");

    let media_type = match format.to_lowercase().as_str() {
        "jpeg" | "jpg" => "image/jpeg".to_string(),
        "png" => "image/png".to_string(),
        "webp" => "image/webp".to_string(),
        "gif" => "image/gif".to_string(),
        other => format!("image/{}", other),
    };

    tracing::info!(
        tool_name = %tool_name,
        media_type = %media_type,
        data_len = base64_data.len(),
        "Extracted screenshot image from tool result"
    );

    // Build blocks: text summary + image
    let mut blocks = Vec::new();

    // Add a text summary (without the base64 data to save space)
    let mut summary = result.clone();
    if let Some(obj) = summary.as_object_mut() {
        obj.remove("base64");
        obj.remove("image");
        obj.remove("data");
    }
    blocks.push(ToolResultBlock::Text {
        text: serde_json::to_string(&summary).unwrap_or_default(),
    });

    // Add the image block
    blocks.push(ToolResultBlock::Image {
        source: ImageSource::Base64 {
            media_type,
            data: base64_data.to_string(),
        },
    });

    ToolResultContent::Blocks(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_runner_creation() {
        let config = AgentConfig::default();
        let runner = AgentRunner::new(config);
        assert!(!runner.is_running());
    }

    #[test]
    fn test_agent_runner_with_session_id() {
        let config = AgentConfig::default();
        let runner = AgentRunner::with_session_id(config, "test-session");
        assert_eq!(runner.session_id(), "test-session");
    }

    #[test]
    fn test_agent_runner_permission_check_no_context() {
        let config = AgentConfig::default();
        let runner = AgentRunner::new(config);

        // Without permission context, should allow by default
        let result =
            runner.check_tool_permission("Read", &serde_json::json!({"file_path": "/tmp/test"}));
        assert!(result.is_allowed());
    }

    #[test]
    fn test_agent_runner_permission_check_bypass_mode() {
        let config = AgentConfig::default();
        let ctx = PermissionContext {
            mode: PermissionMode::BypassPermissions,
            ..Default::default()
        };
        let runner = AgentRunner::new(config).with_permission_context(ctx);

        // Bypass mode should allow everything
        let result =
            runner.check_tool_permission("Bash", &serde_json::json!({"command": "rm -rf /"}));
        assert!(result.is_allowed());
    }

    #[test]
    fn test_agent_runner_permission_check_plan_mode() {
        let config = AgentConfig::default();
        let ctx = PermissionContext {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        let runner = AgentRunner::new(config).with_permission_context(ctx);

        // Plan mode should deny Write tool
        let result =
            runner.check_tool_permission("Write", &serde_json::json!({"file_path": "/tmp/test"}));
        assert!(result.is_denied());

        // Plan mode should allow Read tool
        let result =
            runner.check_tool_permission("Read", &serde_json::json!({"file_path": "/tmp/test"}));
        // Read returns Ask in default mode without rules
        assert!(!result.is_denied());
    }

    #[test]
    fn test_build_environment_context() {
        let config = AgentConfig::default();
        let runner = AgentRunner::new(config);

        let env_context = runner.build_environment_context();

        // Verify the context contains expected elements
        assert!(env_context.contains("<env>"));
        assert!(env_context.contains("</env>"));
        assert!(env_context.contains("Working directory:"));
        assert!(env_context.contains("Is directory a git repo:"));
        assert!(env_context.contains("Platform:"));
        assert!(env_context.contains("OS Version:"));
        assert!(env_context.contains("Today's date:"));
    }

    #[test]
    fn test_load_claude_md_not_found() {
        // Create a config pointing to a temp directory without CLAUDE.md
        let temp_dir = std::env::temp_dir().join("test_no_claude_md");
        let _ = std::fs::create_dir_all(&temp_dir);

        let config = AgentConfig {
            cwd: Some(temp_dir.clone()),
            ..Default::default()
        };
        let runner = AgentRunner::new(config);

        // Should return None when CLAUDE.md doesn't exist
        let result = runner.load_claude_md();
        assert!(result.is_none());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_claude_md_found_in_root() {
        // Create a temp directory with CLAUDE.md
        let temp_dir = std::env::temp_dir().join("test_claude_md_root");
        let _ = std::fs::create_dir_all(&temp_dir);

        let claude_md_path = temp_dir.join("CLAUDE.md");
        let test_content = "# Project Rules\n\nThis is a test CLAUDE.md file.";
        std::fs::write(&claude_md_path, test_content).unwrap();

        let config = AgentConfig {
            cwd: Some(temp_dir.clone()),
            ..Default::default()
        };
        let runner = AgentRunner::new(config);

        // Should find and return the content
        let result = runner.load_claude_md();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), test_content);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_claude_md_found_in_dot_claude() {
        // Create a temp directory with .claude/CLAUDE.md
        let temp_dir = std::env::temp_dir().join("test_claude_md_dot_claude");
        let dot_claude_dir = temp_dir.join(".claude");
        let _ = std::fs::create_dir_all(&dot_claude_dir);

        let claude_md_path = dot_claude_dir.join("CLAUDE.md");
        let test_content = "# Rules from .claude directory\n\nAlternative location.";
        std::fs::write(&claude_md_path, test_content).unwrap();

        let config = AgentConfig {
            cwd: Some(temp_dir.clone()),
            ..Default::default()
        };
        let runner = AgentRunner::new(config);

        // Should find and return the content from .claude/CLAUDE.md
        let result = runner.load_claude_md();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), test_content);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_claude_md_root_takes_precedence() {
        // Create a temp directory with both CLAUDE.md and .claude/CLAUDE.md
        let temp_dir = std::env::temp_dir().join("test_claude_md_precedence");
        let dot_claude_dir = temp_dir.join(".claude");
        let _ = std::fs::create_dir_all(&dot_claude_dir);

        let root_content = "# Root CLAUDE.md";
        let alt_content = "# .claude/CLAUDE.md";

        std::fs::write(temp_dir.join("CLAUDE.md"), root_content).unwrap();
        std::fs::write(dot_claude_dir.join("CLAUDE.md"), alt_content).unwrap();

        let config = AgentConfig {
            cwd: Some(temp_dir.clone()),
            ..Default::default()
        };
        let runner = AgentRunner::new(config);

        // Root CLAUDE.md should take precedence
        let result = runner.load_claude_md();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), root_content);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_claude_md_found_in_parent() {
        // Create a nested directory structure with CLAUDE.md in parent
        let temp_dir = std::env::temp_dir().join("test_claude_md_parent");
        let child_dir = temp_dir.join("subdir").join("nested");
        let _ = std::fs::create_dir_all(&child_dir);

        let parent_content = "# Parent CLAUDE.md";
        std::fs::write(temp_dir.join("CLAUDE.md"), parent_content).unwrap();

        let config = AgentConfig {
            cwd: Some(child_dir.clone()),
            ..Default::default()
        };
        let runner = AgentRunner::new(config);

        // Should find CLAUDE.md in parent directory
        let result = runner.load_claude_md();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), parent_content);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_default_system_prompt_includes_claude_md() {
        // Create a temp directory with CLAUDE.md
        let temp_dir = std::env::temp_dir().join("test_prompt_claude_md");
        let _ = std::fs::create_dir_all(&temp_dir);

        let claude_md_content =
            "# My Project Rules\n\n- Rule 1: Be awesome\n- Rule 2: Stay awesome";
        std::fs::write(temp_dir.join("CLAUDE.md"), claude_md_content).unwrap();

        let config = AgentConfig {
            cwd: Some(temp_dir.clone()),
            ..Default::default()
        };
        let runner = AgentRunner::new(config);

        let system_prompt = runner.default_system_prompt();

        // Should contain the CLAUDE.md content wrapped in project-rules tags
        assert!(system_prompt.contains("<project-rules>"));
        assert!(system_prompt.contains("</project-rules>"));
        assert!(system_prompt.contains("# My Project Rules"));
        assert!(system_prompt.contains("Rule 1: Be awesome"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_default_system_prompt_without_claude_md() {
        // Create a temp directory without CLAUDE.md
        let temp_dir = std::env::temp_dir().join("test_prompt_no_claude_md");
        let _ = std::fs::create_dir_all(&temp_dir);

        let config = AgentConfig {
            cwd: Some(temp_dir.clone()),
            ..Default::default()
        };
        let runner = AgentRunner::new(config);

        let system_prompt = runner.default_system_prompt();

        // Should not contain project-rules tags
        assert!(!system_prompt.contains("<project-rules>"));
        assert!(!system_prompt.contains("</project-rules>"));

        // Should still contain the base prompt
        assert!(system_prompt.contains("You are Canal Agent"));
        assert!(system_prompt.contains("<env>"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_agent_runner_with_compactor() {
        let config = AgentConfig::default();

        // Create a custom compactor with specific settings
        let compactor = ContextCompactor::builder()
            .max_tokens(50_000)
            .threshold_ratio(0.75)
            .keep_recent(5)
            .build();

        let runner = AgentRunner::new(config).with_compactor(compactor);

        // Verify the compactor settings are applied
        let compactor = runner.compactor();
        assert_eq!(compactor.config().max_tokens, 50_000);
        assert_eq!(compactor.config().keep_recent, 5);
        // threshold_ratio of 0.75 means threshold is 75% of max_tokens
        assert_eq!(compactor.threshold_tokens(), 37_500);
    }

    #[test]
    fn test_agent_runner_compactor_getter() {
        let config = AgentConfig {
            compaction: CompactionConfig {
                enabled: true,
                max_context_tokens: 80_000,
                min_messages_to_keep: 8,
                target_tokens: 40_000,
            },
            ..Default::default()
        };

        let runner = AgentRunner::new(config);

        // The default compactor should use config values
        let compactor = runner.compactor();
        assert_eq!(compactor.config().max_tokens, 80_000);
        assert_eq!(compactor.config().keep_recent, 8);
        assert_eq!(compactor.config().target_tokens, 40_000);
    }

    #[tokio::test]
    async fn test_agent_runner_working_memory_variables() {
        let config = AgentConfig::default();
        let runner = AgentRunner::new(config);

        // Set variables
        runner
            .set_variable("test_key", serde_json::json!("test_value"))
            .await;
        runner.set_variable("count", serde_json::json!(42)).await;

        // Get variables
        let value = runner.get_variable("test_key").await;
        assert_eq!(value, Some(serde_json::json!("test_value")));

        let count = runner.get_variable("count").await;
        assert_eq!(count, Some(serde_json::json!(42)));

        // Non-existent variable
        let missing = runner.get_variable("non_existent").await;
        assert!(missing.is_none());

        // Remove variable
        let removed = runner.remove_variable("test_key").await;
        assert_eq!(removed, Some(serde_json::json!("test_value")));

        // Variable should be gone now
        let value_after_remove = runner.get_variable("test_key").await;
        assert!(value_after_remove.is_none());
    }

    #[tokio::test]
    async fn test_agent_runner_working_memory_tasks() {
        let config = AgentConfig::default();
        let runner = AgentRunner::new(config);

        // Start a task
        let task_id = runner.start_task("Test task description").await;
        assert!(!task_id.is_empty());

        // Check working memory status
        let summary = runner.working_memory_summary().await;
        assert!(summary.contains("working_memory"));
        assert!(summary.contains("Test task description"));

        // Complete the task
        runner
            .complete_task(&task_id, serde_json::json!({"result": "success"}))
            .await;

        // Verify task status updated
        let wm = runner.working_memory().read().await;
        let task = wm.task_tree.get(&task_id);
        assert!(task.is_some());
        assert_eq!(
            task.unwrap().status,
            crate::agent::memory::TaskStatus::Completed
        );
    }

    #[tokio::test]
    async fn test_agent_runner_working_memory_clear() {
        let config = AgentConfig::default();
        let runner = AgentRunner::new(config);

        // Set up some state
        runner
            .set_variable("key1", serde_json::json!("value1"))
            .await;
        runner.start_task("Task 1").await;

        // Clear working memory
        runner.clear_working_memory().await;

        // Variables should be cleared
        assert!(runner.get_variable("key1").await.is_none());

        // Task tree should be cleared
        let wm = runner.working_memory().read().await;
        assert!(wm.task_tree.nodes.is_empty());
    }

    #[test]
    fn test_agent_runner_with_working_memory() {
        let config = AgentConfig::default();

        // Create a pre-populated working memory
        let mut working_memory = WorkingMemory::new();
        working_memory.set_variable("pre_set", serde_json::json!("pre_value"));

        let wm_arc = Arc::new(RwLock::new(working_memory));
        let runner = AgentRunner::new(config).with_working_memory(wm_arc.clone());

        // Verify the working memory is shared
        assert!(Arc::ptr_eq(runner.working_memory(), &wm_arc));
    }
}
