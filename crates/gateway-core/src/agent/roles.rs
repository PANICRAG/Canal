//! Agent Role and Tool Permission System
//!
//! Defines agent categories/roles and their allowed tools.
//! Each agent type can only access tools within their designated categories.
//!
//! # Example
//!
//! ```rust,ignore
//! use gateway_core::agent::roles::{AgentRole, ToolPermissionManager};
//!
//! let manager = ToolPermissionManager::new();
//!
//! // Check if an agent can use a tool
//! if manager.can_use_tool(AgentRole::Administrative, "calendar.create_event") {
//!     // Execute tool
//! }
//!
//! // Get all allowed tools for a role
//! let tools = manager.get_allowed_tools(AgentRole::Marketing);
//! ```

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Agent role/category defining what type of work the agent handles
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    /// Administrative tasks: scheduling, emails, document management
    Administrative,

    /// Creative tasks: design, video editing, content creation
    Creative,

    /// Marketing tasks: campaigns, analytics, social media
    Marketing,

    /// Engineering tasks: coding, debugging, deployment
    Engineering,

    /// Research tasks: data analysis, information gathering
    Research,

    /// Customer support: ticket handling, FAQ, communication
    CustomerSupport,

    /// Finance tasks: invoicing, budgets, reports
    Finance,

    /// HR tasks: recruiting, onboarding, employee management
    HumanResources,

    /// General purpose: has access to common tools only
    General,

    /// System admin: full access (use with caution)
    SystemAdmin,
}

impl AgentRole {
    /// Get display name for the role
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Administrative => "Administrative Assistant",
            Self::Creative => "Creative Assistant",
            Self::Marketing => "Marketing Assistant",
            Self::Engineering => "Engineering Assistant",
            Self::Research => "Research Assistant",
            Self::CustomerSupport => "Customer Support",
            Self::Finance => "Finance Assistant",
            Self::HumanResources => "HR Assistant",
            Self::General => "General Assistant",
            Self::SystemAdmin => "System Administrator",
        }
    }

    /// Get description of the role
    pub fn description(&self) -> &'static str {
        match self {
            Self::Administrative => "Handles scheduling, emails, documents, and office tasks",
            Self::Creative => "Handles design, video editing, and content creation",
            Self::Marketing => "Handles campaigns, analytics, and social media",
            Self::Engineering => "Handles coding, debugging, and technical tasks",
            Self::Research => "Handles data analysis and information gathering",
            Self::CustomerSupport => "Handles customer inquiries and support tickets",
            Self::Finance => "Handles invoicing, budgets, and financial reports",
            Self::HumanResources => "Handles recruiting, onboarding, and HR tasks",
            Self::General => "General purpose assistant with basic capabilities",
            Self::SystemAdmin => "Full system access for administration",
        }
    }

    /// Get all roles
    pub fn all() -> Vec<Self> {
        vec![
            Self::Administrative,
            Self::Creative,
            Self::Marketing,
            Self::Engineering,
            Self::Research,
            Self::CustomerSupport,
            Self::Finance,
            Self::HumanResources,
            Self::General,
            Self::SystemAdmin,
        ]
    }
}

impl std::fmt::Display for AgentRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Tool category for grouping related tools
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    /// Common tools available to all roles
    Common,

    /// Calendar and scheduling tools
    Calendar,

    /// Email and communication tools
    Communication,

    /// Document management tools
    Documents,

    /// File system operations
    FileSystem,

    /// Creative and design tools
    Creative,

    /// Video and audio editing tools
    MediaEditing,

    /// Marketing and analytics tools
    MarketingAnalytics,

    /// Social media management
    SocialMedia,

    /// Code editing and development tools
    Development,

    /// Git and version control
    VersionControl,

    /// Database operations
    Database,

    /// Web browsing and scraping
    WebBrowsing,

    /// Search and research tools
    Search,

    /// Customer support tools
    Support,

    /// Financial tools
    Financial,

    /// HR and recruiting tools
    HumanResources,

    /// System administration tools
    System,

    /// External API integrations
    ExternalApi,
}

