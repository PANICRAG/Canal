//! SubAgent Context Manager
//!
//! Manages sub-agent level context with three context fork modes:
//! - None: Complete isolation (no parent context)
//! - Inherit: Read-only access to parent context
//! - Fork: Independent copy of parent context
//!
//! SubAgent context is the lowest priority layer and cannot modify parent context.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use super::resolver::{
    ContextLayer, ContextPriority, LoadedSkill, PermissionMode, ResolvedContext,
};
use super::session::SessionContext;

// ============================================================================
// SubAgent Context Types
// ============================================================================

/// Mode for context inheritance from parent
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ContextForkMode {
    /// Complete isolation - no parent context access
    #[default]
    None,
    /// Read-only access to parent context
    Inherit,
    /// Independent copy of parent context
    Fork,
}

/// SubAgent-level context configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentContext {
    /// SubAgent unique identifier
    pub subagent_id: String,

    /// Agent type/role
    pub agent_type: String,

    /// Parent session ID
    pub parent_session_id: Option<Uuid>,

    /// Context fork mode
    pub fork_mode: ContextForkMode,

    /// Forked parent context (if fork mode is Fork)
    pub forked_context: Option<ForkedContext>,

    /// SubAgent-specific instructions
    pub specific_instructions: String,

    /// Tools allowed for this subagent
    pub allowed_tools: Vec<String>,

    /// Tools blocked for this subagent
    pub blocked_tools: Vec<String>,

    /// Permission mode for this subagent
    pub permission_mode: PermissionMode,

    /// Maximum turns/iterations allowed
    pub max_turns: Option<u32>,

    /// Custom subagent configuration
    pub config: HashMap<String, serde_json::Value>,
}

/// Forked context from parent (immutable copy)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkedContext {
    /// Snapshot of parent's working files
    pub working_files: Vec<String>,

    /// Snapshot of parent's loaded skills
    pub loaded_skills: Vec<LoadedSkill>,

    /// Snapshot of parent's discoveries
    pub discoveries: Vec<String>,

    /// Snapshot of parent's custom context
    pub custom_context: HashMap<String, serde_json::Value>,

    /// Timestamp when forked
    pub forked_at: chrono::DateTime<chrono::Utc>,

    /// Parent task description (if available)
    pub task_description: Option<String>,
}

impl Default for ForkedContext {
    fn default() -> Self {
        Self {
            working_files: Vec::new(),
            loaded_skills: Vec::new(),
            discoveries: Vec::new(),
            custom_context: HashMap::new(),
            forked_at: chrono::Utc::now(),
            task_description: None,
        }
    }
}

impl Default for SubAgentContext {
    fn default() -> Self {
        Self {
            subagent_id: Uuid::new_v4().to_string(),
            agent_type: "general".to_string(),
            parent_session_id: None,
            fork_mode: ContextForkMode::None,
            forked_context: None,
            specific_instructions: String::new(),
            allowed_tools: Vec::new(),
            blocked_tools: Vec::new(),
            permission_mode: PermissionMode::Normal,
            max_turns: None,
            config: HashMap::new(),
        }
    }
}

impl SubAgentContext {
    /// Create a new isolated subagent context
    pub fn new(agent_type: impl Into<String>) -> Self {
        Self {
            agent_type: agent_type.into(),
            fork_mode: ContextForkMode::None,
            ..Default::default()
        }
    }

    /// Create a subagent that inherits parent context (read-only)
    pub fn inherit(agent_type: impl Into<String>, parent_session_id: Uuid) -> Self {
        Self {
            agent_type: agent_type.into(),
            parent_session_id: Some(parent_session_id),
            fork_mode: ContextForkMode::Inherit,
            ..Default::default()
        }
    }

    /// Create a subagent with forked parent context
    pub fn fork(agent_type: impl Into<String>, parent: &SessionContext) -> Self {
        let forked = ForkedContext {
            working_files: parent.working_files.keys().cloned().collect(),
            loaded_skills: parent.loaded_skills.clone(),
            discoveries: Vec::new(), // Would need to extract from session
            custom_context: parent.custom_context.clone(),
            forked_at: chrono::Utc::now(),
            task_description: None,
        };

        Self {
            agent_type: agent_type.into(),
            parent_session_id: Some(parent.session_id),
            fork_mode: ContextForkMode::Fork,
            forked_context: Some(forked),
            ..Default::default()
        }
    }

    /// Set specific instructions for this subagent
    pub fn set_instructions(&mut self, instructions: impl Into<String>) {
        self.specific_instructions = instructions.into();
    }

