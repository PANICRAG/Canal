//! Permission Types - Claude Agent SDK Compatible
//!
//! Defines permission modes, rules, and results for tool execution.

use serde::{Deserialize, Serialize};

/// Permission mode for the agent — re-exported from gateway-tool-types.
pub use gateway_tool_types::PermissionMode;

/// Permission check result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "behavior")]
pub enum PermissionResult {
    /// Allow the operation
    #[serde(rename = "allow")]
    Allow {
        /// Updated input (if modified by permission handler)
        #[serde(skip_serializing_if = "Option::is_none")]
        updated_input: Option<serde_json::Value>,
        /// Permission updates to apply
        #[serde(skip_serializing_if = "Option::is_none")]
        updated_permissions: Option<Vec<PermissionUpdate>>,
    },

    /// Deny the operation
    #[serde(rename = "deny")]
    Deny {
        /// Reason for denial
        message: String,
        /// Whether to interrupt the entire agent loop
        interrupt: bool,
    },

    /// Ask the user for permission
    #[serde(rename = "ask")]
    Ask {
        /// Question to ask the user
        question: String,
        /// Suggested permissions
        #[serde(skip_serializing_if = "Option::is_none")]
        suggestions: Option<Vec<PermissionSuggestion>>,
    },
}

impl PermissionResult {
    /// Create an allow result
    pub fn allow() -> Self {
        Self::Allow {
            updated_input: None,
            updated_permissions: None,
        }
    }

    /// Create an allow result with updated input
    pub fn allow_with_input(input: serde_json::Value) -> Self {
        Self::Allow {
            updated_input: Some(input),
            updated_permissions: None,
        }
    }

    /// Create a deny result
    pub fn deny(message: impl Into<String>, interrupt: bool) -> Self {
        Self::Deny {
            message: message.into(),
            interrupt,
        }
    }

    /// Create an ask result
    pub fn ask(question: impl Into<String>) -> Self {
        Self::Ask {
            question: question.into(),
            suggestions: None,
        }
    }

    /// Check if this is an allow result
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow { .. })
    }

    /// Check if this is a deny result
    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Deny { .. })
    }
}

/// Permission behavior for rules
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionBehavior {
    /// Allow the operation
    Allow,
    /// Deny the operation
    Deny,
    /// Ask the user
    Ask,
}

/// Permission update to apply
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PermissionUpdate {
    /// Add permission rules
    #[serde(rename = "addRules")]
    AddRules {
        rules: Vec<PermissionRule>,
        behavior: PermissionBehavior,
        destination: PermissionDestination,
    },

    /// Replace permission rules
    #[serde(rename = "replaceRules")]
    ReplaceRules {
        rules: Vec<PermissionRule>,
        behavior: PermissionBehavior,
        destination: PermissionDestination,
    },

    /// Remove permission rules
    #[serde(rename = "removeRules")]
    RemoveRules {
        rules: Vec<PermissionRule>,
        behavior: PermissionBehavior,
        destination: PermissionDestination,
    },

    /// Set permission mode
    #[serde(rename = "setMode")]
    SetMode {
        mode: PermissionMode,
        destination: PermissionDestination,
    },

    /// Add allowed directories
    #[serde(rename = "addDirectories")]
    AddDirectories {
        directories: Vec<String>,
        destination: PermissionDestination,
    },

    /// Remove allowed directories
    #[serde(rename = "removeDirectories")]
    RemoveDirectories {
        directories: Vec<String>,
        destination: PermissionDestination,
    },
}

/// Permission rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    /// Tool name pattern (glob or exact)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Path pattern (for file operations)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Command pattern (for bash)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

impl PermissionRule {
    /// Create a rule for a tool
    pub fn tool(pattern: impl Into<String>) -> Self {
        Self {
            tool: Some(pattern.into()),
            path: None,
            command: None,
        }
    }

    /// Create a rule for a path
    pub fn path(pattern: impl Into<String>) -> Self {
        Self {
            tool: None,
            path: Some(pattern.into()),
            command: None,
        }
    }

    /// Create a rule for a command
    pub fn command(pattern: impl Into<String>) -> Self {
        Self {
            tool: None,
            path: None,
            command: Some(pattern.into()),
        }
    }
}

/// Where to store permission updates
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionDestination {
    /// User settings (persistent, applies globally)
    UserSettings,
    /// Project settings (persistent, applies to project)
    ProjectSettings,
    /// Local settings (persistent, applies to local directory)
    LocalSettings,
    /// Session only (temporary, applies to current session)
    Session,
}

