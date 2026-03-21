//! Agent Configuration

use crate::agent::types::PermissionMode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Model to use (e.g., "claude-sonnet-4-6")
    #[serde(default)]
    pub model: Option<String>,

    /// Maximum turns before stopping
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,

    /// Maximum budget in USD
    #[serde(default)]
    pub max_budget_usd: Option<f64>,

    /// Permission mode
    #[serde(default)]
    pub permission_mode: PermissionMode,

    /// Allowed tools
    #[serde(default)]
    pub tools: Vec<String>,

    /// Blocked tools
    #[serde(default)]
    pub blocked_tools: Vec<String>,

    /// System prompt override
    #[serde(default)]
    pub system_prompt: Option<String>,

    /// System prompt additions
    #[serde(default)]
    pub system_prompt_additions: Vec<String>,

    /// Working directory
    #[serde(default)]
    pub cwd: Option<PathBuf>,

    /// Allowed directories
    #[serde(default)]
    pub allowed_directories: Vec<PathBuf>,

    /// MCP server configurations
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,

    /// Custom agent definitions
    #[serde(default)]
    pub agents: HashMap<String, AgentDefinition>,

    /// Enable extended thinking
    #[serde(default)]
    pub enable_thinking: bool,

    /// API timeout in seconds
    #[serde(default = "default_api_timeout")]
    pub api_timeout_secs: u64,

    /// Tool execution timeout in seconds
    #[serde(default = "default_tool_timeout")]
    pub tool_timeout_secs: u64,

    /// Context compaction configuration
    #[serde(default)]
    pub compaction: CompactionConfig,

    /// Orchestrator-Worker configuration
    #[serde(default)]
    pub orchestrator_config: Option<crate::agent::worker::OrchestratorConfig>,

    /// Whether code orchestration is enabled
    #[serde(default)]
    pub code_orchestration_enabled: bool,

    /// Subagent system configuration (Claude-style Single Agent + Subagent)
    #[serde(default)]
    pub subagent_config: SubagentSystemConfig,

    /// Maximum tokens for conversation history (token-based windowing)
    ///
    /// Instead of using a fixed message count, this budget allows smart
    /// windowing based on actual content size. Messages are included
    /// newest-first until the budget is reached.
    #[serde(default = "default_history_token_budget")]
    pub history_token_budget: usize,

    /// Enable dynamic tool filtering based on task context
    ///
    /// When enabled, only relevant tools are sent to the LLM:
    /// - Browser tools only for browser-related tasks
    /// - Orchestrate tool only when workers are enabled
    /// - This can save 1,000-3,500 tokens per request
    #[serde(default)]
    pub enable_tool_filtering: bool,
}

/// Context compaction configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Whether compaction is enabled
    #[serde(default = "default_compaction_enabled")]
    pub enabled: bool,

    /// Maximum context tokens before compaction is triggered
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,

    /// Minimum messages to keep after compaction
    #[serde(default = "default_min_messages_to_keep")]
    pub min_messages_to_keep: usize,

    /// Target token count after compaction
    #[serde(default = "default_target_tokens")]
    pub target_tokens: usize,
}

/// Subagent configuration for Claude-style Single Agent + Subagent architecture
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSystemConfig {
    /// Model for the Lead Agent (default: opus)
    #[serde(default = "default_lead_model")]
    pub lead_model: String,

    /// Default model for Subagents (default: sonnet)
    #[serde(default = "default_subagent_model")]
    pub default_subagent_model: String,

    /// Maximum parallel subagents (default: 3)
    #[serde(default = "default_max_parallel_subagents")]
    pub max_parallel_subagents: usize,

    /// Context window size for subagents in tokens (default: 150000)
    #[serde(default = "default_subagent_context_tokens")]
    pub subagent_context_tokens: usize,

    /// Default max turns for Explore subagent
    #[serde(default = "default_explore_max_turns")]
    pub explore_max_turns: u32,

    /// Default max turns for Plan subagent
    #[serde(default = "default_plan_max_turns")]
    pub plan_max_turns: u32,

    /// Default max turns for Code subagent
    #[serde(default = "default_code_max_turns")]
    pub code_max_turns: u32,

    /// Default max turns for Bash subagent
    #[serde(default = "default_bash_max_turns")]
    pub bash_max_turns: u32,

    /// Whether to enable result compression when returning to lead agent
    #[serde(default = "default_result_compression")]
    pub enable_result_compression: bool,

    /// Maximum result tokens to return to lead agent (if compression enabled)
    #[serde(default = "default_max_result_tokens")]
    pub max_result_tokens: usize,
}

fn default_lead_model() -> String {
    "claude-opus-4-6".to_string()
}

fn default_subagent_model() -> String {
    "claude-sonnet-4-6".to_string()
}

fn default_max_parallel_subagents() -> usize {
    3
}

fn default_subagent_context_tokens() -> usize {
    150000 // 150K tokens
}