    /// Allow a specific tool
    pub fn allow_tool(&mut self, tool: impl Into<String>) {
        self.allowed_tools.push(tool.into());
    }

    /// Block a specific tool
    pub fn block_tool(&mut self, tool: impl Into<String>) {
        self.blocked_tools.push(tool.into());
    }

    /// Set max turns
    pub fn set_max_turns(&mut self, max: u32) {
        self.max_turns = Some(max);
    }

    /// Set permission mode
    pub fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.permission_mode = mode;
    }

    /// Set a configuration value
    pub fn set_config(&mut self, key: impl Into<String>, value: serde_json::Value) {
        self.config.insert(key.into(), value);
    }

    /// Get a configuration value
    pub fn get_config(&self, key: &str) -> Option<&serde_json::Value> {
        self.config.get(key)
    }

    /// Check if this subagent has access to parent context
    pub fn has_parent_access(&self) -> bool {
        matches!(
            self.fork_mode,
            ContextForkMode::Inherit | ContextForkMode::Fork
        )
    }

    /// Check if a tool is allowed for this subagent
    pub fn is_tool_allowed(&self, tool: &str) -> bool {
        // Blocked tools take precedence
        if self.blocked_tools.contains(&tool.to_string()) {
            return false;
        }

        // If allow list is empty, all (non-blocked) tools are allowed
        if self.allowed_tools.is_empty() {
            return true;
        }

        // Check if in allow list
        self.allowed_tools.contains(&tool.to_string())
    }
}

// ============================================================================
// ContextLayer Implementation
// ============================================================================

impl ContextLayer for SubAgentContext {
    fn layer_name(&self) -> &str {
        "subagent"
    }

    fn priority(&self) -> ContextPriority {
        ContextPriority::SubAgent
    }

    fn apply_to(&self, resolved: &mut ResolvedContext) {
        // Apply forked context if present
        if let Some(forked) = &self.forked_context {
            // Add forked skills
            for skill in &forked.loaded_skills {
                if !resolved.loaded_skills.iter().any(|s| s.name == skill.name) {
                    resolved.loaded_skills.push(skill.clone());
                }
            }

            // Merge forked custom context into config_values
            for (key, value) in &forked.custom_context {
                if !resolved.config_values.contains_key(key) {
                    resolved.config_values.insert(key.clone(), value.clone());
                }
            }
        }

        // Apply tool restrictions
        if !self.allowed_tools.is_empty() {
            // Only allow intersection of current and subagent allowed tools
            if resolved.allowed_tools.is_empty() {
                resolved.allowed_tools = self.allowed_tools.clone();
            } else {
                resolved
                    .allowed_tools
                    .retain(|t| self.allowed_tools.contains(t));
            }
        }

        // Add blocked tools
        for tool in &self.blocked_tools {
            if !resolved.blocked_tools.contains(tool) {
                resolved.blocked_tools.push(tool.clone());
            }
        }

        // Set permission mode
        resolved.permission_mode = self.permission_mode;

        // Build subagent instructions
        let mut instructions = String::new();

        instructions.push_str(&format!("## SubAgent: {}\n\n", self.agent_type));

        if !self.specific_instructions.is_empty() {
            instructions.push_str(&self.specific_instructions);
            instructions.push_str("\n\n");
        }

        // Add context mode info
        instructions.push_str("### Context Mode\n\n");
        match self.fork_mode {
            ContextForkMode::None => {
                instructions.push_str("- Isolated execution (no parent context access)\n");
            }
            ContextForkMode::Inherit => {
                instructions.push_str("- Inheriting parent context (read-only)\n");
            }
            ContextForkMode::Fork => {
                instructions.push_str("- Forked parent context (independent copy)\n");
                if let Some(forked) = &self.forked_context {
                    instructions.push_str(&format!(
                        "- Forked at: {}\n",
                        forked.forked_at.format("%Y-%m-%d %H:%M:%S UTC")
                    ));
                    if !forked.working_files.is_empty() {
                        instructions.push_str(&format!(
                            "- Inherited {} working files\n",
                            forked.working_files.len()
                        ));
                    }
                    if !forked.loaded_skills.is_empty() {
                        instructions.push_str(&format!(
                            "- Inherited {} skills\n",
                            forked.loaded_skills.len()
                        ));
                    }
                }
            }
        }

        // Add constraints
        if let Some(max) = self.max_turns {
            instructions.push_str(&format!("\n- Max turns: {}\n", max));
        }

        // Append to task instructions
        if let Some(existing) = &resolved.task_instructions {
            resolved.task_instructions = Some(format!("{}\n\n{}", existing, instructions));
        } else {
            resolved.task_instructions = Some(instructions);
        }
    }
}