/// Permission suggestion for ask result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionSuggestion {
    /// Description of the suggestion
    pub description: String,
    /// Permission update to apply if accepted
    pub update: PermissionUpdate,
}

// ============================================================================
// Interactive Permission Request/Response Types
// ============================================================================

/// Permission request sent to the user
///
/// When a tool requires user approval, this request is sent to the client.
/// The client should display this to the user and collect their response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    /// Unique request ID for tracking
    pub request_id: String,
    /// Tool name requesting permission
    pub tool_name: String,
    /// Tool input parameters
    pub tool_input: serde_json::Value,
    /// Question to display to the user
    pub question: String,
    /// Available options for the user to choose from
    pub options: Vec<PermissionOption>,
    /// Session ID this request belongs to
    pub session_id: String,
    /// Tool use ID from the LLM response (for correlating with tool results)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    /// Timestamp when the request was created
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl PermissionRequest {
    /// Create a new permission request
    pub fn new(
        tool_name: impl Into<String>,
        tool_input: serde_json::Value,
        question: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            tool_name: tool_name.into(),
            tool_input,
            question: question.into(),
            options: Self::default_options(),
            session_id: session_id.into(),
            tool_use_id: None,
            created_at: Some(chrono::Utc::now()),
        }
    }

    /// Set the tool use ID
    pub fn with_tool_use_id(mut self, id: impl Into<String>) -> Self {
        self.tool_use_id = Some(id.into());
        self
    }

    /// Set custom options
    pub fn with_options(mut self, options: Vec<PermissionOption>) -> Self {
        self.options = options;
        self
    }

    /// Default permission options
    pub fn default_options() -> Vec<PermissionOption> {
        vec![
            PermissionOption {
                label: "Allow".to_string(),
                value: "allow".to_string(),
                is_default: false,
                description: Some("Allow this tool execution".to_string()),
            },
            PermissionOption {
                label: "Deny".to_string(),
                value: "deny".to_string(),
                is_default: true,
                description: Some("Deny this tool execution".to_string()),
            },
            PermissionOption {
                label: "Always allow this tool".to_string(),
                value: "always_allow".to_string(),
                is_default: false,
                description: Some(
                    "Add a rule to always allow this tool in this session".to_string(),
                ),
            },
            PermissionOption {
                label: "Always deny this tool".to_string(),
                value: "always_deny".to_string(),
                is_default: false,
                description: Some(
                    "Add a rule to always deny this tool in this session".to_string(),
                ),
            },
        ]
    }
}

/// Permission option for user selection
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    /// Display label for the option
    pub label: String,
    /// Value to send back when selected
    pub value: String,
    /// Whether this is the default option
    pub is_default: bool,
    /// Optional description for the option
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Permission response from the user
///
/// This is sent by the client after the user makes a decision on a permission request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionResponse {
    /// Request ID being responded to
    pub request_id: String,
    /// Session ID
    pub session_id: String,
    /// Whether permission was granted
    pub granted: bool,
    /// User's selected option value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_option: Option<String>,
    /// Optional modified input (user can edit the tool input before approving)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_input: Option<serde_json::Value>,
}

impl PermissionResponse {
    /// Create an allow response
    pub fn allow(request_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            session_id: session_id.into(),
            granted: true,
            selected_option: Some("allow".to_string()),
            modified_input: None,
        }
    }

    /// Create a deny response
    pub fn deny(request_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            session_id: session_id.into(),
            granted: false,
            selected_option: Some("deny".to_string()),
            modified_input: None,
        }
    }

    /// Create an always allow response
    pub fn always_allow(request_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            session_id: session_id.into(),
            granted: true,
            selected_option: Some("always_allow".to_string()),
            modified_input: None,
        }
    }

    /// Check if this response grants permission
    pub fn is_granted(&self) -> bool {
        self.granted
    }

    /// Check if this should create an "always allow" rule
    pub fn is_always_allow(&self) -> bool {
        self.selected_option.as_deref() == Some("always_allow")
    }

    /// Check if this should create an "always deny" rule
    pub fn is_always_deny(&self) -> bool {
        self.selected_option.as_deref() == Some("always_deny")
    }
}

/// State of a pending permission request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PendingPermissionState {
    /// Waiting for user response
    Pending,
    /// User responded - granted
    Granted {
        modified_input: Option<serde_json::Value>,
    },
    /// User responded - denied
    Denied,
    /// Request timed out
    TimedOut,
    /// Request was cancelled
    Cancelled,
}

