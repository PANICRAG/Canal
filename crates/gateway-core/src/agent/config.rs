//! CLAUDE.md Configuration Parser
//!
//! Supports loading agent configuration from CLAUDE.md files with YAML frontmatter
//! and markdown content for system prompts.
//!
//! # Format Example
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
//! mcp_servers:
//!   filesystem:
//!     command: mcp-server-filesystem
//!     args: [/home/user]
//! ---
//!
//! # Project Instructions
//!
//! ## Rules
//! - Use TypeScript for all code
//! - Follow existing patterns in the codebase
//!
//! ## Context
//! This is a web application project using React and TypeScript.
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

use super::r#loop::config::{AgentConfig, AgentDefinition, AgentModel, McpServerConfig};
use super::types::PermissionMode;

/// Errors that can occur during CLAUDE.md parsing
#[derive(Error, Debug)]
pub enum ClaudeConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    YamlParse(String),

    #[error("Invalid frontmatter: {0}")]
    InvalidFrontmatter(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Circular inheritance detected: {0}")]
    CircularInheritance(String),

    #[error("Parent config not found: {0}")]
    ParentNotFound(String),

    #[error("Validation error: {0}")]
    Validation(String),
}

/// Result type for CLAUDE.md operations
pub type ClaudeConfigResult<T> = Result<T, ClaudeConfigError>;

/// Raw CLAUDE.md frontmatter structure
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeFrontmatter {
    /// Agent name/identifier
    #[serde(default)]
    pub name: Option<String>,

    /// Agent description
    #[serde(default)]
    pub description: Option<String>,

    /// Model to use
    #[serde(default)]
    pub model: Option<String>,

    /// Parent configuration to extend
    #[serde(default)]
    pub extends: Option<String>,

    /// Tool configuration
    #[serde(default)]
    pub tools: Option<ToolsConfig>,

    /// Permission configuration
    #[serde(default)]
    pub permissions: Option<PermissionsConfig>,

    /// MCP server configurations
    #[serde(default)]
    pub mcp_servers: Option<HashMap<String, McpServerDef>>,

    /// Custom agents (sub-agents)
    #[serde(default)]
    pub agents: Option<HashMap<String, AgentDef>>,

    /// Maximum turns
    #[serde(default)]
    pub max_turns: Option<u32>,

    /// Maximum budget in USD
    #[serde(default)]
    pub max_budget_usd: Option<f64>,

    /// Enable extended thinking
    #[serde(default)]
    pub enable_thinking: Option<bool>,

    /// API timeout in seconds
    #[serde(default)]
    pub api_timeout_secs: Option<u64>,

    /// Tool timeout in seconds
    #[serde(default)]
    pub tool_timeout_secs: Option<u64>,

    /// Context compaction settings
    #[serde(default)]
    pub compaction: Option<CompactionDef>,

    /// Custom metadata
    #[serde(default)]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Tool access configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsConfig {
    /// Allowed tools (glob patterns supported)
    #[serde(default)]
    pub allowed: Vec<String>,

    /// Blocked tools (glob patterns supported)
    #[serde(default)]
    pub blocked: Vec<String>,

    /// Tool-specific configurations
    #[serde(default)]
    pub config: Option<HashMap<String, serde_json::Value>>,
}

/// Permission configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionsConfig {
    /// Permission mode
    #[serde(default)]
    pub mode: Option<PermissionModeStr>,

    /// Allowed directories for file operations
    #[serde(default)]
    pub allowed_directories: Vec<PathBuf>,

    /// Blocked directories
    #[serde(default)]
    pub blocked_directories: Vec<PathBuf>,

    /// Command allow patterns
    #[serde(default)]
    pub allowed_commands: Vec<String>,

    /// Command block patterns
    #[serde(default)]
    pub blocked_commands: Vec<String>,
}

/// Permission mode as string for YAML
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionModeStr {
    Default,
    AcceptEdits,
    Plan,
    BypassPermissions,
}