/// Tool definition with category and permission info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name/identifier
    pub name: String,

    /// Tool category
    pub category: ToolCategory,

    /// Human-readable description
    pub description: String,

    /// Whether the tool is potentially dangerous
    pub is_sensitive: bool,

    /// Whether the tool requires confirmation
    pub requires_confirmation: bool,
}

/// Manages tool permissions for different agent roles
#[derive(Debug, Clone)]
pub struct ToolPermissionManager {
    /// Role to allowed tool categories mapping
    role_categories: HashMap<AgentRole, HashSet<ToolCategory>>,

    /// Tool definitions by name
    tool_definitions: HashMap<String, ToolDefinition>,

    /// Custom role-specific tool overrides (allow or deny specific tools)
    custom_overrides: HashMap<AgentRole, ToolOverrides>,
}

/// Custom overrides for specific tools
#[derive(Debug, Clone, Default)]
pub struct ToolOverrides {
    /// Explicitly allowed tools (even if category not allowed)
    pub allowed: HashSet<String>,

    /// Explicitly denied tools (even if category is allowed)
    pub denied: HashSet<String>,
}

impl ToolPermissionManager {
    /// Create a new permission manager with default configuration
    pub fn new() -> Self {
        let mut manager = Self {
            role_categories: HashMap::new(),
            tool_definitions: HashMap::new(),
            custom_overrides: HashMap::new(),
        };

        manager.setup_default_permissions();
        manager.setup_default_tools();

        manager
    }

    /// Setup default role-to-category mappings
    fn setup_default_permissions(&mut self) {
        // Administrative: calendar, email, documents
        self.role_categories.insert(
            AgentRole::Administrative,
            HashSet::from([
                ToolCategory::Common,
                ToolCategory::Calendar,
                ToolCategory::Communication,
                ToolCategory::Documents,
                ToolCategory::Search,
            ]),
        );

        // Creative: design, media, files
        self.role_categories.insert(
            AgentRole::Creative,
            HashSet::from([
                ToolCategory::Common,
                ToolCategory::Creative,
                ToolCategory::MediaEditing,
                ToolCategory::FileSystem,
                ToolCategory::Documents,
            ]),
        );

        // Marketing: analytics, social, communication
        self.role_categories.insert(
            AgentRole::Marketing,
            HashSet::from([
                ToolCategory::Common,
                ToolCategory::MarketingAnalytics,
                ToolCategory::SocialMedia,
                ToolCategory::Communication,
                ToolCategory::Search,
                ToolCategory::WebBrowsing,
            ]),
        );

        // Engineering: development, git, database, system
        self.role_categories.insert(
            AgentRole::Engineering,
            HashSet::from([
                ToolCategory::Common,
                ToolCategory::Development,
                ToolCategory::VersionControl,
                ToolCategory::Database,
                ToolCategory::FileSystem,
                ToolCategory::Search,
                ToolCategory::WebBrowsing,
            ]),
        );

        // Research: search, web, documents
        self.role_categories.insert(
            AgentRole::Research,
            HashSet::from([
                ToolCategory::Common,
                ToolCategory::Search,
                ToolCategory::WebBrowsing,
                ToolCategory::Documents,
                ToolCategory::Database,
            ]),
        );

        // Customer Support: support tools, communication
        self.role_categories.insert(
            AgentRole::CustomerSupport,
            HashSet::from([
                ToolCategory::Common,
                ToolCategory::Support,
                ToolCategory::Communication,
                ToolCategory::Search,
                ToolCategory::Documents,
            ]),
        );

        // Finance: financial tools, documents
        self.role_categories.insert(
            AgentRole::Finance,
            HashSet::from([
                ToolCategory::Common,
                ToolCategory::Financial,
                ToolCategory::Documents,
                ToolCategory::Communication,
            ]),
        );

        // HR: hr tools, communication, documents
        self.role_categories.insert(
            AgentRole::HumanResources,
            HashSet::from([
                ToolCategory::Common,
                ToolCategory::HumanResources,
                ToolCategory::Communication,
                ToolCategory::Documents,
                ToolCategory::Calendar,
            ]),
        );

        // General: only common tools
        self.role_categories
            .insert(AgentRole::General, HashSet::from([ToolCategory::Common]));

        // SystemAdmin: all categories
        let all_categories: HashSet<ToolCategory> = [
            ToolCategory::Common,
            ToolCategory::Calendar,
            ToolCategory::Communication,
            ToolCategory::Documents,
            ToolCategory::FileSystem,
            ToolCategory::Creative,
            ToolCategory::MediaEditing,
            ToolCategory::MarketingAnalytics,
            ToolCategory::SocialMedia,
            ToolCategory::Development,
            ToolCategory::VersionControl,
            ToolCategory::Database,
            ToolCategory::WebBrowsing,
            ToolCategory::Search,
            ToolCategory::Support,
            ToolCategory::Financial,
            ToolCategory::HumanResources,
            ToolCategory::System,
            ToolCategory::ExternalApi,
        ]
        .into_iter()
        .collect();

        self.role_categories
            .insert(AgentRole::SystemAdmin, all_categories);
    }