fn default_explore_max_turns() -> u32 {
    20
}

fn default_plan_max_turns() -> u32 {
    30
}

fn default_code_max_turns() -> u32 {
    50
}

fn default_bash_max_turns() -> u32 {
    10
}

fn default_result_compression() -> bool {
    true
}

fn default_max_result_tokens() -> usize {
    4000 // Compress results to ~4K tokens
}

impl Default for SubagentSystemConfig {
    fn default() -> Self {
        Self {
            lead_model: default_lead_model(),
            default_subagent_model: default_subagent_model(),
            max_parallel_subagents: default_max_parallel_subagents(),
            subagent_context_tokens: default_subagent_context_tokens(),
            explore_max_turns: default_explore_max_turns(),
            plan_max_turns: default_plan_max_turns(),
            code_max_turns: default_code_max_turns(),
            bash_max_turns: default_bash_max_turns(),
            enable_result_compression: default_result_compression(),
            max_result_tokens: default_max_result_tokens(),
        }
    }
}

impl SubagentSystemConfig {
    /// Create a new subagent config with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the lead model
    pub fn lead_model(mut self, model: impl Into<String>) -> Self {
        self.lead_model = model.into();
        self
    }

    /// Set the default subagent model
    pub fn default_subagent_model(mut self, model: impl Into<String>) -> Self {
        self.default_subagent_model = model.into();
        self
    }

    /// Set max parallel subagents
    pub fn max_parallel_subagents(mut self, count: usize) -> Self {
        self.max_parallel_subagents = count;
        self
    }

    /// Set subagent context window size
    pub fn subagent_context_tokens(mut self, tokens: usize) -> Self {
        self.subagent_context_tokens = tokens;
        self
    }

    /// Get default max turns for a subagent type
    pub fn max_turns_for_type(&self, subagent_type: &str) -> u32 {
        match subagent_type {
            "Explore" => self.explore_max_turns,
            "Plan" => self.plan_max_turns,
            "Code" => self.code_max_turns,
            "Bash" => self.bash_max_turns,
            _ => 20, // Default
        }
    }

    /// Resolve model shorthand to full model ID
    pub fn resolve_model(&self, model: &str) -> String {
        match model.to_lowercase().as_str() {
            "sonnet" => "claude-sonnet-4-6".to_string(),
            "opus" => "claude-opus-4-6".to_string(),
            "haiku" => "claude-haiku-3-20250514".to_string(),
            "inherit" => self.default_subagent_model.clone(),
            _ => model.to_string(),
        }
    }
}

fn default_compaction_enabled() -> bool {
    true
}

fn default_max_context_tokens() -> usize {
    // Most models support 128k-256k+. Use 150k to maximize context utilization
    // while staying safe for smaller models (qwen-turbo 32k will be capped separately)
    150000
}

fn default_min_messages_to_keep() -> usize {
    5 // Keep fewer messages to ensure compaction is aggressive enough
}

fn default_target_tokens() -> usize {
    75000 // Target ~75k tokens after compaction (half of 150k max)
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: default_compaction_enabled(),
            max_context_tokens: default_max_context_tokens(),
            min_messages_to_keep: default_min_messages_to_keep(),
            target_tokens: default_target_tokens(),
        }
    }
}

impl CompactionConfig {
    /// Create a new compaction config
    pub fn new() -> Self {
        Self::default()
    }

    /// Disable compaction
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Set max context tokens
    pub fn max_context_tokens(mut self, tokens: usize) -> Self {
        self.max_context_tokens = tokens;
        self
    }

    /// Set min messages to keep
    pub fn min_messages_to_keep(mut self, count: usize) -> Self {
        self.min_messages_to_keep = count;
        self
    }

    /// Set target tokens
    pub fn target_tokens(mut self, tokens: usize) -> Self {
        self.target_tokens = tokens;
        self
    }
}

fn default_max_turns() -> u32 {
    100
}

fn default_history_token_budget() -> usize {
    50000 // 50K tokens for conversation history
}

fn default_api_timeout() -> u64 {
    120
}

fn default_tool_timeout() -> u64 {
    300
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: None,
            max_turns: default_max_turns(),
            max_budget_usd: None,
            permission_mode: PermissionMode::BypassPermissions,
            tools: Vec::new(),
            blocked_tools: Vec::new(),
            system_prompt: None,
            system_prompt_additions: Vec::new(),
            cwd: None,
            allowed_directories: Vec::new(),
            mcp_servers: HashMap::new(),
            agents: HashMap::new(),
            enable_thinking: false,
            api_timeout_secs: default_api_timeout(),
            tool_timeout_secs: default_tool_timeout(),
            compaction: CompactionConfig::default(),
            orchestrator_config: None,
            code_orchestration_enabled: false,
            subagent_config: SubagentSystemConfig::default(),
            history_token_budget: default_history_token_budget(),
            enable_tool_filtering: false,
        }
    }
}