impl From<PermissionModeStr> for PermissionMode {
    fn from(mode: PermissionModeStr) -> Self {
        match mode {
            PermissionModeStr::Default => PermissionMode::Default,
            PermissionModeStr::AcceptEdits => PermissionMode::AcceptEdits,
            PermissionModeStr::Plan => PermissionMode::Plan,
            PermissionModeStr::BypassPermissions => PermissionMode::BypassPermissions,
        }
    }
}

impl From<PermissionMode> for PermissionModeStr {
    fn from(mode: PermissionMode) -> Self {
        match mode {
            PermissionMode::Default => PermissionModeStr::Default,
            PermissionMode::AcceptEdits => PermissionModeStr::AcceptEdits,
            PermissionMode::Plan => PermissionModeStr::Plan,
            PermissionMode::BypassPermissions => PermissionModeStr::BypassPermissions,
        }
    }
}

/// MCP server definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerDef {
    /// Command to start the server
    pub command: String,
    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Working directory
    #[serde(default)]
    pub cwd: Option<String>,
    /// Whether to auto-start
    #[serde(default = "default_true")]
    pub auto_start: bool,
}

fn default_true() -> bool {
    true
}

/// Sub-agent definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    /// Agent description
    pub description: String,
    /// System prompt for this agent
    pub prompt: String,
    /// Tools available to this agent
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Model to use (sonnet, opus, haiku, or inherit)
    #[serde(default)]
    pub model: Option<AgentModelStr>,
    /// Whether this agent can spawn sub-agents
    #[serde(default)]
    pub can_spawn_agents: bool,
}

/// Agent model selection as string
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentModelStr {
    Sonnet,
    Opus,
    Haiku,
    Inherit,
}

/// Compaction configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionDef {
    /// Whether compaction is enabled
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Max context tokens before compaction
    #[serde(default)]
    pub max_context_tokens: Option<usize>,
    /// Minimum messages to keep
    #[serde(default)]
    pub min_messages_to_keep: Option<usize>,
    /// Target tokens after compaction
    #[serde(default)]
    pub target_tokens: Option<usize>,
}

/// Parsed CLAUDE.md file
#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    /// Frontmatter configuration
    pub frontmatter: ClaudeFrontmatter,
    /// Markdown content (system prompt)
    pub content: String,
    /// Source file path
    pub source_path: Option<PathBuf>,
}

impl ClaudeConfig {
    /// Parse a CLAUDE.md file from a string
    pub fn parse(input: &str) -> ClaudeConfigResult<Self> {
        let (frontmatter, content) = Self::split_frontmatter(input)?;

        let frontmatter: ClaudeFrontmatter = if frontmatter.is_empty() {
            ClaudeFrontmatter::default()
        } else {
            serde_yaml::from_str(&frontmatter)
                .map_err(|e| ClaudeConfigError::YamlParse(e.to_string()))?
        };

        Ok(Self {
            frontmatter,
            content: content.trim().to_string(),
            source_path: None,
        })
    }