/// Pending permission with state
#[derive(Debug, Clone)]
pub struct PendingPermission {
    /// The original request
    pub request: PermissionRequest,
    /// Current state
    pub state: PendingPermissionState,
    /// When the state last changed
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Context for permission checks
#[derive(Debug, Clone, Default)]
pub struct PermissionContext {
    /// Current working directory
    pub cwd: Option<String>,
    /// Session ID
    pub session_id: Option<String>,
    /// Current permission mode
    pub mode: PermissionMode,
    /// Allowed directories
    pub allowed_directories: Vec<String>,
    /// Permission rules
    pub rules: Vec<(PermissionRule, PermissionBehavior)>,
}

impl PermissionContext {
    /// Check if a tool is allowed
    pub fn check_tool(&self, tool_name: &str, input: &serde_json::Value) -> PermissionResult {
        // Bypass mode allows everything
        if self.mode == PermissionMode::BypassPermissions {
            return PermissionResult::allow();
        }

        // Plan mode denies modifications
        if self.mode == PermissionMode::Plan {
            let modifying_tools = ["Write", "Edit", "Bash", "NotebookEdit"];
            if modifying_tools.iter().any(|t| tool_name.contains(t)) {
                return PermissionResult::deny("Plan mode does not allow modifications", false);
            }
        }

        // Check rules
        for (rule, behavior) in &self.rules {
            if self.rule_matches(rule, tool_name, input) {
                match behavior {
                    PermissionBehavior::Allow => return PermissionResult::allow(),
                    PermissionBehavior::Deny => {
                        return PermissionResult::deny("Denied by permission rule", false)
                    }
                    PermissionBehavior::Ask => {
                        return PermissionResult::ask(format!("Allow {} to execute?", tool_name))
                    }
                }
            }
        }

        // Default behavior based on mode
        match self.mode {
            PermissionMode::AcceptEdits if is_edit_tool(tool_name) => PermissionResult::allow(),
            _ => PermissionResult::ask(format!("Allow {} to execute?", tool_name)),
        }
    }

    fn rule_matches(
        &self,
        rule: &PermissionRule,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> bool {
        // Check tool pattern
        if let Some(tool_pattern) = &rule.tool {
            if !glob_match(tool_pattern, tool_name) {
                return false;
            }
        }

        // Check path pattern
        if let Some(path_pattern) = &rule.path {
            let path = input
                .get("file_path")
                .or_else(|| input.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !glob_match(path_pattern, path) {
                return false;
            }
        }

        // Check command pattern
        if let Some(cmd_pattern) = &rule.command {
            let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if !glob_match(cmd_pattern, command) {
                return false;
            }
        }

        true
    }
}

/// Check if a tool is an editing tool
fn is_edit_tool(tool_name: &str) -> bool {
    let edit_tools = ["Write", "Edit", "NotebookEdit"];
    edit_tools.iter().any(|t| tool_name.contains(t))
}

/// Simple glob matching
fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if pattern.starts_with('*') && pattern.ends_with('*') {
        let middle = &pattern[1..pattern.len() - 1];
        return text.contains(middle);
    }

    if pattern.starts_with('*') {
        let suffix = &pattern[1..];
        return text.ends_with(suffix);
    }

    if pattern.ends_with('*') {
        let prefix = &pattern[..pattern.len() - 1];
        return text.starts_with(prefix);
    }

    pattern == text
}

// ============================================================================
// Permission Checker Trait and Implementations
// ============================================================================

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Permission checker trait for evaluating tool execution permissions
///
/// Implement this trait to create custom permission checking logic.
/// The checker is called before each tool execution to determine if
/// the operation should be allowed.
#[async_trait]
pub trait PermissionChecker: Send + Sync {
    /// Check if a tool execution is permitted
    ///
    /// # Arguments
    /// * `tool_name` - Name of the tool being executed
    /// * `input` - Tool input parameters
    /// * `context` - Permission context with rules and mode
    ///
    /// # Returns
    /// * `PermissionResult::Allow` - Allow execution, optionally with modified input
    /// * `PermissionResult::Deny` - Deny execution with reason
    /// * `PermissionResult::Ask` - Require user approval
    async fn check(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        context: &PermissionContext,
    ) -> PermissionResult;

    /// Get the checker name (for debugging and logging)
    fn name(&self) -> &str {
        "anonymous"
    }

    /// Check if this checker handles a specific tool
    fn handles_tool(&self, tool_name: &str) -> bool {
        let _ = tool_name;
        true // By default, handle all tools
    }
}

/// Default permission checker using context rules and mode
pub struct DefaultPermissionChecker;

#[async_trait]
impl PermissionChecker for DefaultPermissionChecker {
    async fn check(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        context: &PermissionContext,
    ) -> PermissionResult {
        context.check_tool(tool_name, input)
    }