    /// Setup default tool definitions
    fn setup_default_tools(&mut self) {
        // Common tools
        self.register_tool("read", ToolCategory::Common, "Read file contents", false);
        self.register_tool(
            "search",
            ToolCategory::Common,
            "Search for information",
            false,
        );
        self.register_tool(
            "ask_user",
            ToolCategory::Common,
            "Ask user a question",
            false,
        );

        // Calendar tools
        self.register_tool(
            "calendar.list_events",
            ToolCategory::Calendar,
            "List calendar events",
            false,
        );
        self.register_tool(
            "calendar.create_event",
            ToolCategory::Calendar,
            "Create calendar event",
            false,
        );
        self.register_tool(
            "calendar.update_event",
            ToolCategory::Calendar,
            "Update calendar event",
            false,
        );
        self.register_tool(
            "calendar.delete_event",
            ToolCategory::Calendar,
            "Delete calendar event",
            true,
        );

        // Communication tools
        self.register_tool(
            "email.read",
            ToolCategory::Communication,
            "Read emails",
            false,
        );
        self.register_tool(
            "email.send",
            ToolCategory::Communication,
            "Send email",
            true,
        );
        self.register_tool(
            "email.draft",
            ToolCategory::Communication,
            "Create email draft",
            false,
        );
        self.register_tool(
            "slack.send_message",
            ToolCategory::Communication,
            "Send Slack message",
            false,
        );
        self.register_tool(
            "slack.read_channel",
            ToolCategory::Communication,
            "Read Slack channel",
            false,
        );

        // Document tools
        self.register_tool(
            "docs.create",
            ToolCategory::Documents,
            "Create document",
            false,
        );
        self.register_tool("docs.read", ToolCategory::Documents, "Read document", false);
        self.register_tool(
            "docs.update",
            ToolCategory::Documents,
            "Update document",
            false,
        );
        self.register_tool(
            "docs.delete",
            ToolCategory::Documents,
            "Delete document",
            true,
        );
        self.register_tool(
            "docs.export",
            ToolCategory::Documents,
            "Export document",
            false,
        );

        // File system tools
        self.register_tool("fs.read", ToolCategory::FileSystem, "Read file", false);
        self.register_tool("fs.write", ToolCategory::FileSystem, "Write file", true);
        self.register_tool("fs.delete", ToolCategory::FileSystem, "Delete file", true);
        self.register_tool("fs.list", ToolCategory::FileSystem, "List directory", false);
        self.register_tool("fs.move", ToolCategory::FileSystem, "Move file", true);

        // Creative tools (disabled for now)
        self.register_tool(
            "creative.design",
            ToolCategory::Creative,
            "Create design",
            false,
        );
        self.register_tool(
            "creative.image_edit",
            ToolCategory::Creative,
            "Edit image",
            false,
        );

        // Media editing tools (disabled for now)
        self.register_tool(
            "media.video_edit",
            ToolCategory::MediaEditing,
            "Edit video",
            false,
        );
        self.register_tool(
            "media.audio_edit",
            ToolCategory::MediaEditing,
            "Edit audio",
            false,
        );
        self.register_tool(
            "media.color_grade",
            ToolCategory::MediaEditing,
            "Color grading",
            false,
        );

        // Marketing tools
        self.register_tool(
            "marketing.analytics",
            ToolCategory::MarketingAnalytics,
            "View analytics",
            false,
        );
        self.register_tool(
            "marketing.campaign_create",
            ToolCategory::MarketingAnalytics,
            "Create campaign",
            true,
        );
        self.register_tool(
            "marketing.report",
            ToolCategory::MarketingAnalytics,
            "Generate report",
            false,
        );

        // Social media tools
        self.register_tool(
            "social.post",
            ToolCategory::SocialMedia,
            "Create social post",
            true,
        );
        self.register_tool(
            "social.schedule",
            ToolCategory::SocialMedia,
            "Schedule post",
            false,
        );
        self.register_tool(
            "social.analytics",
            ToolCategory::SocialMedia,
            "View social analytics",
            false,
        );

        // Development tools
        self.register_tool(
            "code.read",
            ToolCategory::Development,
            "Read code file",
            false,
        );
        self.register_tool(
            "code.write",
            ToolCategory::Development,
            "Write code file",
            true,
        );
        self.register_tool(
            "code.edit",
            ToolCategory::Development,
            "Edit code file",
            true,
        );
        self.register_tool("code.run", ToolCategory::Development, "Run code", true);
        self.register_tool(
            "bash",
            ToolCategory::Development,
            "Execute bash command",
            true,
        );

        // Version control tools
        self.register_tool(
            "git.status",
            ToolCategory::VersionControl,
            "Git status",
            false,
        );
        self.register_tool("git.diff", ToolCategory::VersionControl, "Git diff", false);
        self.register_tool(
            "git.commit",
            ToolCategory::VersionControl,
            "Git commit",
            true,
        );
        self.register_tool("git.push", ToolCategory::VersionControl, "Git push", true);
        self.register_tool("git.pull", ToolCategory::VersionControl, "Git pull", false);

        // Database tools
        self.register_tool("db.query", ToolCategory::Database, "Execute query", true);
        self.register_tool("db.read", ToolCategory::Database, "Read data", false);

        // Web browsing tools
        self.register_tool(
            "browser.navigate",
            ToolCategory::WebBrowsing,
            "Navigate to URL",
            false,
        );
        self.register_tool(
            "browser.snapshot",
            ToolCategory::WebBrowsing,
            "Get page accessibility tree with ref IDs (PRIMARY tool)",
            false,
        );
        self.register_tool(
            "browser.click",
            ToolCategory::WebBrowsing,
            "Click element by ref ID or selector",
            false,
        );
        self.register_tool(
            "browser.fill",
            ToolCategory::WebBrowsing,
            "Fill form field by ref ID",
            false,
        );
        self.register_tool(
            "browser.get_page_text",
            ToolCategory::WebBrowsing,
            "Get page text content (low tokens)",
            false,
        );
        self.register_tool(
            "browser.find",
            ToolCategory::WebBrowsing,
            "Find element by natural language description",
            false,
        );
        self.register_tool(
            "browser.screenshot",
            ToolCategory::WebBrowsing,
            "Take screenshot (avoid - high token cost)",
            false,
        );

        // Search tools
        self.register_tool("web_search", ToolCategory::Search, "Web search", false);
        self.register_tool("grep", ToolCategory::Search, "Search in files", false);
        self.register_tool("glob", ToolCategory::Search, "Find files by pattern", false);

        // Support tools
        self.register_tool(
            "support.ticket_read",
            ToolCategory::Support,
            "Read support ticket",
            false,
        );
        self.register_tool(
            "support.ticket_reply",
            ToolCategory::Support,
            "Reply to ticket",
            true,
        );
        self.register_tool(
            "support.ticket_close",
            ToolCategory::Support,
            "Close ticket",
            true,
        );

        // Financial tools
        self.register_tool(
            "finance.invoice_create",
            ToolCategory::Financial,
            "Create invoice",
            true,
        );
        self.register_tool(
            "finance.invoice_read",
            ToolCategory::Financial,
            "Read invoice",
            false,
        );
        self.register_tool(
            "finance.report",
            ToolCategory::Financial,
            "Generate financial report",
            false,
        );

        // HR tools
        self.register_tool(
            "hr.employee_read",
            ToolCategory::HumanResources,
            "Read employee info",
            false,
        );
        self.register_tool(
            "hr.schedule_interview",
            ToolCategory::HumanResources,
            "Schedule interview",
            false,
        );
        self.register_tool(
            "hr.onboarding",
            ToolCategory::HumanResources,
            "Manage onboarding",
            false,
        );

        // System tools
        self.register_tool(
            "system.config",
            ToolCategory::System,
            "System configuration",
            true,
        );
        self.register_tool(
            "system.logs",
            ToolCategory::System,
            "View system logs",
            false,
        );
        self.register_tool(
            "system.restart",
            ToolCategory::System,
            "Restart service",
            true,
        );
    }