    /// Load a CLAUDE.md file from a path
    pub fn load(path: impl AsRef<Path>) -> ClaudeConfigResult<Self> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)?;
        let mut config = Self::parse(&content)?;
        config.source_path = Some(path.to_path_buf());
        Ok(config)
    }

    /// Load CLAUDE.md from a directory (looks for CLAUDE.md file)
    pub fn load_from_dir(dir: impl AsRef<Path>) -> ClaudeConfigResult<Self> {
        let dir = dir.as_ref();
        let claude_md = dir.join("CLAUDE.md");

        if claude_md.exists() {
            Self::load(&claude_md)
        } else {
            // Return empty config if no CLAUDE.md found
            Ok(Self {
                frontmatter: ClaudeFrontmatter::default(),
                content: String::new(),
                source_path: None,
            })
        }
    }

    /// Split frontmatter from content
    fn split_frontmatter(input: &str) -> ClaudeConfigResult<(String, String)> {
        let input = input.trim();

        if !input.starts_with("---") {
            // No frontmatter, entire content is markdown
            return Ok((String::new(), input.to_string()));
        }

        // Find the closing ---
        let after_first = &input[3..];
        let end_pos = after_first.find("\n---");

        match end_pos {
            Some(pos) => {
                let frontmatter = after_first[..pos].trim().to_string();
                let content = after_first[pos + 4..].trim().to_string();
                Ok((frontmatter, content))
            }
            None => Err(ClaudeConfigError::InvalidFrontmatter(
                "Missing closing --- for frontmatter".to_string(),
            )),
        }
    }

    /// Get the system prompt (markdown content)
    pub fn system_prompt(&self) -> &str {
        &self.content
    }

    /// Get agent name
    pub fn name(&self) -> Option<&str> {
        self.frontmatter.name.as_deref()
    }

    /// Check if this config extends another
    pub fn extends(&self) -> Option<&str> {
        self.frontmatter.extends.as_deref()
    }

    /// Validate the configuration
    pub fn validate(&self) -> ClaudeConfigResult<()> {
        // Validate tools config
        if let Some(tools) = &self.frontmatter.tools {
            // Check for conflicts between allowed and blocked
            for blocked in &tools.blocked {
                for allowed in &tools.allowed {
                    if allowed == blocked {
                        return Err(ClaudeConfigError::Validation(format!(
                            "Tool '{}' is both allowed and blocked",
                            blocked
                        )));
                    }
                }
            }
        }

        // Validate permissions
        if let Some(perms) = &self.frontmatter.permissions {
            for blocked in &perms.blocked_directories {
                for allowed in &perms.allowed_directories {
                    if allowed == blocked {
                        return Err(ClaudeConfigError::Validation(format!(
                            "Directory '{}' is both allowed and blocked",
                            blocked.display()
                        )));
                    }
                }
            }
        }

        Ok(())
    }
}

/// Configuration loader with inheritance support
pub struct ClaudeConfigLoader {
    /// Base configurations by name
    configs: HashMap<String, ClaudeConfig>,
    /// Search paths for configurations
    search_paths: Vec<PathBuf>,
}

impl ClaudeConfigLoader {
    /// Create a new configuration loader
    pub fn new() -> Self {
        Self {
            configs: HashMap::new(),
            search_paths: Vec::new(),
        }
    }

    /// Add a search path for configurations
    pub fn add_search_path(&mut self, path: impl Into<PathBuf>) {
        self.search_paths.push(path.into());
    }

    /// Register a named configuration
    pub fn register(&mut self, name: impl Into<String>, config: ClaudeConfig) {
        self.configs.insert(name.into(), config);
    }

    /// Load a configuration by name or path
    pub fn load(&mut self, name_or_path: &str) -> ClaudeConfigResult<ClaudeConfig> {
        // Check if already loaded
        if let Some(config) = self.configs.get(name_or_path) {
            return Ok(config.clone());
        }

        // Try as a path first
        let path = Path::new(name_or_path);
        if path.exists() {
            let config = ClaudeConfig::load(path)?;
            return Ok(config);
        }

        // Search in search paths
        for search_path in &self.search_paths {
            let candidate = search_path.join(format!("{}.md", name_or_path));
            if candidate.exists() {
                let config = ClaudeConfig::load(&candidate)?;
                self.configs
                    .insert(name_or_path.to_string(), config.clone());
                return Ok(config);
            }

            // Also try CLAUDE.md in subdirectory
            let candidate = search_path.join(name_or_path).join("CLAUDE.md");
            if candidate.exists() {
                let config = ClaudeConfig::load(&candidate)?;
                self.configs
                    .insert(name_or_path.to_string(), config.clone());
                return Ok(config);
            }
        }

        Err(ClaudeConfigError::ParentNotFound(name_or_path.to_string()))
    }

