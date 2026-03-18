//! Agent Orchestrator module
//!
//! Handles intent recognition, task planning, and step execution.
//! Also provides Claude Agent SDK compatible types for protocol interop.
//!
//! # Module Structure
//!
//! - `config` - CLAUDE.md configuration parser with inheritance support
//! - `types` - Protocol types (Messages, ContentBlocks, Permissions, Hooks, Memory)
//! - `hooks` - Hook system (executor, matcher, shell runner)
//! - `tools` - Built-in tools (Read, Write, Edit, Bash, Glob, Grep, Task)
//! - `loop` - Agent loop (config, state, runner)
//! - `session` - Session management (manager, storage, compactor)
//! - `memory` - User memory storage (file store, in-memory store)
//! - `factory` - Agent factory for creating configured runners
//!
//! # CLAUDE.md Support
//!
//! The agent module supports initialization from CLAUDE.md configuration files,
//! which define agent behavior using YAML frontmatter and markdown content.
//!
//! ## Example CLAUDE.md
//!
//! ```markdown
//! ---
//! name: my-agent
//! model: claude-sonnet-4-6
//! extends: base-agent
//! tools:
//!   allowed: [Read, Write, Edit, Glob, Grep, Bash]
//!   blocked: [NotebookEdit]
//! permissions:
//!   mode: accept_edits
//!   allowed_directories: [/home/user/projects]
//! ---
//!
//! # Project Instructions
//!
//! ## Rules
//! - Use TypeScript for all code
//! - Follow existing patterns
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use gateway_core::agent::{AgentFactory, ClaudeConfigBuilder};
//!
//! // Load from file
//! let factory = AgentFactory::new(llm_router);
//! let agent = factory.create_from_claude_md("./CLAUDE.md")?;
//!
//! // Load from directory hierarchy
//! let agent = factory.create_from_hierarchy("/path/to/project")?;
//!
//! // Parse from string
//! let config = ClaudeConfigBuilder::new()
//!     .parse(claude_md_content)?
//!     .build()?;
//! ```

// Original modules
pub mod config;
pub mod context;
pub mod executor;
pub mod factory;
pub mod hybrid;
pub mod intent;
pub mod llm_adapter;
pub mod mode_selector;
pub mod planner;

// Five-Layer Automation Architecture
pub mod automation;

// Orchestrator-Worker and Code Orchestration modules
pub mod code_orchestration;
pub mod worker;

// Claude Agent SDK compatible modules
pub mod hooks;
pub mod iteration;
pub mod r#loop;
pub mod memory;
pub mod roles;
// pub mod enhanced_loop; // Disabled - pending full implementation
#[cfg(feature = "devtools")]
pub mod devtools_bridge;
pub mod session;
pub mod skills;
#[cfg(feature = "collaboration")]
pub mod step_delegate;
#[cfg(feature = "collaboration")]
pub mod task_classifier;
pub mod tools;
pub mod types;

// Original exports
pub use executor::{ExecutionResult, StepExecutor, StepResult};
pub use intent::{Intent, IntentRecognizer, TaskType};
pub use planner::{PlanStep, StepAction, TaskPlan, TaskPlanner};

// Re-export SDK compatible types
pub use types::{
    // Messages
    AgentMessage,
    AssistantMessage,
    AssistantMessageError,
    // Permissions - Checker system
    CompositePermissionChecker,
    // Content blocks
    ContentBlock,
    DangerousCommandChecker,
    DefaultPermissionChecker,
    DocumentSource,
    // Hooks
    ErrorHookData,
    // Memory
    ExtractedMemory,
    HookContext,
    HookDefinition,
    HookEvent,
    HookResult,
    ImageSource,
    MemoryCategory,
    MemoryConfidence,
    MemoryEntry,
    MemoryExtractionRequest,
    MemoryExtractionResponse,
    MemoryLoadedHookData,
    MemorySource,
    MemoryUpdateEvent,
    MemoryUpdateHookData,
    MessageContent,
    PathSecurityChecker,
    // Permissions - Core types
    PendingPermission,
    PendingPermissionState,
    PermissionBehavior,
    PermissionChecker,
    PermissionContext,
    PermissionDestination,
    PermissionHook,
    PermissionManager,
    PermissionMode,
    PermissionOption,
    PermissionRequest,
    PermissionResponse,
    PermissionResult,
    PermissionRule,
    PermissionSuggestion,
    PermissionUpdate,
    PostMessageHookData,
    PostToolUseHookData,
    PreMessageHookData,
    PreToolUseHookData,
    ResultMessage,
    ResultSubtype,
    SessionEndHookData,
    SessionStartHookData,
    StreamEventMessage,
    StreamEventSubtype,
    SubagentCompleteHookData,
    SubagentSpawnHookData,
    SystemMessage,
    ToolResultBlock,
    ToolResultContent,
    Usage,
    UserMemory,
    UserMessage,
};