    /// Register a tool definition
    fn register_tool(
        &mut self,
        name: &str,
        category: ToolCategory,
        description: &str,
        is_sensitive: bool,
    ) {
        self.tool_definitions.insert(
            name.to_string(),
            ToolDefinition {
                name: name.to_string(),
                category,
                description: description.to_string(),
                is_sensitive,
                requires_confirmation: is_sensitive,
            },
        );
    }

    /// Check if a role can use a specific tool
    pub fn can_use_tool(&self, role: AgentRole, tool_name: &str) -> bool {
        // Check custom overrides first
        if let Some(overrides) = self.custom_overrides.get(&role) {
            if overrides.denied.contains(tool_name) {
                return false;
            }
            if overrides.allowed.contains(tool_name) {
                return true;
            }
        }

        // Check category permissions
        if let Some(tool_def) = self.tool_definitions.get(tool_name) {
            if let Some(allowed_categories) = self.role_categories.get(&role) {
                return allowed_categories.contains(&tool_def.category);
            }
        }

        // Unknown tool - deny by default
        false
    }

    /// Get all allowed tools for a role
    pub fn get_allowed_tools(&self, role: AgentRole) -> Vec<&ToolDefinition> {
        self.tool_definitions
            .values()
            .filter(|tool| self.can_use_tool(role, &tool.name))
            .collect()
    }