    /// Resolve inheritance and merge configurations
    pub fn resolve(&mut self, config: &ClaudeConfig) -> ClaudeConfigResult<ClaudeConfig> {
        self.resolve_with_chain(config, &mut Vec::new())
    }

    fn resolve_with_chain(
        &mut self,
        config: &ClaudeConfig,
        chain: &mut Vec<String>,
    ) -> ClaudeConfigResult<ClaudeConfig> {
        let name = config.name().unwrap_or("anonymous").to_string();

        // Check for circular inheritance
        if chain.contains(&name) {
            return Err(ClaudeConfigError::CircularInheritance(format!(
                "Circular inheritance detected: {:?} -> {}",
                chain, name
            )));
        }
        chain.push(name.clone());

        // If no parent, return as-is
        let parent_name = match config.extends() {
            Some(parent) => parent.to_string(),
            None => return Ok(config.clone()),
        };

        // Load and resolve parent
        let parent = self.load(&parent_name)?;
        let resolved_parent = self.resolve_with_chain(&parent, chain)?;

        // Merge child into parent
        Ok(Self::merge_configs(&resolved_parent, config))
    }

    /// Merge child config into parent config
    fn merge_configs(parent: &ClaudeConfig, child: &ClaudeConfig) -> ClaudeConfig {
        let mut merged = parent.clone();
        let parent_fm = &parent.frontmatter;
        let child_fm = &child.frontmatter;

        // Merge frontmatter fields (child overrides parent)
        merged.frontmatter.name = child_fm.name.clone().or_else(|| parent_fm.name.clone());
        merged.frontmatter.description = child_fm
            .description
            .clone()
            .or_else(|| parent_fm.description.clone());
        merged.frontmatter.model = child_fm.model.clone().or_else(|| parent_fm.model.clone());
        merged.frontmatter.max_turns = child_fm.max_turns.or(parent_fm.max_turns);
        merged.frontmatter.max_budget_usd = child_fm.max_budget_usd.or(parent_fm.max_budget_usd);
        merged.frontmatter.enable_thinking = child_fm.enable_thinking.or(parent_fm.enable_thinking);
        merged.frontmatter.api_timeout_secs =
            child_fm.api_timeout_secs.or(parent_fm.api_timeout_secs);
        merged.frontmatter.tool_timeout_secs =
            child_fm.tool_timeout_secs.or(parent_fm.tool_timeout_secs);

        // Clear extends in merged config
        merged.frontmatter.extends = None;

        // Merge tools
        merged.frontmatter.tools =
            Self::merge_tools(parent_fm.tools.as_ref(), child_fm.tools.as_ref());

        // Merge permissions
        merged.frontmatter.permissions = Self::merge_permissions(
            parent_fm.permissions.as_ref(),
            child_fm.permissions.as_ref(),
        );

        // Merge MCP servers
        merged.frontmatter.mcp_servers = Self::merge_maps(
            parent_fm.mcp_servers.as_ref(),
            child_fm.mcp_servers.as_ref(),
        );

        // Merge agents
        merged.frontmatter.agents =
            Self::merge_maps(parent_fm.agents.as_ref(), child_fm.agents.as_ref());

        // Merge compaction
        merged.frontmatter.compaction =
            Self::merge_compaction(parent_fm.compaction.as_ref(), child_fm.compaction.as_ref());

        // Merge metadata
        merged.frontmatter.metadata =
            Self::merge_maps(parent_fm.metadata.as_ref(), child_fm.metadata.as_ref());

        // Merge content (append child content to parent)
        if !child.content.is_empty() {
            if !merged.content.is_empty() {
                merged.content.push_str("\n\n");
            }
            merged.content.push_str(&child.content);
        }

        // Update source path to child
        merged.source_path = child.source_path.clone();

        merged
    }