impl AgentConfig {
    /// Create a new config with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the model
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set max turns
    pub fn max_turns(mut self, turns: u32) -> Self {
        self.max_turns = turns;
        self
    }

    /// Set max budget
    pub fn max_budget_usd(mut self, budget: f64) -> Self {
        self.max_budget_usd = Some(budget);
        self
    }

    /// Set permission mode
    pub fn permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = mode;
        self
    }

    /// Add allowed tool
    pub fn allow_tool(mut self, tool: impl Into<String>) -> Self {
        self.tools.push(tool.into());
        self
    }

    /// Add blocked tool
    pub fn block_tool(mut self, tool: impl Into<String>) -> Self {
        self.blocked_tools.push(tool.into());
        self
    }

    /// Set working directory
    pub fn cwd(mut self, path: impl Into<PathBuf>) -> Self {
        self.cwd = Some(path.into());
        self
    }

    /// Add allowed directory
    pub fn allow_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.allowed_directories.push(path.into());
        self
    }

    /// Enable thinking
    pub fn enable_thinking(mut self) -> Self {
        self.enable_thinking = true;
        self
    }

    /// Set compaction config
    pub fn compaction(mut self, config: CompactionConfig) -> Self {
        self.compaction = config;
        self
    }

    /// Disable compaction
    pub fn disable_compaction(mut self) -> Self {
        self.compaction.enabled = false;
        self
    }

    /// Set orchestrator config
    pub fn orchestrator_config(mut self, config: crate::agent::worker::OrchestratorConfig) -> Self {
        self.orchestrator_config = Some(config);
        self
    }

    /// Enable code orchestration
    pub fn enable_code_orchestration(mut self) -> Self {
        self.code_orchestration_enabled = true;
        self
    }

    /// Set subagent configuration
    pub fn subagent_config(mut self, config: SubagentSystemConfig) -> Self {
        self.subagent_config = config;
        self
    }

    /// Set history token budget for conversation windowing
    pub fn history_token_budget(mut self, tokens: usize) -> Self {
        self.history_token_budget = tokens;
        self
    }

    /// Enable dynamic tool filtering
    pub fn enable_tool_filtering(mut self) -> Self {
        self.enable_tool_filtering = true;
        self
    }

    /// Check if a tool is allowed
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        // Check blocked list first
        if self
            .blocked_tools
            .iter()
            .any(|t| tool_matches_pattern(t, tool_name))
        {
            return false;
        }

        // If tools list is empty, all tools are allowed
        if self.tools.is_empty() {
            return true;
        }

        // Check allowed list
        self.tools
            .iter()
            .any(|t| tool_matches_pattern(t, tool_name))
    }
}

/// Check if a tool name matches a pattern (supports wildcards)
fn tool_matches_pattern(pattern: &str, tool_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if pattern.ends_with('*') {
        let prefix = &pattern[..pattern.len() - 1];
        return tool_name.starts_with(prefix);
    }

    pattern == tool_name
}

/// MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Command to start the server
    pub command: String,
    /// Arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Working directory
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Custom agent definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Agent description
    pub description: String,
    /// System prompt for this agent
    pub prompt: String,
    /// Tools available to this agent
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Model to use
    #[serde(default)]
    pub model: Option<AgentModel>,
}

/// Agent model selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentModel {
    Sonnet,
    Opus,
    Haiku,
    /// Inherit from parent
    Inherit,
}

impl AgentModel {
    /// Convert to model ID
    pub fn to_model_id(&self, default: &str) -> String {
        match self {
            Self::Sonnet => "claude-sonnet-4-6".to_string(),
            Self::Opus => "claude-opus-4-6".to_string(),
            Self::Haiku => "claude-haiku-3-20250514".to_string(),
            Self::Inherit => default.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config_builder() {
        let config = AgentConfig::new()
            .model("claude-sonnet-4-6")
            .max_turns(50)
            .permission_mode(PermissionMode::AcceptEdits)
            .allow_tool("Bash*")
            .cwd("/tmp");

        assert_eq!(config.max_turns, 50);
        assert_eq!(config.permission_mode, PermissionMode::AcceptEdits);
        assert!(config.is_tool_allowed("BashTool"));
    }

    #[test]
    fn test_tool_allowed() {
        let config = AgentConfig::new()
            .allow_tool("Bash*")
            .allow_tool("Read")
            .block_tool("BashDangerous");

        assert!(config.is_tool_allowed("BashTool"));
        assert!(config.is_tool_allowed("Read"));
        assert!(!config.is_tool_allowed("BashDangerous"));
        assert!(!config.is_tool_allowed("Write"));
    }

    #[test]
    fn test_tool_matches_pattern() {
        assert!(tool_matches_pattern("*", "anything"));
        assert!(tool_matches_pattern("Bash*", "BashTool"));
        assert!(tool_matches_pattern("Read", "Read"));
        assert!(!tool_matches_pattern("Read", "Write"));
    }
}