    /// Get all allowed tool names for a role
    pub fn get_allowed_tool_names(&self, role: AgentRole) -> Vec<String> {
        self.get_allowed_tools(role)
            .iter()
            .map(|t| t.name.clone())
            .collect()
    }

    /// Get tool categories allowed for a role
    pub fn get_allowed_categories(&self, role: AgentRole) -> HashSet<ToolCategory> {
        self.role_categories.get(&role).cloned().unwrap_or_default()
    }

    /// Add a custom override for a role
    pub fn add_override(&mut self, role: AgentRole, overrides: ToolOverrides) {
        self.custom_overrides.insert(role, overrides);
    }

    /// Allow a specific tool for a role
    pub fn allow_tool(&mut self, role: AgentRole, tool_name: &str) {
        self.custom_overrides
            .entry(role)
            .or_default()
            .allowed
            .insert(tool_name.to_string());
    }

    /// Deny a specific tool for a role
    pub fn deny_tool(&mut self, role: AgentRole, tool_name: &str) {
        self.custom_overrides
            .entry(role)
            .or_default()
            .denied
            .insert(tool_name.to_string());
    }

    /// Register a new tool
    pub fn register_custom_tool(&mut self, tool: ToolDefinition) {
        self.tool_definitions.insert(tool.name.clone(), tool);
    }

    /// Get tool definition by name
    pub fn get_tool(&self, name: &str) -> Option<&ToolDefinition> {
        self.tool_definitions.get(name)
    }

    /// Check if a tool is sensitive
    pub fn is_sensitive_tool(&self, name: &str) -> bool {
        self.tool_definitions
            .get(name)
            .map(|t| t.is_sensitive)
            .unwrap_or(true) // Unknown tools are treated as sensitive
    }

    /// Validate a list of tool calls for a role
    pub fn validate_tool_calls(
        &self,
        role: AgentRole,
        tool_names: &[String],
    ) -> ToolValidationResult {
        let mut allowed = Vec::new();
        let mut denied = Vec::new();

        for name in tool_names {
            if self.can_use_tool(role, name) {
                allowed.push(name.clone());
            } else {
                denied.push(name.clone());
            }
        }

        ToolValidationResult { allowed, denied }
    }
}