    fn merge_tools(
        parent: Option<&ToolsConfig>,
        child: Option<&ToolsConfig>,
    ) -> Option<ToolsConfig> {
        match (parent, child) {
            (None, None) => None,
            (Some(p), None) => Some(p.clone()),
            (None, Some(c)) => Some(c.clone()),
            (Some(p), Some(c)) => {
                let mut merged = p.clone();

                // Child allowed tools override/extend parent
                if !c.allowed.is_empty() {
                    merged.allowed = c.allowed.clone();
                }

                // Blocked tools are merged (union)
                for blocked in &c.blocked {
                    if !merged.blocked.contains(blocked) {
                        merged.blocked.push(blocked.clone());
                    }
                }

                // Merge tool configs
                if let Some(child_config) = &c.config {
                    let mut config = merged.config.unwrap_or_default();
                    for (k, v) in child_config {
                        config.insert(k.clone(), v.clone());
                    }
                    merged.config = Some(config);
                }

                Some(merged)
            }
        }
    }

    fn merge_permissions(
        parent: Option<&PermissionsConfig>,
        child: Option<&PermissionsConfig>,
    ) -> Option<PermissionsConfig> {
        match (parent, child) {
            (None, None) => None,
            (Some(p), None) => Some(p.clone()),
            (None, Some(c)) => Some(c.clone()),
            (Some(p), Some(c)) => {
                let mut merged = p.clone();

                // Child mode overrides parent
                if c.mode.is_some() {
                    merged.mode = c.mode.clone();
                }

                // Merge directories (union)
                for dir in &c.allowed_directories {
                    if !merged.allowed_directories.contains(dir) {
                        merged.allowed_directories.push(dir.clone());
                    }
                }
                for dir in &c.blocked_directories {
                    if !merged.blocked_directories.contains(dir) {
                        merged.blocked_directories.push(dir.clone());
                    }
                }

                // Merge commands (union)
                for cmd in &c.allowed_commands {
                    if !merged.allowed_commands.contains(cmd) {
                        merged.allowed_commands.push(cmd.clone());
                    }
                }
                for cmd in &c.blocked_commands {
                    if !merged.blocked_commands.contains(cmd) {
                        merged.blocked_commands.push(cmd.clone());
                    }
                }

                Some(merged)
            }
        }
    }

    fn merge_maps<V: Clone>(
        parent: Option<&HashMap<String, V>>,
        child: Option<&HashMap<String, V>>,
    ) -> Option<HashMap<String, V>> {
        match (parent, child) {
            (None, None) => None,
            (Some(p), None) => Some(p.clone()),
            (None, Some(c)) => Some(c.clone()),
            (Some(p), Some(c)) => {
                let mut merged = p.clone();
                for (k, v) in c {
                    merged.insert(k.clone(), v.clone());
                }
                Some(merged)
            }
        }
    }

    fn merge_compaction(
        parent: Option<&CompactionDef>,
        child: Option<&CompactionDef>,
    ) -> Option<CompactionDef> {
        match (parent, child) {
            (None, None) => None,
            (Some(p), None) => Some(p.clone()),
            (None, Some(c)) => Some(c.clone()),
            (Some(p), Some(c)) => Some(CompactionDef {
                enabled: c.enabled.or(p.enabled),
                max_context_tokens: c.max_context_tokens.or(p.max_context_tokens),
                min_messages_to_keep: c.min_messages_to_keep.or(p.min_messages_to_keep),
                target_tokens: c.target_tokens.or(p.target_tokens),
            }),
        }
    }
}