    fn name(&self) -> &str {
        "default"
    }
}

/// Permission checker that requires approval for dangerous commands
pub struct DangerousCommandChecker {
    /// Dangerous command patterns
    pub patterns: Vec<String>,
}

impl Default for DangerousCommandChecker {
    fn default() -> Self {
        Self {
            patterns: vec![
                "rm -rf".to_string(),
                "rm -r".to_string(),
                "sudo".to_string(),
                "chmod".to_string(),
                "chown".to_string(),
                "dd ".to_string(),
                "mkfs".to_string(),
                "shutdown".to_string(),
                "reboot".to_string(),
                "> /dev/".to_string(),
                "curl | sh".to_string(),
                "wget | sh".to_string(),
                "curl | bash".to_string(),
                "wget | bash".to_string(),
            ],
        }
    }
}

#[async_trait]
impl PermissionChecker for DangerousCommandChecker {
    async fn check(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        context: &PermissionContext,
    ) -> PermissionResult {
        // In bypass mode, don't check dangerous commands
        if context.mode == PermissionMode::BypassPermissions {
            return PermissionResult::allow();
        }

        // Only check Bash tool
        if !tool_name.contains("Bash") {
            return PermissionResult::allow();
        }

        // Get the command from input
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

        // Check for dangerous patterns
        for pattern in &self.patterns {
            if command.contains(pattern) {
                return PermissionResult::ask(format!(
                    "This command contains '{}' which could be dangerous. Allow execution of: {}?",
                    pattern, command
                ));
            }
        }

        PermissionResult::allow()
    }

    fn name(&self) -> &str {
        "dangerous_command"
    }