impl Default for ToolPermissionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of tool validation
#[derive(Debug, Clone)]
pub struct ToolValidationResult {
    /// Tools that are allowed
    pub allowed: Vec<String>,

    /// Tools that are denied
    pub denied: Vec<String>,
}

impl ToolValidationResult {
    /// Check if all tools are allowed
    pub fn all_allowed(&self) -> bool {
        self.denied.is_empty()
    }

    /// Get error message for denied tools
    pub fn error_message(&self) -> Option<String> {
        if self.denied.is_empty() {
            None
        } else {
            Some(format!(
                "Access denied for tools: {}",
                self.denied.join(", ")
            ))
        }
    }
}

/// Agent configuration with role
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleBasedAgentConfig {
    /// Agent identifier
    pub agent_id: String,

    /// Agent role
    pub role: AgentRole,

    /// Optional custom name
    pub name: Option<String>,

    /// Optional custom description
    pub description: Option<String>,

    /// Custom tool overrides
    #[serde(default)]
    pub tool_overrides: Option<ToolOverridesConfig>,
}

/// Serializable tool overrides config
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolOverridesConfig {
    /// Explicitly allowed tools
    #[serde(default)]
    pub allowed: Vec<String>,

    /// Explicitly denied tools
    #[serde(default)]
    pub denied: Vec<String>,
}

impl From<ToolOverridesConfig> for ToolOverrides {
    fn from(config: ToolOverridesConfig) -> Self {
        Self {
            allowed: config.allowed.into_iter().collect(),
            denied: config.denied.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_administrative_permissions() {
        let manager = ToolPermissionManager::new();

        // Administrative can use calendar tools
        assert!(manager.can_use_tool(AgentRole::Administrative, "calendar.create_event"));
        assert!(manager.can_use_tool(AgentRole::Administrative, "email.send"));

        // Administrative cannot use development tools
        assert!(!manager.can_use_tool(AgentRole::Administrative, "code.write"));
        assert!(!manager.can_use_tool(AgentRole::Administrative, "bash"));
    }

    #[test]
    fn test_engineering_permissions() {
        let manager = ToolPermissionManager::new();

        // Engineering can use development tools
        assert!(manager.can_use_tool(AgentRole::Engineering, "code.write"));
        assert!(manager.can_use_tool(AgentRole::Engineering, "bash"));
        assert!(manager.can_use_tool(AgentRole::Engineering, "git.commit"));

        // Engineering cannot use HR tools
        assert!(!manager.can_use_tool(AgentRole::Engineering, "hr.employee_read"));
    }

    #[test]
    fn test_custom_overrides() {
        let mut manager = ToolPermissionManager::new();

        // By default, General cannot use bash
        assert!(!manager.can_use_tool(AgentRole::General, "bash"));

        // Add custom override to allow bash
        manager.allow_tool(AgentRole::General, "bash");
        assert!(manager.can_use_tool(AgentRole::General, "bash"));

        // Deny a normally allowed tool
        manager.deny_tool(AgentRole::Engineering, "bash");
        assert!(!manager.can_use_tool(AgentRole::Engineering, "bash"));
    }

    #[test]
    fn test_system_admin_full_access() {
        let manager = ToolPermissionManager::new();

        // SystemAdmin has access to everything
        assert!(manager.can_use_tool(AgentRole::SystemAdmin, "calendar.create_event"));
        assert!(manager.can_use_tool(AgentRole::SystemAdmin, "code.write"));
        assert!(manager.can_use_tool(AgentRole::SystemAdmin, "hr.employee_read"));
        assert!(manager.can_use_tool(AgentRole::SystemAdmin, "system.restart"));
    }

    #[test]
    fn test_validate_tool_calls() {
        let manager = ToolPermissionManager::new();

        let tools = vec![
            "calendar.create_event".to_string(),
            "code.write".to_string(),
        ];

        let result = manager.validate_tool_calls(AgentRole::Administrative, &tools);

        assert_eq!(result.allowed, vec!["calendar.create_event"]);
        assert_eq!(result.denied, vec!["code.write"]);
        assert!(!result.all_allowed());
    }
}