impl Default for ClaudeConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert ClaudeConfig to AgentConfig
impl From<ClaudeConfig> for AgentConfig {
    fn from(config: ClaudeConfig) -> Self {
        let fm = &config.frontmatter;
        let mut agent_config = AgentConfig::default();

        // Basic settings
        if let Some(model) = &fm.model {
            agent_config.model = Some(model.clone());
        }
        if let Some(max_turns) = fm.max_turns {
            agent_config.max_turns = max_turns;
        }
        if let Some(max_budget) = fm.max_budget_usd {
            agent_config.max_budget_usd = Some(max_budget);
        }
        if let Some(enable_thinking) = fm.enable_thinking {
            agent_config.enable_thinking = enable_thinking;
        }
        if let Some(api_timeout) = fm.api_timeout_secs {
            agent_config.api_timeout_secs = api_timeout;
        }
        if let Some(tool_timeout) = fm.tool_timeout_secs {
            agent_config.tool_timeout_secs = tool_timeout;
        }

        // System prompt from markdown content
        if !config.content.is_empty() {
            agent_config.system_prompt = Some(config.content.clone());
        }

        // Tools configuration
        if let Some(tools) = &fm.tools {
            agent_config.tools = tools.allowed.clone();
            agent_config.blocked_tools = tools.blocked.clone();
        }

        // Permissions configuration
        if let Some(perms) = &fm.permissions {
            if let Some(mode) = &perms.mode {
                agent_config.permission_mode = mode.clone().into();
            }
            agent_config.allowed_directories = perms.allowed_directories.clone();
        }

        // MCP servers
        if let Some(mcp_servers) = &fm.mcp_servers {
            for (name, server) in mcp_servers {
                agent_config.mcp_servers.insert(
                    name.clone(),
                    McpServerConfig {
                        command: server.command.clone(),
                        args: server.args.clone(),
                        env: server.env.clone(),
                        cwd: server.cwd.clone(),
                    },
                );
            }
        }

        // Custom agents
        if let Some(agents) = &fm.agents {
            for (name, agent) in agents {
                agent_config.agents.insert(
                    name.clone(),
                    AgentDefinition {
                        description: agent.description.clone(),
                        prompt: agent.prompt.clone(),
                        tools: agent.tools.clone(),
                        model: agent.model.as_ref().map(|m| match m {
                            AgentModelStr::Sonnet => AgentModel::Sonnet,
                            AgentModelStr::Opus => AgentModel::Opus,
                            AgentModelStr::Haiku => AgentModel::Haiku,
                            AgentModelStr::Inherit => AgentModel::Inherit,
                        }),
                    },
                );
            }
        }

        // Compaction
        if let Some(compaction) = &fm.compaction {
            if let Some(enabled) = compaction.enabled {
                agent_config.compaction.enabled = enabled;
            }
            if let Some(max_tokens) = compaction.max_context_tokens {
                agent_config.compaction.max_context_tokens = max_tokens;
            }
            if let Some(min_messages) = compaction.min_messages_to_keep {
                agent_config.compaction.min_messages_to_keep = min_messages;
            }
            if let Some(target) = compaction.target_tokens {
                agent_config.compaction.target_tokens = target;
            }
        }

        agent_config
    }
}

/// Builder for creating AgentConfig from CLAUDE.md
pub struct ClaudeConfigBuilder {
    loader: ClaudeConfigLoader,
    base_config: Option<ClaudeConfig>,
    working_dir: Option<PathBuf>,
}