    fn handles_tool(&self, tool_name: &str) -> bool {
        tool_name.contains("Bash")
    }
}

/// Permission checker for file operations outside allowed directories
pub struct PathSecurityChecker {
    /// Blocked path prefixes
    pub blocked_paths: Vec<String>,
}

impl Default for PathSecurityChecker {
    fn default() -> Self {
        Self {
            blocked_paths: vec![
                "/etc".to_string(),
                "/var".to_string(),
                "/usr".to_string(),
                "/bin".to_string(),
                "/sbin".to_string(),
                "/root".to_string(),
                "/System".to_string(),
                "/Library".to_string(),
                "C:\\Windows".to_string(),
                "C:\\Program Files".to_string(),
            ],
        }
    }
}

#[async_trait]
impl PermissionChecker for PathSecurityChecker {
    async fn check(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        context: &PermissionContext,
    ) -> PermissionResult {
        // Only check file operation tools
        let file_tools = ["Read", "Write", "Edit", "NotebookEdit"];
        if !file_tools.iter().any(|t| tool_name.contains(t)) {
            return PermissionResult::allow();
        }

        // Get the file path from input
        let path = input
            .get("file_path")
            .or_else(|| input.get("path"))
            .or_else(|| input.get("notebook_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Check for blocked system paths FIRST (security critical)
        for blocked in &self.blocked_paths {
            if path.starts_with(blocked) {
                return PermissionResult::deny(
                    format!("Access to system path '{}' is not allowed", blocked),
                    false,
                );
            }
        }

        // Check if path is in allowed directories
        if !context.allowed_directories.is_empty() {
            let path_allowed = context.allowed_directories.iter().any(|allowed| {
                path.starts_with(allowed) || path.starts_with(&format!("{}/", allowed))
            });

            if !path_allowed {
                return PermissionResult::ask(format!(
                    "File '{}' is outside allowed directories. Allow access?",
                    path
                ));
            }
        }

        PermissionResult::allow()
    }

    fn name(&self) -> &str {
        "path_security"
    }

    fn handles_tool(&self, tool_name: &str) -> bool {
        let file_tools = ["Read", "Write", "Edit", "NotebookEdit"];
        file_tools.iter().any(|t| tool_name.contains(t))
    }
}

/// Composite permission checker that chains multiple checkers
pub struct CompositePermissionChecker {
    checkers: Vec<Arc<dyn PermissionChecker>>,
}

impl Default for CompositePermissionChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl CompositePermissionChecker {
    /// Create a new composite checker
    pub fn new() -> Self {
        Self {
            checkers: Vec::new(),
        }
    }

    /// Create with default checkers
    pub fn with_defaults() -> Self {
        Self {
            checkers: vec![
                Arc::new(DefaultPermissionChecker),
                Arc::new(DangerousCommandChecker::default()),
                Arc::new(PathSecurityChecker::default()),
            ],
        }
    }

    /// Add a checker
    pub fn add_checker(&mut self, checker: Arc<dyn PermissionChecker>) {
        self.checkers.push(checker);
    }

    /// Add a checker (builder pattern)
    pub fn with_checker(mut self, checker: Arc<dyn PermissionChecker>) -> Self {
        self.checkers.push(checker);
        self
    }
}

#[async_trait]
impl PermissionChecker for CompositePermissionChecker {
    async fn check(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        context: &PermissionContext,
    ) -> PermissionResult {
        for checker in &self.checkers {
            // Skip checkers that don't handle this tool
            if !checker.handles_tool(tool_name) {
                continue;
            }

            let result = checker.check(tool_name, input, context).await;

            // If denied, stop immediately
            if result.is_denied() {
                return result;
            }

            // If asking, return the ask (first checker to ask wins)
            if matches!(result, PermissionResult::Ask { .. }) {
                return result;
            }

            // If allowed with updates, apply them
            if let PermissionResult::Allow {
                updated_input: Some(_),
                ..
            } = &result
            {
                return result;
            }
        }

        // All checkers passed
        PermissionResult::allow()
    }

    fn name(&self) -> &str {
        "composite"
    }
}

// ============================================================================
// Permission Manager for Dynamic Rule Updates
// ============================================================================

/// Permission manager for managing permission rules at runtime
///
/// This provides a thread-safe way to manage permission rules that can be
/// updated dynamically during agent execution.
pub struct PermissionManager {
    /// Current permission mode
    mode: RwLock<PermissionMode>,
    /// Session rules (temporary, cleared when session ends)
    session_rules: RwLock<Vec<(PermissionRule, PermissionBehavior)>>,
    /// Persistent rules (survive session restarts)
    persistent_rules: RwLock<Vec<(PermissionRule, PermissionBehavior)>>,
    /// Allowed directories
    allowed_directories: RwLock<Vec<String>>,
    /// The permission checker to use
    checker: Arc<dyn PermissionChecker>,
}

impl Default for PermissionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionManager {
    /// Create a new permission manager
    pub fn new() -> Self {
        Self {
            mode: RwLock::new(PermissionMode::BypassPermissions),
            session_rules: RwLock::new(Vec::new()),
            persistent_rules: RwLock::new(Vec::new()),
            allowed_directories: RwLock::new(Vec::new()),
            checker: Arc::new(CompositePermissionChecker::with_defaults()),
        }
    }

    /// Create with a custom checker
    pub fn with_checker(checker: Arc<dyn PermissionChecker>) -> Self {
        Self {
            mode: RwLock::new(PermissionMode::BypassPermissions),
            session_rules: RwLock::new(Vec::new()),
            persistent_rules: RwLock::new(Vec::new()),
            allowed_directories: RwLock::new(Vec::new()),
            checker,
        }
    }

    /// Get the current permission mode
    pub async fn mode(&self) -> PermissionMode {
        *self.mode.read().await
    }

    /// Set the permission mode
    pub async fn set_mode(&self, mode: PermissionMode) {
        *self.mode.write().await = mode;
    }

    /// Add a session rule (temporary)
    pub async fn add_session_rule(&self, rule: PermissionRule, behavior: PermissionBehavior) {
        self.session_rules.write().await.push((rule, behavior));
    }

    /// Add a persistent rule
    pub async fn add_persistent_rule(&self, rule: PermissionRule, behavior: PermissionBehavior) {
        self.persistent_rules.write().await.push((rule, behavior));
    }

    /// Remove rules matching a predicate
    pub async fn remove_session_rules<F>(&self, predicate: F)
    where
        F: Fn(&PermissionRule, &PermissionBehavior) -> bool,
    {
        let mut rules = self.session_rules.write().await;
        rules.retain(|(r, b)| !predicate(r, b));
    }

    /// Clear all session rules
    pub async fn clear_session_rules(&self) {
        self.session_rules.write().await.clear();
    }

    /// Add an allowed directory
    pub async fn add_allowed_directory(&self, dir: impl Into<String>) {
        self.allowed_directories.write().await.push(dir.into());
    }

    /// Remove an allowed directory
    pub async fn remove_allowed_directory(&self, dir: &str) {
        let mut dirs = self.allowed_directories.write().await;
        dirs.retain(|d| d != dir);
    }

    /// Build a permission context from current state
    pub async fn build_context(
        &self,
        session_id: Option<&str>,
        cwd: Option<&str>,
    ) -> PermissionContext {
        let mode = *self.mode.read().await;
        let session_rules = self.session_rules.read().await.clone();
        let persistent_rules = self.persistent_rules.read().await.clone();
        let allowed_directories = self.allowed_directories.read().await.clone();

        let mut rules = persistent_rules;
        rules.extend(session_rules);

        PermissionContext {
            mode,
            rules,
            allowed_directories,
            cwd: cwd.map(String::from),
            session_id: session_id.map(String::from),
        }
    }

    /// Check tool permission using the manager's checker
    pub async fn check_tool(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        session_id: Option<&str>,
        cwd: Option<&str>,
    ) -> PermissionResult {
        let context = self.build_context(session_id, cwd).await;
        self.checker.check(tool_name, input, &context).await
    }

    /// Apply a permission update
    pub async fn apply_update(&self, update: PermissionUpdate) {
        match update {
            PermissionUpdate::AddRules {
                rules,
                behavior,
                destination,
            } => {
                for rule in rules {
                    match destination {
                        PermissionDestination::Session => {
                            self.add_session_rule(rule, behavior).await;
                        }
                        _ => {
                            self.add_persistent_rule(rule, behavior).await;
                        }
                    }
                }
            }
            PermissionUpdate::SetMode { mode, .. } => {
                self.set_mode(mode).await;
            }
            PermissionUpdate::AddDirectories { directories, .. } => {
                for dir in directories {
                    self.add_allowed_directory(dir).await;
                }
            }
            PermissionUpdate::RemoveDirectories { directories, .. } => {
                for dir in directories {
                    self.remove_allowed_directory(&dir).await;
                }
            }
            PermissionUpdate::ReplaceRules {
                rules,
                behavior,
                destination,
            } => {
                // Clear existing rules of the same behavior and destination
                match destination {
                    PermissionDestination::Session => {
                        self.remove_session_rules(|_, b| *b == behavior).await;
                    }
                    _ => {
                        // For persistent rules, would need similar logic
                    }
                }
                // Add new rules
                for rule in rules {
                    match destination {
                        PermissionDestination::Session => {
                            self.add_session_rule(rule, behavior).await;
                        }
                        _ => {
                            self.add_persistent_rule(rule, behavior).await;
                        }
                    }
                }
            }
            PermissionUpdate::RemoveRules {
                rules,
                behavior,
                destination,
            } => {
                // Remove matching rules
                let rules_to_remove: Vec<_> = rules.into_iter().collect();
                match destination {
                    PermissionDestination::Session => {
                        self.remove_session_rules(|r, b| {
                            *b == behavior
                                && rules_to_remove.iter().any(|rr| {
                                    rr.tool == r.tool
                                        && rr.path == r.path
                                        && rr.command == r.command
                                })
                        })
                        .await;
                    }
                    _ => {
                        // For persistent rules, would need similar logic
                    }
                }
            }
        }
    }
}

// ============================================================================
// Permission Hook for Hook System Integration
// ============================================================================

use super::hooks::{HookContext, HookEvent, HookResult};
use crate::agent::hooks::HookCallback;

/// Permission checking hook that integrates with the hook system
pub struct PermissionHook {
    manager: Arc<PermissionManager>,
}

impl PermissionHook {
    /// Create a new permission hook
    pub fn new(manager: Arc<PermissionManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl HookCallback for PermissionHook {
    async fn on_event(
        &self,
        event: HookEvent,
        data: serde_json::Value,
        context: &HookContext,
    ) -> HookResult {
        // Only handle PreToolUse and PermissionCheck events
        if !matches!(event, HookEvent::PreToolUse | HookEvent::PermissionCheck) {
            return HookResult::continue_();
        }

        // Extract tool info from data
        let tool_name = data
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let input = data
            .get("tool_input")
            .or_else(|| data.get("input"))
            .cloned()
            .unwrap_or(serde_json::json!({}));

        // Check permission
        let result = self
            .manager
            .check_tool(
                tool_name,
                &input,
                Some(&context.session_id),
                context.cwd.as_deref(),
            )
            .await;

        match result {
            PermissionResult::Allow {
                updated_input,
                updated_permissions,
            } => {
                // Apply any permission updates
                if let Some(updates) = updated_permissions {
                    for update in updates {
                        self.manager.apply_update(update).await;
                    }
                }

                // Return continue with modified input if provided
                if let Some(new_input) = updated_input {
                    let mut modified_data = data.clone();
                    if let Some(obj) = modified_data.as_object_mut() {
                        obj.insert("tool_input".to_string(), new_input.clone());
                        obj.insert("input".to_string(), new_input);
                    }
                    HookResult::continue_with(modified_data)
                } else {
                    HookResult::continue_()
                }
            }
            PermissionResult::Deny { message, .. } => HookResult::cancel(message),
            PermissionResult::Ask { question, .. } => {
                // Return a special result that indicates permission is needed
                // The agent runner should handle this by creating a PermissionRequest
                HookResult::Cancel {
                    reason: format!("PERMISSION_REQUIRED:{}", question),
                }
            }
        }
    }

    fn name(&self) -> &str {
        "permission_hook"
    }

    fn handles_event(&self, event: HookEvent) -> bool {
        matches!(event, HookEvent::PreToolUse | HookEvent::PermissionCheck)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_mode_defaults() {
        let mode = PermissionMode::default();
        assert_eq!(mode, PermissionMode::BypassPermissions);
        assert!(mode.auto_approve());
        assert!(mode.allows_edits());
    }

    #[test]
    fn test_permission_result_allow() {
        let result = PermissionResult::allow();
        assert!(result.is_allowed());
        assert!(!result.is_denied());
    }

    #[test]
    fn test_permission_result_deny() {
        let result = PermissionResult::deny("Not allowed", true);
        assert!(!result.is_allowed());
        assert!(result.is_denied());
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*.rs", "file.rs"));
        assert!(glob_match("test*", "test_file"));
        assert!(glob_match("*test*", "my_test_file"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "not_exact"));
    }

    #[test]
    fn test_permission_context_bypass() {
        let ctx = PermissionContext {
            mode: PermissionMode::BypassPermissions,
            ..Default::default()
        };
        let result = ctx.check_tool("Bash", &serde_json::json!({"command": "rm -rf /"}));
        assert!(result.is_allowed());
    }

    #[test]
    fn test_permission_context_plan() {
        let ctx = PermissionContext {
            mode: PermissionMode::Plan,
            ..Default::default()
        };
        let result = ctx.check_tool("Write", &serde_json::json!({"file_path": "/tmp/test"}));
        assert!(result.is_denied());
    }

    #[test]
    fn test_permission_request_creation() {
        let request = PermissionRequest::new(
            "Bash",
            serde_json::json!({"command": "ls -la"}),
            "Allow Bash to execute 'ls -la'?",
            "session-123",
        );

        assert_eq!(request.tool_name, "Bash");
        assert_eq!(request.session_id, "session-123");
        assert!(!request.request_id.is_empty());
        assert_eq!(request.options.len(), 4); // default options
    }

    #[test]
    fn test_permission_request_with_tool_use_id() {
        let request = PermissionRequest::new(
            "Write",
            serde_json::json!({"file_path": "/tmp/test.txt"}),
            "Allow Write?",
            "session-123",
        )
        .with_tool_use_id("tool-use-456");

        assert_eq!(request.tool_use_id, Some("tool-use-456".to_string()));
    }

    #[test]
    fn test_permission_response_allow() {
        let response = PermissionResponse::allow("req-123", "session-123");
        assert!(response.is_granted());
        assert!(!response.is_always_allow());
        assert!(!response.is_always_deny());
    }

    #[test]
    fn test_permission_response_always_allow() {
        let response = PermissionResponse::always_allow("req-123", "session-123");
        assert!(response.is_granted());
        assert!(response.is_always_allow());
        assert!(!response.is_always_deny());
    }

    #[test]
    fn test_permission_response_deny() {
        let response = PermissionResponse::deny("req-123", "session-123");
        assert!(!response.is_granted());
        assert!(!response.is_always_allow());
        assert!(!response.is_always_deny());
    }

    // ========================================================================
    // Permission Checker Tests
    // ========================================================================

    #[tokio::test]
    async fn test_default_permission_checker() {
        let checker = DefaultPermissionChecker;
        let ctx = PermissionContext {
            mode: PermissionMode::Default,
            ..Default::default()
        };

        let result = checker
            .check("Read", &serde_json::json!({"file_path": "/tmp/test"}), &ctx)
            .await;

        // Default mode asks for permission
        assert!(matches!(result, PermissionResult::Ask { .. }));
    }

    #[tokio::test]
    async fn test_dangerous_command_checker() {
        let checker = DangerousCommandChecker::default();
        let ctx = PermissionContext {
            mode: PermissionMode::Default,
            ..Default::default()
        };

        // Safe command should be allowed
        let result = checker
            .check("Bash", &serde_json::json!({"command": "ls -la"}), &ctx)
            .await;
        assert!(result.is_allowed());

        // Dangerous command should ask for permission
        let result = checker
            .check(
                "Bash",
                &serde_json::json!({"command": "rm -rf /tmp/test"}),
                &ctx,
            )
            .await;
        assert!(matches!(result, PermissionResult::Ask { .. }));

        // Non-Bash tools should be allowed
        let result = checker
            .check("Read", &serde_json::json!({"file_path": "/tmp/test"}), &ctx)
            .await;
        assert!(result.is_allowed());
    }

    #[tokio::test]
    async fn test_path_security_checker() {
        let checker = PathSecurityChecker::default();
        let mut ctx = PermissionContext {
            mode: PermissionMode::Default,
            ..Default::default()
        };
        ctx.allowed_directories = vec!["/home/user/project".to_string()];

        // Allowed path
        let result = checker
            .check(
                "Write",
                &serde_json::json!({"file_path": "/home/user/project/file.txt"}),
                &ctx,
            )
            .await;
        assert!(result.is_allowed());

        // Path outside allowed directories should ask
        let result = checker
            .check(
                "Write",
                &serde_json::json!({"file_path": "/tmp/file.txt"}),
                &ctx,
            )
            .await;
        assert!(matches!(result, PermissionResult::Ask { .. }));

        // System path should be denied
        let result = checker
            .check(
                "Write",
                &serde_json::json!({"file_path": "/etc/passwd"}),
                &ctx,
            )
            .await;
        assert!(result.is_denied());
    }

    #[tokio::test]
    async fn test_composite_permission_checker() {
        let checker = CompositePermissionChecker::with_defaults();
        let ctx = PermissionContext {
            mode: PermissionMode::BypassPermissions,
            ..Default::default()
        };

        // In bypass mode, even dangerous commands should be allowed
        let result = checker
            .check("Bash", &serde_json::json!({"command": "rm -rf /tmp"}), &ctx)
            .await;
        // First checker (default) returns allow due to bypass mode
        assert!(result.is_allowed());
    }

    // ========================================================================
    // Permission Manager Tests
    // ========================================================================

    #[tokio::test]
    async fn test_permission_manager_mode() {
        let manager = PermissionManager::new();

        assert_eq!(manager.mode().await, PermissionMode::BypassPermissions);

        manager.set_mode(PermissionMode::AcceptEdits).await;
        assert_eq!(manager.mode().await, PermissionMode::AcceptEdits);
    }

    #[tokio::test]
    async fn test_permission_manager_session_rules() {
        let manager = PermissionManager::new();

        // Add a session rule
        manager
            .add_session_rule(PermissionRule::tool("Bash"), PermissionBehavior::Allow)
            .await;

        let context = manager.build_context(Some("test"), None).await;
        assert_eq!(context.rules.len(), 1);

        // Clear session rules
        manager.clear_session_rules().await;
        let context = manager.build_context(Some("test"), None).await;
        assert!(context.rules.is_empty());
    }

    #[tokio::test]
    async fn test_permission_manager_allowed_directories() {
        let manager = PermissionManager::new();

        manager.add_allowed_directory("/home/user/project").await;
        manager.add_allowed_directory("/tmp").await;

        let context = manager.build_context(Some("test"), None).await;
        assert_eq!(context.allowed_directories.len(), 2);

        manager.remove_allowed_directory("/tmp").await;
        let context = manager.build_context(Some("test"), None).await;
        assert_eq!(context.allowed_directories.len(), 1);
        assert_eq!(context.allowed_directories[0], "/home/user/project");
    }

    #[tokio::test]
    async fn test_permission_manager_apply_update() {
        let manager = PermissionManager::new();

        // Apply SetMode update
        manager
            .apply_update(PermissionUpdate::SetMode {
                mode: PermissionMode::Plan,
                destination: PermissionDestination::Session,
            })
            .await;
        assert_eq!(manager.mode().await, PermissionMode::Plan);

        // Apply AddDirectories update
        manager
            .apply_update(PermissionUpdate::AddDirectories {
                directories: vec!["/home/user".to_string(), "/tmp".to_string()],
                destination: PermissionDestination::Session,
            })
            .await;
        let context = manager.build_context(None, None).await;
        assert_eq!(context.allowed_directories.len(), 2);

        // Apply AddRules update
        manager
            .apply_update(PermissionUpdate::AddRules {
                rules: vec![PermissionRule::tool("Read")],
                behavior: PermissionBehavior::Allow,
                destination: PermissionDestination::Session,
            })
            .await;
        let context = manager.build_context(None, None).await;
        assert_eq!(context.rules.len(), 1);
    }

    #[tokio::test]
    async fn test_permission_manager_check_tool() {
        let manager = PermissionManager::new();
        manager.set_mode(PermissionMode::BypassPermissions).await;

        let result = manager
            .check_tool(
                "Bash",
                &serde_json::json!({"command": "ls"}),
                Some("test"),
                Some("/tmp"),
            )
            .await;

        assert!(result.is_allowed());
    }
}
