//! Context Management for Agent
//!
//! This module provides context resolution across the six-layer hierarchy:
//! Platform, Organization, User, Session, Task, and SubAgent.
//!
//! # Six-Layer Context Hierarchy
//!
//! 1. **Platform** (L1) - Company-wide policies, global optimizations, core rules
//! 2. **Organization** (L2) - Organization-specific rules, team conventions
//! 3. **User** (L3) - User preferences, personal memory, custom instructions
//! 4. **Session** (L4) - Current session context, conversation history
//! 5. **Task** (L5) - Current task requirements, loaded skills, working memory
//! 6. **SubAgent** (L6) - SubAgent-specific instructions, isolated context
//!
//! # Submodules
//!
//! - `platform` - Platform-level context loaded from YAML configuration
//! - `organization` - Organization-level context loaded from database
//! - `user` - User-level context including preferences and CLAUDE.md
//! - `session` - Session-level context with conversation history
//! - `task` - Task-level context with working memory
//! - `subagent` - SubAgent-level context with fork modes
//! - `resolver` - Context resolution across the hierarchy
//!
//! # Example
//!
//! ```rust,ignore
//! use gateway_core::agent::context::{
//!     PlatformContextLoader, PlatformContext, ContextResolver,
//!     SessionContext, TaskContext, SubAgentContext,
//! };
//!
//! // Load platform context from configuration file
//! let loader = PlatformContextLoader::new("config/platform-rules.yaml");
//! let platform = loader.load()?;
//!
//! // Create session and task contexts
//! let session = SessionContext::new(Uuid::new_v4());
//! let task = TaskContext::new("Fix the authentication bug");
//!
//! // Resolve context across all layers
//! let mut resolver = ContextResolver::new();
//! let resolved = resolver.resolve(&[&platform, &session, &task]);
//!
//! // Use resolved context for system prompt
//! println!("{}", resolved.platform_rules);
//! ```

pub mod eval;
pub mod flags;
pub mod inspector;
pub mod integration;
pub mod observer;
pub mod platform;
pub mod prompt_generator;
pub mod relevance;
pub mod resolver;
pub mod session;
pub mod subagent;
pub mod task;
pub mod user;

// Re-export from platform module
pub use platform::{
    // Configuration types
    ContextHierarchyConfig,
    ContextLayer as PlatformContextLayer,
    IssueRecordingConfig,
    IterationConfig,
    LanguageConfig,
    LearningLoopStep,
    // Main types
    PlatformContext,
    PlatformContextLoader,
    SkillLoadingConfig,
    SystemPromptConfig,
};

// Re-export from resolver module
pub use resolver::{
    ContextLayer, ContextPriority, ContextResolver, LoadedSkill, PermissionMode, ResolvedContext,
};

// Re-export from user module
// Note: Using UserCtx alias to avoid conflict with deprecated UserContext in memory module
pub use user::{
    CodingStyle, CommunicationPrefs, MemoryItem as UserMemoryItem, UserContext as UserCtx,
    UserContextLoader, UserPreferences,
};

// Re-export from session module
pub use session::{FileState, SessionContext, SessionContextLoader, ToolState as SessionToolState};

// Re-export from task module
pub use task::{
    Discovery, TaskConstraints, TaskContext, TaskContextBuilder, Verification as TaskVerification,
    VerificationStatus as TaskVerificationStatus, VerificationType, WorkingMemory,
};

// Re-export from subagent module
pub use subagent::{ContextForkMode, ForkedContext, SubAgentContext, SubAgentContextBuilder};

// Re-export from prompt_generator module
pub use prompt_generator::{PromptBuilder, PromptConfig, PromptSection, SystemPromptGenerator};

// Re-export from flags module
pub use flags::ContextResolverFlags;

// Re-export from inspector module
pub use inspector::{PromptInspection, SectionInfo};

// Re-export from observer module
pub use observer::{AgentObserver, CompositeAgentObserver, JsonlAgentObserver, NoOpAgentObserver};

// Re-export from relevance module
pub use relevance::{ItemSource, RelevanceScorer, RelevanceScorerConfig, Scorable, ScoredItem};

// Re-export from eval module
pub use eval::{AssertionResult, EvalAssertion, EvalCase, EvalReport, EvalResult, PromptEvaluator};

// Re-export from integration module
pub use integration::ContextIntegration;