// Re-export hook system
pub use hooks::{
    HookCallback, HookExecutor, HookMatcher, HookOutput, IterationConfig, IterationHook,
    RegisteredHook, ShellHookRunner,
};

// Re-export tools
pub use tools::{
    AgentTool,
    AgentTypeInfo,
    BashInput,
    BashOutput,
    BashTool,
    DynamicTool,
    EditInput,
    EditOutput,
    EditTool,
    GlobInput,
    GlobOutput,
    GlobTool,
    GrepInput,
    GrepOutput,
    GrepTool,
    PlaceholderAgentFactory,
    PlatformToolConfig,
    ReadInput,
    ReadOutput,
    ReadTool,
    RealAgentFactory,
    Subagent,
    SubagentConfig,
    SubagentResult,
    // Task/Subagent related types
    TaskAgentFactory,
    TaskInput,
    TaskOutput,
    TaskTool,
    ToolContext,
    ToolError,
    ToolMetadata,
    ToolRegistry,
    ToolRegistryBuilder,
    ToolResult,
    ToolWrapper,
    // Skill iteration tools
    UpdateSkillIssueInput,
    UpdateSkillIssueOutput,
    UpdateSkillIssueTool,
    UpdateSkillStatsInput,
    UpdateSkillStatsOutput,
    UpdateSkillStatsTool,
    WriteInput,
    WriteOutput,
    WriteTool,
};

// Re-export loop
pub use r#loop::{
    AgentConfig, AgentError, AgentLoop, AgentRunner, AgentState, SubagentSystemConfig,
};

// Re-export session
pub use session::{
    // Checkpoint types
    AutoCheckpointConfig,
    AutoCheckpointTrigger,
    Checkpoint,
    CheckpointError,
    CheckpointManager,
    CheckpointMetadata,
    CheckpointTrigger,
    // Compaction types
    CompactConfig,
    CompactTrigger,
    CompactableSession,
    // Session management
    CompactingSessionManager,
    CompactionError,
    CompactionResult,
    CompactionStats,
    ContextCompactor,
    ContextCompactorBuilder,
    ContextState,
    ContextStats,
    DangerousOperation,
    DefaultSessionManager,
    FileCheckpointManager,
    FileSessionStorage,
    LlmSummarizer,
    MemoryCheckpointManager,
    MemorySessionStorage,
    RestoreResult,
    Session,
    SessionError,
    SessionManager,
    SessionMetadata,
    SessionSnapshot,
    SessionStorage,
    Summarizer,
    TokenEstimationStrategy,
};

// Re-export memory
pub use memory::{
    CompressionResult,
    ContextManager,
    ContextMemory,
    FileMemoryStore,
    InMemoryStore,
    MemoryError,
    MemoryStore,
    SessionMemory,
    SessionMessage,
    TaskNode,
    TaskResult,
    TaskStatus,
    TaskTree,
    TeamMemory,
    ToolCallRecord,
    ToolState,
    ToolStateValue,
    Verification,
    VerificationStatus,
    // Hierarchical context memory types
    WorkingMemory,
};

// Re-export factory
pub use factory::{AgentFactory, AgentFactoryBuilder};

// Re-export LLM adapter
pub use llm_adapter::LlmRouterAdapter;

// Re-export skills
pub use skills::{
    get_builtin_skills, BuiltinSkill, Skill, SkillExecutionResult, SkillExecutor,
    SkillExecutorBuilder, SkillMetadata, SkillParseError, SkillParser, SkillRegistry,
    SkillRegistryBuilder,
};

// Re-export config
pub use config::{
    discover_configs, merge_discovered_configs, AgentDef, ClaudeConfig, ClaudeConfigBuilder,
    ClaudeConfigError, ClaudeConfigLoader, ClaudeConfigResult, ClaudeFrontmatter, CompactionDef,
    McpServerDef, PermissionsConfig, ToolsConfig,
};

// Re-export mode selector
pub use mode_selector::{
    AlternativeMode,
    AutoModeSelector,
    AutoModeSelectorBuilder,
    AutoModeSelectorConfig,
    CapabilityCheckResult,
    CapabilityChecker,
    ComplexityThresholds,
    CostBreakdown,
    // Cost estimation
    CostEstimate,
    CostEstimator,
    DecisionFactor,
    // Decision logging
    DecisionLogEntry,
    DecisionLogger,
    DecisionStatistics,
    DefaultCapabilityChecker,
    DefaultCostEstimator,
    DefaultTaskAnalyzer,
    // Core types
    ExecutionMode,
    ExecutionOutcome,
    // Capability checking
    ExecutorCapabilities,
    InMemoryDecisionLogger,
    // Mode selector
    ModeSelectionRequest,
    ModeSelectionResult,
    ModeSelector,
    OptimizationGoal,
    PricingConfig,
    // Task analysis
    ResourceRequirements,
    TaskAnalysis,
    TaskAnalyzer,
    TaskCategory,
    TaskComplexity,
    TaskInfo,
    // User preferences
    UserPreferences,
};