// ============================================================================
// Builder
// ============================================================================

/// Builder for SubAgentContext
pub struct SubAgentContextBuilder {
    context: SubAgentContext,
}

impl SubAgentContextBuilder {
    /// Create a new builder
    pub fn new(agent_type: impl Into<String>) -> Self {
        Self {
            context: SubAgentContext::new(agent_type),
        }
    }

    /// Set subagent ID
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.context.subagent_id = id.into();
        self
    }

    /// Set parent session
    pub fn parent(mut self, session_id: Uuid) -> Self {
        self.context.parent_session_id = Some(session_id);
        self
    }

    /// Set fork mode
    pub fn fork_mode(mut self, mode: ContextForkMode) -> Self {
        self.context.fork_mode = mode;
        self
    }

    /// Set forked context
    pub fn forked_context(mut self, forked: ForkedContext) -> Self {
        self.context.forked_context = Some(forked);
        self.context.fork_mode = ContextForkMode::Fork;
        self
    }

    /// Set instructions
    pub fn instructions(mut self, instructions: impl Into<String>) -> Self {
        self.context.specific_instructions = instructions.into();
        self
    }

    /// Allow a tool
    pub fn allow_tool(mut self, tool: impl Into<String>) -> Self {
        self.context.allowed_tools.push(tool.into());
        self
    }

    /// Block a tool
    pub fn block_tool(mut self, tool: impl Into<String>) -> Self {
        self.context.blocked_tools.push(tool.into());
        self
    }

    /// Set permission mode
    pub fn permission_mode(mut self, mode: PermissionMode) -> Self {
        self.context.permission_mode = mode;
        self
    }

    /// Set max turns
    pub fn max_turns(mut self, max: u32) -> Self {
        self.context.max_turns = Some(max);
        self
    }

    /// Set config value
    pub fn config(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.context.config.insert(key.into(), value);
        self
    }

    /// Build the context
    pub fn build(self) -> SubAgentContext {
        self.context
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subagent_context_default() {
        let ctx = SubAgentContext::default();
        assert!(!ctx.subagent_id.is_empty());
        assert_eq!(ctx.agent_type, "general");
        assert_eq!(ctx.fork_mode, ContextForkMode::None);
        assert!(ctx.forked_context.is_none());
    }

    #[test]
    fn test_subagent_context_new() {
        let ctx = SubAgentContext::new("explorer");
        assert_eq!(ctx.agent_type, "explorer");
        assert_eq!(ctx.fork_mode, ContextForkMode::None);
    }

    #[test]
    fn test_subagent_context_inherit() {
        let session_id = Uuid::new_v4();
        let ctx = SubAgentContext::inherit("planner", session_id);

        assert_eq!(ctx.agent_type, "planner");
        assert_eq!(ctx.parent_session_id, Some(session_id));
        assert_eq!(ctx.fork_mode, ContextForkMode::Inherit);
        assert!(ctx.has_parent_access());
    }

    #[test]
    fn test_subagent_context_fork() {
        let mut session = SessionContext::default();
        session.track_file("/path/to/file.rs");
        session.load_skill(LoadedSkill {
            name: "test-skill".to_string(),
            content: "content".to_string(),
            requires_browser: false,
            automation_tab: false,
        });

        let ctx = SubAgentContext::fork("worker", &session);

        assert_eq!(ctx.agent_type, "worker");
        assert_eq!(ctx.parent_session_id, Some(session.session_id));
        assert_eq!(ctx.fork_mode, ContextForkMode::Fork);
        assert!(ctx.forked_context.is_some());

        let forked = ctx.forked_context.as_ref().unwrap();
        assert!(forked
            .working_files
            .contains(&"/path/to/file.rs".to_string()));
        assert_eq!(forked.loaded_skills.len(), 1);
    }

    #[test]
    fn test_tool_restrictions() {
        let mut ctx = SubAgentContext::new("restricted");

        // No restrictions = all allowed
        assert!(ctx.is_tool_allowed("read"));
        assert!(ctx.is_tool_allowed("bash"));

        // Block a tool
        ctx.block_tool("bash");
        assert!(!ctx.is_tool_allowed("bash"));
        assert!(ctx.is_tool_allowed("read"));

        // Set allow list
        ctx.allow_tool("read");
        ctx.allow_tool("glob");
        assert!(ctx.is_tool_allowed("read"));
        assert!(!ctx.is_tool_allowed("write")); // Not in allow list
        assert!(!ctx.is_tool_allowed("bash")); // Blocked takes precedence
    }

    #[test]
    fn test_context_layer_metadata() {
        let ctx = SubAgentContext::default();
        assert_eq!(ctx.layer_name(), "subagent");
        assert_eq!(ctx.priority(), ContextPriority::SubAgent);
    }

    #[test]
    fn test_apply_to_isolated() {
        let ctx = SubAgentContext::new("isolated");

        let mut resolved = ResolvedContext::default();
        ctx.apply_to(&mut resolved);

        assert!(resolved.task_instructions.is_some());
        let instructions = resolved.task_instructions.unwrap();
        assert!(instructions.contains("SubAgent: isolated"));
        assert!(instructions.contains("Isolated execution"));
    }

    #[test]
    fn test_apply_to_with_fork() {
        let mut session = SessionContext::default();
        session.load_skill(LoadedSkill {
            name: "forked-skill".to_string(),
            content: "content".to_string(),
            requires_browser: false,
            automation_tab: false,
        });

        let ctx = SubAgentContext::fork("forked-agent", &session);

        let mut resolved = ResolvedContext::default();
        ctx.apply_to(&mut resolved);

        assert_eq!(resolved.loaded_skills.len(), 1);
        assert_eq!(resolved.loaded_skills[0].name, "forked-skill");

        let instructions = resolved.task_instructions.unwrap();
        assert!(instructions.contains("Forked parent context"));
    }

    #[test]
    fn test_apply_to_with_restrictions() {
        let mut ctx = SubAgentContext::new("restricted");
        ctx.allow_tool("read");
        ctx.allow_tool("glob");
        ctx.block_tool("bash");
        ctx.set_max_turns(5);
        ctx.set_permission_mode(PermissionMode::Restricted);

        let mut resolved = ResolvedContext::default();
        ctx.apply_to(&mut resolved);

        assert!(resolved.allowed_tools.contains(&"read".to_string()));
        assert!(resolved.blocked_tools.contains(&"bash".to_string()));
        assert_eq!(resolved.permission_mode, PermissionMode::Restricted);
    }

    #[test]
    fn test_builder() {
        let ctx = SubAgentContextBuilder::new("explorer")
            .id("sub-1")
            .parent(Uuid::new_v4())
            .fork_mode(ContextForkMode::Inherit)
            .instructions("Find relevant files")
            .allow_tool("glob")
            .allow_tool("grep")
            .block_tool("write")
            .max_turns(10)
            .config("depth", serde_json::json!(3))
            .build();

        assert_eq!(ctx.subagent_id, "sub-1");
        assert_eq!(ctx.agent_type, "explorer");
        assert_eq!(ctx.fork_mode, ContextForkMode::Inherit);
        assert_eq!(ctx.specific_instructions, "Find relevant files");
        assert_eq!(ctx.allowed_tools.len(), 2);
        assert_eq!(ctx.blocked_tools.len(), 1);
        assert_eq!(ctx.max_turns, Some(10));
        assert_eq!(ctx.get_config("depth"), Some(&serde_json::json!(3)));
    }

    #[test]
    fn test_serde_round_trip() {
        let ctx = SubAgentContext::new("test-agent");

        let json = serde_json::to_string(&ctx).expect("serialize");
        let restored: SubAgentContext = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(ctx.subagent_id, restored.subagent_id);
        assert_eq!(ctx.agent_type, restored.agent_type);
        assert_eq!(ctx.fork_mode, restored.fork_mode);
    }

    #[test]
    fn test_fork_mode_serde() {
        assert_eq!(
            serde_json::to_string(&ContextForkMode::None).unwrap(),
            "\"None\""
        );
        assert_eq!(
            serde_json::to_string(&ContextForkMode::Inherit).unwrap(),
            "\"Inherit\""
        );
        assert_eq!(
            serde_json::to_string(&ContextForkMode::Fork).unwrap(),
            "\"Fork\""
        );
    }

    #[test]
    fn test_has_parent_access() {
        let isolated = SubAgentContext::new("isolated");
        assert!(!isolated.has_parent_access());

        let inherit = SubAgentContext::inherit("inherit", Uuid::new_v4());
        assert!(inherit.has_parent_access());

        let session = SessionContext::default();
        let fork = SubAgentContext::fork("fork", &session);
        assert!(fork.has_parent_access());
    }
}