impl ClaudeConfigBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            loader: ClaudeConfigLoader::new(),
            base_config: None,
            working_dir: None,
        }
    }

    /// Set the working directory
    pub fn working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    /// Add a search path for parent configurations
    pub fn search_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.loader.add_search_path(path);
        self
    }

    /// Load base configuration from a CLAUDE.md file
    pub fn load_file(mut self, path: impl AsRef<Path>) -> ClaudeConfigResult<Self> {
        self.base_config = Some(ClaudeConfig::load(path)?);
        Ok(self)
    }

    /// Load base configuration from a directory
    pub fn load_dir(mut self, dir: impl AsRef<Path>) -> ClaudeConfigResult<Self> {
        self.base_config = Some(ClaudeConfig::load_from_dir(dir)?);
        Ok(self)
    }

    /// Parse configuration from a string
    pub fn parse(mut self, content: &str) -> ClaudeConfigResult<Self> {
        self.base_config = Some(ClaudeConfig::parse(content)?);
        Ok(self)
    }

    /// Register a named configuration for inheritance
    pub fn register(mut self, name: impl Into<String>, config: ClaudeConfig) -> Self {
        self.loader.register(name, config);
        self
    }

    /// Build the AgentConfig
    pub fn build(mut self) -> ClaudeConfigResult<AgentConfig> {
        let config = match self.base_config.take() {
            Some(config) => config,
            None => ClaudeConfig {
                frontmatter: ClaudeFrontmatter::default(),
                content: String::new(),
                source_path: None,
            },
        };

        // Validate
        config.validate()?;

        // Resolve inheritance
        let resolved = self.loader.resolve(&config)?;

        // Convert to AgentConfig
        let mut agent_config: AgentConfig = resolved.into();

        // Set working directory if provided
        if let Some(cwd) = self.working_dir {
            agent_config.cwd = Some(cwd);
        }

        Ok(agent_config)
    }
}

impl Default for ClaudeConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Discover and load CLAUDE.md files from directory hierarchy
pub fn discover_configs(start_dir: &Path) -> ClaudeConfigResult<Vec<ClaudeConfig>> {
    let mut configs = Vec::new();
    let mut current = start_dir.to_path_buf();

    loop {
        let claude_md = current.join("CLAUDE.md");
        if claude_md.exists() {
            configs.push(ClaudeConfig::load(&claude_md)?);
        }

        // Also check for .claude/CLAUDE.md
        let dot_claude = current.join(".claude").join("CLAUDE.md");
        if dot_claude.exists() {
            configs.push(ClaudeConfig::load(&dot_claude)?);
        }

        // Move to parent directory
        if !current.pop() {
            break;
        }
    }

    // Reverse so that root configs come first
    configs.reverse();
    Ok(configs)
}