// Re-export worker types
pub use worker::{
    OrchestratedResult, OrchestratorConfig, WorkerManager, WorkerResult, WorkerSpec,
    WorkerSpecJson, WorkerStatus, WorkerUsage,
};

// Re-export code orchestration types
pub use code_orchestration::{
    CodeOrchestrationConfig, CodeOrchestrationRequest, CodeOrchestrationResult,
    CodeOrchestrationRuntime, ToolCallRecord as CodeToolCallRecord, ToolCodeGenerator,
    ToolProxyBridge,
};

// Re-export hybrid MCP/CodeAct router
pub use hybrid::{
    ArtifactInfo,
    ExecutionRequest,
    ExecutionResult as HybridExecutionResult,
    // Error types
    HybridError,
    HybridErrorInfo,
    HybridErrorType,
    // Executor trait and status
    HybridExecutor,
    HybridExecutorStatus,
    // Metrics
    HybridMetrics,
    // Router
    HybridRouter,
    HybridRouterBuilder,
    HybridRouterConfig,
    // Core types
    ToolType,
    // Tool type detection
    ToolTypeDetector,
};

// Re-export role-based permissions
pub use roles::{
    AgentRole, RoleBasedAgentConfig, ToolCategory, ToolDefinition, ToolOverrides,
    ToolOverridesConfig, ToolPermissionManager, ToolValidationResult,
};

// Re-export Five-Layer Automation Architecture
pub use automation::{
    ActionSchema,
    ApiInfo,
    AssetQuery,
    AssetStats,
    // Asset Store (Layer 5)
    AssetStore,
    AssetStoreBuilder,
    AssetStoreConfig,
    AutomationPath,
    AutomationRequest,
    AutomationResult,
    // Orchestrator
    BrowserAutomationOrchestrator,
    BrowserAutomationOrchestratorBuilder,
    // Code Generator (Layer 3)
    CodeGenerator,
    CodeGeneratorBuilder,
    Coordinates,
    // CV Explorer (Layer 2)
    CvExplorer,
    CvExplorerBuilder,
    ElementSchema,
    ElementType,
    ExecutionOptions,
    ExecutionStats,
    ExplorationOptions,
    ExplorationResult,
    FileAssetStore,
    GeneratedScript,
    GenerationOptions,
    GenerationResult,
    IntentAnalysis,
    // Intent Router (Layer 1)
    IntentRouter,
    IntentRouterBuilder,
    MemoryAssetStore,
    MetricsSnapshot,
    OrchestratorConfig as AutomationOrchestratorConfig,
    OrchestratorMetrics,
    OrchestratorStatus,
    // Core types
    PageSchema,
    PathDecision,
    RouteAnalysis,
    ScriptAsset,
    // Script Executor (Layer 4)
    ScriptExecutor,
    ScriptExecutorBuilder,
    ScriptMetadata,
    ScriptType,
    TargetSystem,
};

// Re-export iteration learning system
pub use iteration::{ExecutionLog, ExecutionTracker, LearnedIssue, SkillUpdater, ToolExecution};

// Re-export context resolver and platform context
pub use context::{
    AgentObserver,
    CompositeAgentObserver,
    ContextForkMode,
    ContextHierarchyConfig,
    // Integration and prompt generation
    ContextIntegration,
    // Resolver types
    ContextLayer,
    ContextPriority,
    ContextResolver,
    // Context engineering v2 flags
    ContextResolverFlags,
    FileState,
    ForkedContext,
    IssueRecordingConfig,
    // Context engineering v2 relevance scoring
    ItemSource,
    IterationConfig as PlatformIterationConfig,
    JsonlAgentObserver,
    LanguageConfig,
    LearningLoopStep,
    LoadedSkill,
    NoOpAgentObserver,
    // Organization and User contexts
    OrganizationContext,
    OrganizationContextLoader,
    PermissionMode as ContextPermissionMode,
    // Platform context types
    PlatformContext,
    PlatformContextLayer,
    PlatformContextLoader,
    PromptBuilder,
    PromptConfig,
    // Context engineering v2 inspector and observer
    PromptInspection,
    PromptSection,
    RelevanceScorer,
    RelevanceScorerConfig,
    ResolvedContext,
    Scorable,
    ScoredItem,
    SectionInfo,
    // Session, Task, SubAgent contexts
    SessionContext,
    SkillLoadingConfig,
    SubAgentContext,
    SubAgentContextBuilder,
    SystemPromptConfig,
    SystemPromptGenerator,
    TaskContext,
    TaskContextBuilder,
    UserContextLoader,
    UserCtx,
};
