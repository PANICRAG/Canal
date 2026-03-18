//! Agent Tools - Claude Agent SDK Compatible Built-in Tools
//!
//! Provides standardized tool definitions compatible with Claude Agent SDK.

pub mod bash;
#[cfg(unix)]
pub mod browser;
pub mod claude_code;
pub mod code_orchestration_tool;
pub mod computer;
pub mod computer_vision;
pub mod context;
#[cfg(feature = "database")]
pub mod database;
pub mod devtools;
pub mod file_ops;
pub mod git;
pub mod hosting;
pub mod invoke_skill;
pub mod orchestrate;
pub mod platform;
pub mod registry;
pub mod search;
pub mod search_tools;
pub mod skill_issue;
pub mod task;
pub mod traits;

pub use bash::{BashInput, BashOutput, BashTool};
#[cfg(unix)]
pub use browser::{BrowserInput, BrowserOutput, BrowserTool};
pub use claude_code::{ClaudeCodeInput, ClaudeCodeOutput, ClaudeCodeTool};
pub use code_orchestration_tool::{
    CodeOrchestrationInput, CodeOrchestrationOutput, CodeOrchestrationTool,
};
pub use computer::{
    ComputerInput, ComputerOutput, ComputerTool, LocalComputerTool, UnifiedComputerTool,
};
pub use context::ToolContext;
pub use devtools::DevtoolsToolConfig;
pub use file_ops::{
    EditInput, EditOutput, EditTool, ReadInput, ReadOutput, ReadTool, WriteInput, WriteOutput,
    WriteTool,
};
pub use git::{
    GitBranchInput, GitBranchOutput, GitBranchTool, GitCommitInput, GitCommitOutput, GitCommitTool,
    GitDiffInput, GitDiffOutput, GitDiffTool, GitLogInput, GitLogOutput, GitLogTool,
    GitStatusInput, GitStatusOutput, GitStatusTool,
};
pub use hosting::{HostingToolConfig, TokenProvider};
#[cfg(feature = "database")]
pub use database::DatabaseToolConfig;
pub use invoke_skill::{InvokeSkillInput, InvokeSkillOutput, InvokeSkillTool};
pub use orchestrate::{OrchestrateInput, OrchestrateOutput, OrchestrateTool};
pub use platform::PlatformToolConfig;
pub use registry::{ToolFilterContext, ToolRegistry, ToolRegistryBuilder};
pub use search::{GlobInput, GlobOutput, GlobTool, GrepInput, GrepOutput, GrepTool};
pub use search_tools::{
    SearchToolsInput, SearchToolsOutput, SearchToolsTool, SearchableToolCatalog,
};
pub use skill_issue::{
    UpdateSkillIssueInput, UpdateSkillIssueOutput, UpdateSkillIssueTool, UpdateSkillStatsInput,
    UpdateSkillStatsOutput, UpdateSkillStatsTool,
};
pub use task::{
    AgentFactory as TaskAgentFactory, AgentTypeInfo, PlaceholderAgentFactory, RealAgentFactory,
    Subagent, SubagentConfig, SubagentResult, TaskInput, TaskOutput, TaskTool,
};
pub use traits::{AgentTool, DynamicTool, ToolError, ToolMetadata, ToolResult, ToolWrapper};

// CV Tools (CP27.2)
pub use computer_vision::{register_cv_tools, CV_TOOL_NAMES};

// Re-export screen tools from screen module (replaces legacy browser Computer Use tools)
pub use crate::screen::{register_screen_tools, SCREEN_TOOL_NAMES};