/// Merge multiple CLAUDE.md configs in order (later configs override earlier)
pub fn merge_discovered_configs(configs: Vec<ClaudeConfig>) -> ClaudeConfigResult<ClaudeConfig> {
    let mut merged = ClaudeConfig {
        frontmatter: ClaudeFrontmatter::default(),
        content: String::new(),
        source_path: None,
    };

    for config in configs {
        merged = ClaudeConfigLoader::merge_configs(&merged, &config);
    }

    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_claude_md() {
        let input = r#"---
name: test-agent
model: claude-sonnet-4-6
---

# Test Agent

This is the system prompt.
"#;

        let config = ClaudeConfig::parse(input).unwrap();
        assert_eq!(config.name(), Some("test-agent"));
        assert_eq!(
            config.frontmatter.model,
            Some("claude-sonnet-4-6".to_string())
        );
        assert!(config.content.contains("Test Agent"));
    }

    #[test]
    fn test_parse_no_frontmatter() {
        let input = r#"# Just Markdown

No frontmatter here.
"#;

        let config = ClaudeConfig::parse(input).unwrap();
        assert!(config.frontmatter.name.is_none());
        assert!(config.content.contains("Just Markdown"));
    }

    #[test]
    fn test_parse_full_config() {
        let input = r#"---
name: full-agent
model: claude-opus-4-6
extends: base-agent
max_turns: 50
max_budget_usd: 10.0
enable_thinking: true
tools:
  allowed:
    - Read
    - Write
    - Edit
  blocked:
    - Bash
permissions:
  mode: accept_edits
  allowed_directories:
    - /home/user/projects
mcp_servers:
  filesystem:
    command: mcp-server-filesystem
    args:
      - /home/user
agents:
  researcher:
    description: Research agent
    prompt: You are a research assistant
    model: haiku
---

# Full Agent

This is a full test.
"#;

        let config = ClaudeConfig::parse(input).unwrap();
        assert_eq!(config.name(), Some("full-agent"));
        assert_eq!(config.extends(), Some("base-agent"));
        assert_eq!(config.frontmatter.max_turns, Some(50));

        let tools = config.frontmatter.tools.as_ref().unwrap();
        assert!(tools.allowed.contains(&"Read".to_string()));
        assert!(tools.blocked.contains(&"Bash".to_string()));

        let perms = config.frontmatter.permissions.as_ref().unwrap();
        assert!(matches!(perms.mode, Some(PermissionModeStr::AcceptEdits)));

        let mcp = config.frontmatter.mcp_servers.as_ref().unwrap();
        assert!(mcp.contains_key("filesystem"));

        let agents = config.frontmatter.agents.as_ref().unwrap();
        assert!(agents.contains_key("researcher"));
    }

    #[test]
    fn test_validate_tool_conflict() {
        let input = r#"---
tools:
  allowed:
    - Bash
  blocked:
    - Bash
---
"#;

        let config = ClaudeConfig::parse(input).unwrap();
        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_to_agent_config() {
        let input = r#"---
name: converter-test
model: claude-sonnet-4-6
max_turns: 25
tools:
  allowed:
    - Read
    - Write
permissions:
  mode: plan
---

# Test Prompt

Use TypeScript.
"#;

        let config = ClaudeConfig::parse(input).unwrap();
        let agent_config: AgentConfig = config.into();

        assert_eq!(agent_config.model, Some("claude-sonnet-4-6".to_string()));
        assert_eq!(agent_config.max_turns, 25);
        assert_eq!(agent_config.tools, vec!["Read", "Write"]);
        assert_eq!(agent_config.permission_mode, PermissionMode::Plan);
        assert!(agent_config.system_prompt.unwrap().contains("TypeScript"));
    }

    #[test]
    fn test_config_inheritance() {
        let parent_input = r#"---
name: parent
model: claude-sonnet-4-6
max_turns: 100
tools:
  allowed:
    - Read
    - Write
---

# Parent Instructions

Base rules.
"#;

        let child_input = r#"---
name: child
extends: parent
max_turns: 50
tools:
  allowed:
    - Edit
  blocked:
    - Write
---

# Child Instructions

Additional rules.
"#;

        let parent = ClaudeConfig::parse(parent_input).unwrap();
        let child = ClaudeConfig::parse(child_input).unwrap();

        let mut loader = ClaudeConfigLoader::new();
        loader.register("parent", parent);

        let resolved = loader.resolve(&child).unwrap();

        // Child max_turns should override parent
        assert_eq!(resolved.frontmatter.max_turns, Some(50));

        // Model should be inherited from parent
        assert_eq!(
            resolved.frontmatter.model,
            Some("claude-sonnet-4-6".to_string())
        );

        // Child allowed tools should override parent
        let tools = resolved.frontmatter.tools.as_ref().unwrap();
        assert_eq!(tools.allowed, vec!["Edit"]);
        assert!(tools.blocked.contains(&"Write".to_string()));

        // Content should be merged
        assert!(resolved.content.contains("Parent Instructions"));
        assert!(resolved.content.contains("Child Instructions"));
    }

    #[test]
    fn test_circular_inheritance() {
        let a_input = r#"---
name: a
extends: b
---
"#;

        let b_input = r#"---
name: b
extends: a
---
"#;

        let a = ClaudeConfig::parse(a_input).unwrap();
        let b = ClaudeConfig::parse(b_input).unwrap();

        let mut loader = ClaudeConfigLoader::new();
        loader.register("a", a.clone());
        loader.register("b", b);

        let result = loader.resolve(&a);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ClaudeConfigError::CircularInheritance(_)
        ));
    }

    #[test]
    fn test_builder() {
        let input = r#"---
name: builder-test
max_turns: 30
---

# Test
"#;

        let agent_config = ClaudeConfigBuilder::new()
            .working_dir("/tmp/test")
            .parse(input)
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(agent_config.max_turns, 30);
        assert_eq!(agent_config.cwd, Some(PathBuf::from("/tmp/test")));
    }
}
