//! Context Resolver for Six-Layer Context Hierarchy
//!
//! This module provides types and traits for resolving context across
//! six priority layers: Platform, Organization, User, Session, Task, and SubAgent.
//!
//! # Priority Rules
//!
//! - Platform (L1) rules CANNOT be overridden by any layer
//! - Organization (L2) can extend but not override Platform
//! - User (L3) can override Organization but not Platform
//! - Session (L4) can override User but not Platform/Org
//! - Task (L5) can override Session but not higher layers
//! - SubAgent (L6) is most isolated, cannot modify parent context

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Priority levels for context layers (1 = highest, 6 = lowest)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ContextPriority {
    Platform = 1,
    Organization = 2,
    User = 3,
    Session = 4,
    Task = 5,
    SubAgent = 6,
}

impl ContextPriority {
    /// Get the layer name for this priority
    pub fn layer_name(&self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::Organization => "organization",
            Self::User => "user",
            Self::Session => "session",
            Self::Task => "task",
            Self::SubAgent => "subagent",
        }
    }

    /// Check if this priority can override another
    pub fn can_override(&self, other: Self) -> bool {
        *self < other
    }
}

/// Permission mode for tool execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PermissionMode {
    #[default]
    Normal,
    Elevated,
    Restricted,
}

/// The resolved context after merging all layers
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResolvedContext {
    /// Platform-level rules (immutable, highest priority)
    pub platform_rules: String,

    /// Organization conventions (if applicable)
    pub org_conventions: Option<String>,

    /// User preferences
    pub user_preferences: Option<String>,

    /// Memory context (A38) — merged custom instructions, patterns, semantic recall
    pub memory_context: Option<String>,

    /// Session context summary
    pub session_context: Option<String>,

    /// Skill descriptions for system prompt (Layer 1 of two-layer loading)
    pub skill_descriptions: String,

    /// Full content of loaded/invoked skills (Layer 2 of two-layer loading)
    pub loaded_skills: Vec<LoadedSkill>,

    /// Task-specific instructions
    pub task_instructions: Option<String>,

    /// SubAgent context (if applicable)
    pub subagent_context: Option<String>,

    /// Allowed tool names
    pub allowed_tools: Vec<String>,

    /// Blocked tool names
    pub blocked_tools: Vec<String>,

    /// Current permission mode
    pub permission_mode: PermissionMode,

    /// Merged configuration values (lower priority can be overridden by higher)
    pub config_values: HashMap<String, Value>,

    /// Whether English is enforced
    pub enforce_english: bool,

    /// Maximum token limit
    pub max_token_limit: usize,

    /// Active layers that were applied
    pub active_layers: Vec<String>,
}

impl ResolvedContext {
    /// Check if a tool is allowed
    pub fn is_tool_allowed(&self, tool: &str) -> bool {
        // Blocked tools take precedence
        if self.blocked_tools.contains(&tool.to_string()) {
            return false;
        }

        // If allow list is empty, all non-blocked tools are allowed
        if self.allowed_tools.is_empty() {
            return true;
        }

        self.allowed_tools.contains(&tool.to_string())
    }

    /// Get a config value
    pub fn get_config(&self, key: &str) -> Option<&Value> {
        self.config_values.get(key)
    }

    /// Get a config value with a default
    pub fn get_config_or<'a>(&'a self, key: &str, default: &'a Value) -> &'a Value {
        self.config_values.get(key).unwrap_or(default)
    }

    /// Check if a layer was applied
    pub fn has_layer(&self, layer_name: &str) -> bool {
        self.active_layers.contains(&layer_name.to_string())
    }

    /// Get loaded skill by name
    pub fn get_skill(&self, name: &str) -> Option<&LoadedSkill> {
        self.loaded_skills.iter().find(|s| s.name == name)
    }

    /// Check if a skill is loaded
    pub fn has_skill(&self, name: &str) -> bool {
        self.loaded_skills.iter().any(|s| s.name == name)
    }

    /// Generate full system prompt from resolved context
    pub fn generate_system_prompt(&self) -> String {
        let mut prompt = String::new();

        // 1. Platform rules (always first)
        if !self.platform_rules.is_empty() {
            prompt.push_str(&self.platform_rules);
            prompt.push_str("\n\n");
        }

        // 2. Organization conventions
        if let Some(org) = &self.org_conventions {
            prompt.push_str(org);
            prompt.push_str("\n\n");
        }

        // 3. User preferences
        if let Some(user) = &self.user_preferences {
            prompt.push_str(user);
            prompt.push_str("\n\n");
        }

        // 4. Skill descriptions (Layer 1)
        if !self.skill_descriptions.is_empty() {
            prompt.push_str(&self.skill_descriptions);
            prompt.push_str("\n\n");
        }

        // 5. Loaded skill content (Layer 2)
        for skill in &self.loaded_skills {
            prompt.push_str(&format!("## [{}] Skill Loaded\n\n", skill.name));
            prompt.push_str(&skill.content);
            prompt.push_str("\n\n");
        }

        // 6. Task instructions
        if let Some(task) = &self.task_instructions {
            prompt.push_str(task);
            prompt.push_str("\n\n");
        }

        // 7. SubAgent context
        if let Some(subagent) = &self.subagent_context {
            prompt.push_str(subagent);
        }

        prompt.trim_end().to_string()
    }
}

/// A loaded skill with full content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedSkill {
    pub name: String,
    pub content: String,
    pub requires_browser: bool,
    pub automation_tab: bool,
}

/// Trait for context layers
pub trait ContextLayer: Send + Sync {
    /// Layer name for identification
    fn layer_name(&self) -> &str;

    /// Priority (lower = higher priority)
    fn priority(&self) -> ContextPriority;

    /// Apply this layer to the resolved context
    fn apply_to(&self, resolved: &mut ResolvedContext);

    /// Check if a config key can be overridden by this layer
    fn can_override(&self, _key: &str, current_priority: ContextPriority) -> bool {
        // Higher priority layers (lower number) cannot be overridden
        self.priority() < current_priority
    }
}

/// Context resolver that merges all layers
pub struct ContextResolver {
    /// Track which priority set each config key
    key_priorities: HashMap<String, ContextPriority>,
}

impl Default for ContextResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextResolver {
    pub fn new() -> Self {
        Self {
            key_priorities: HashMap::new(),
        }
    }

    /// Reset the resolver state
    pub fn reset(&mut self) {
        self.key_priorities.clear();
    }

    /// Resolve context by applying layers in priority order
    pub fn resolve(&mut self, layers: &[&dyn ContextLayer]) -> ResolvedContext {
        self.reset();
        let mut resolved = ResolvedContext::default();

        // Sort layers by priority (Platform first)
        let mut sorted_layers: Vec<_> = layers.iter().collect();
        sorted_layers.sort_by_key(|l| l.priority());

        // Apply each layer
        for layer in sorted_layers {
            layer.apply_to(&mut resolved);
            resolved.active_layers.push(layer.layer_name().to_string());
        }

        resolved
    }

    /// Resolve context with typed layers for better ergonomics
    pub fn resolve_full(
        &mut self,
        platform: Option<&dyn ContextLayer>,
        organization: Option<&dyn ContextLayer>,
        user: Option<&dyn ContextLayer>,
        session: Option<&dyn ContextLayer>,
        task: Option<&dyn ContextLayer>,
        subagent: Option<&dyn ContextLayer>,
    ) -> ResolvedContext {
        let mut layers: Vec<&dyn ContextLayer> = Vec::new();

        if let Some(l) = platform {
            layers.push(l);
        }
        if let Some(l) = organization {
            layers.push(l);
        }
        if let Some(l) = user {
            layers.push(l);
        }
        if let Some(l) = session {
            layers.push(l);
        }
        if let Some(l) = task {
            layers.push(l);
        }
        if let Some(l) = subagent {
            layers.push(l);
        }

        self.resolve(&layers)
    }

    /// Set a config value, respecting priority
    pub fn set_config(
        &mut self,
        resolved: &mut ResolvedContext,
        key: &str,
        value: Value,
        priority: ContextPriority,
    ) {
        // Check if we can override
        if let Some(&existing_priority) = self.key_priorities.get(key) {
            if priority >= existing_priority {
                // Cannot override higher priority
                return;
            }
        }

        resolved.config_values.insert(key.to_string(), value);
        self.key_priorities.insert(key.to_string(), priority);
    }

    /// Extend config (add without overriding existing keys)
    pub fn extend_config(
        &mut self,
        resolved: &mut ResolvedContext,
        values: &HashMap<String, Value>,
        priority: ContextPriority,
    ) {
        for (key, value) in values {
            if !self.key_priorities.contains_key(key) {
                resolved.config_values.insert(key.clone(), value.clone());
                self.key_priorities.insert(key.clone(), priority);
            }
        }
    }

    /// Merge tools from a layer, respecting allow/block semantics
    pub fn merge_tools(
        &self,
        resolved: &mut ResolvedContext,
        allowed: &[String],
        blocked: &[String],
    ) {
        // Add blocked tools (union)
        for tool in blocked {
            if !resolved.blocked_tools.contains(tool) {
                resolved.blocked_tools.push(tool.clone());
            }
        }

        // For allowed tools:
        // - If resolved has no allowed tools, use the layer's allowed tools
        // - Otherwise, intersect with the layer's allowed tools
        if !allowed.is_empty() {
            if resolved.allowed_tools.is_empty() {
                resolved.allowed_tools = allowed.to_vec();
            } else {
                resolved.allowed_tools.retain(|t| allowed.contains(t));
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Mock context layer for testing
    struct MockLayer {
        name: String,
        priority: ContextPriority,
        rules: String,
    }

    impl MockLayer {
        fn new(name: &str, priority: ContextPriority, rules: &str) -> Self {
            Self {
                name: name.to_string(),
                priority,
                rules: rules.to_string(),
            }
        }
    }

    impl ContextLayer for MockLayer {
        fn layer_name(&self) -> &str {
            &self.name
        }

        fn priority(&self) -> ContextPriority {
            self.priority
        }

        fn apply_to(&self, resolved: &mut ResolvedContext) {
            match self.priority {
                ContextPriority::Platform => {
                    resolved.platform_rules = self.rules.clone();
                }
                ContextPriority::Organization => {
                    resolved.org_conventions = Some(self.rules.clone());
                }
                ContextPriority::User => {
                    resolved.user_preferences = Some(self.rules.clone());
                }
                ContextPriority::Session => {
                    resolved.session_context = Some(self.rules.clone());
                }
                ContextPriority::Task => {
                    resolved.task_instructions = Some(self.rules.clone());
                }
                ContextPriority::SubAgent => {
                    resolved.subagent_context = Some(self.rules.clone());
                }
            }
        }
    }

    #[test]
    fn test_priority_ordering() {
        assert!(ContextPriority::Platform < ContextPriority::Organization);
        assert!(ContextPriority::Organization < ContextPriority::User);
        assert!(ContextPriority::User < ContextPriority::Session);
        assert!(ContextPriority::Session < ContextPriority::Task);
        assert!(ContextPriority::Task < ContextPriority::SubAgent);
    }

    #[test]
    fn test_priority_can_override() {
        assert!(ContextPriority::Platform.can_override(ContextPriority::Organization));
        assert!(ContextPriority::Platform.can_override(ContextPriority::User));
        assert!(!ContextPriority::User.can_override(ContextPriority::Platform));
        assert!(!ContextPriority::SubAgent.can_override(ContextPriority::Task));
    }

    #[test]
    fn test_priority_layer_name() {
        assert_eq!(ContextPriority::Platform.layer_name(), "platform");
        assert_eq!(ContextPriority::SubAgent.layer_name(), "subagent");
    }

    #[test]
    fn test_resolved_context_default() {
        let ctx = ResolvedContext::default();
        assert!(ctx.platform_rules.is_empty());
        assert_eq!(ctx.permission_mode, PermissionMode::Normal);
        assert!(ctx.active_layers.is_empty());
    }

    #[test]
    fn test_resolved_context_is_tool_allowed() {
        let mut ctx = ResolvedContext::default();

        // No restrictions = all allowed
        assert!(ctx.is_tool_allowed("read"));
        assert!(ctx.is_tool_allowed("bash"));

        // Block a tool
        ctx.blocked_tools.push("bash".to_string());
        assert!(!ctx.is_tool_allowed("bash"));
        assert!(ctx.is_tool_allowed("read"));

        // Set allow list
        ctx.allowed_tools.push("read".to_string());
        ctx.allowed_tools.push("write".to_string());
        assert!(ctx.is_tool_allowed("read"));
        assert!(!ctx.is_tool_allowed("glob")); // Not in allow list
        assert!(!ctx.is_tool_allowed("bash")); // Blocked takes precedence
    }

    #[test]
    fn test_resolved_context_get_config() {
        let mut ctx = ResolvedContext::default();
        ctx.config_values
            .insert("key1".to_string(), serde_json::json!("value1"));

        assert_eq!(ctx.get_config("key1"), Some(&serde_json::json!("value1")));
        assert_eq!(ctx.get_config("key2"), None);

        let default = serde_json::json!("default");
        assert_eq!(ctx.get_config_or("key2", &default), &default);
    }

    #[test]
    fn test_resolved_context_has_layer() {
        let mut ctx = ResolvedContext::default();
        ctx.active_layers.push("platform".to_string());
        ctx.active_layers.push("user".to_string());

        assert!(ctx.has_layer("platform"));
        assert!(ctx.has_layer("user"));
        assert!(!ctx.has_layer("organization"));
    }

    #[test]
    fn test_resolved_context_skills() {
        let mut ctx = ResolvedContext::default();
        ctx.loaded_skills.push(LoadedSkill {
            name: "skill1".to_string(),
            content: "content1".to_string(),
            requires_browser: false,
            automation_tab: false,
        });

        assert!(ctx.has_skill("skill1"));
        assert!(!ctx.has_skill("skill2"));

        let skill = ctx.get_skill("skill1");
        assert!(skill.is_some());
        assert_eq!(skill.unwrap().content, "content1");
    }

    #[test]
    fn test_resolver_resolve() {
        let platform = MockLayer::new("platform", ContextPriority::Platform, "Platform rules");
        let user = MockLayer::new("user", ContextPriority::User, "User prefs");
        let task = MockLayer::new("task", ContextPriority::Task, "Task instructions");

        let mut resolver = ContextResolver::new();
        let resolved = resolver.resolve(&[&task, &platform, &user]); // Out of order

        // Should be applied in priority order
        assert_eq!(resolved.platform_rules, "Platform rules");
        assert_eq!(resolved.user_preferences, Some("User prefs".to_string()));
        assert_eq!(
            resolved.task_instructions,
            Some("Task instructions".to_string())
        );

        // Active layers tracked
        assert_eq!(resolved.active_layers.len(), 3);
        assert_eq!(resolved.active_layers[0], "platform");
        assert_eq!(resolved.active_layers[1], "user");
        assert_eq!(resolved.active_layers[2], "task");
    }

    #[test]
    fn test_resolver_resolve_full() {
        let platform = MockLayer::new("platform", ContextPriority::Platform, "Platform");
        let org = MockLayer::new("org", ContextPriority::Organization, "Org");
        let user = MockLayer::new("user", ContextPriority::User, "User");
        let session = MockLayer::new("session", ContextPriority::Session, "Session");
        let task = MockLayer::new("task", ContextPriority::Task, "Task");
        let subagent = MockLayer::new("subagent", ContextPriority::SubAgent, "SubAgent");

        let mut resolver = ContextResolver::new();
        let resolved = resolver.resolve_full(
            Some(&platform),
            Some(&org),
            Some(&user),
            Some(&session),
            Some(&task),
            Some(&subagent),
        );

        assert_eq!(resolved.platform_rules, "Platform");
        assert_eq!(resolved.org_conventions, Some("Org".to_string()));
        assert_eq!(resolved.user_preferences, Some("User".to_string()));
        assert_eq!(resolved.session_context, Some("Session".to_string()));
        assert_eq!(resolved.task_instructions, Some("Task".to_string()));
        assert_eq!(resolved.subagent_context, Some("SubAgent".to_string()));
        assert_eq!(resolved.active_layers.len(), 6);
    }

    #[test]
    fn test_resolver_resolve_partial() {
        let platform = MockLayer::new("platform", ContextPriority::Platform, "Platform");
        let task = MockLayer::new("task", ContextPriority::Task, "Task");

        let mut resolver = ContextResolver::new();
        let resolved = resolver.resolve_full(
            Some(&platform),
            None, // No org
            None, // No user
            None, // No session
            Some(&task),
            None, // No subagent
        );

        assert_eq!(resolved.platform_rules, "Platform");
        assert!(resolved.org_conventions.is_none());
        assert!(resolved.user_preferences.is_none());
        assert_eq!(resolved.task_instructions, Some("Task".to_string()));
        assert_eq!(resolved.active_layers.len(), 2);
    }

    #[test]
    fn test_resolver_set_config() {
        let mut resolver = ContextResolver::new();
        let mut resolved = ResolvedContext::default();

        // Set from platform (highest priority)
        resolver.set_config(
            &mut resolved,
            "key1",
            serde_json::json!("platform"),
            ContextPriority::Platform,
        );
        assert_eq!(resolved.config_values["key1"], "platform");

        // Try to override from user (lower priority) - should fail
        resolver.set_config(
            &mut resolved,
            "key1",
            serde_json::json!("user"),
            ContextPriority::User,
        );
        assert_eq!(resolved.config_values["key1"], "platform"); // Unchanged

        // Set new key from user
        resolver.set_config(
            &mut resolved,
            "key2",
            serde_json::json!("user"),
            ContextPriority::User,
        );
        assert_eq!(resolved.config_values["key2"], "user");
    }

    #[test]
    fn test_resolver_extend_config() {
        let mut resolver = ContextResolver::new();
        let mut resolved = ResolvedContext::default();

        // Set initial value
        resolver.set_config(
            &mut resolved,
            "key1",
            serde_json::json!("original"),
            ContextPriority::Platform,
        );

        // Extend with new values
        let mut values = HashMap::new();
        values.insert("key1".to_string(), serde_json::json!("new1")); // Won't override
        values.insert("key2".to_string(), serde_json::json!("new2")); // Will add

        resolver.extend_config(&mut resolved, &values, ContextPriority::User);

        assert_eq!(resolved.config_values["key1"], "original"); // Not overridden
        assert_eq!(resolved.config_values["key2"], "new2"); // Added
    }

    #[test]
    fn test_resolver_merge_tools() {
        let resolver = ContextResolver::new();
        let mut resolved = ResolvedContext::default();

        // First layer sets allowed tools
        resolver.merge_tools(
            &mut resolved,
            &["read".to_string(), "write".to_string()],
            &[],
        );
        assert_eq!(resolved.allowed_tools, vec!["read", "write"]);

        // Second layer restricts to intersection
        resolver.merge_tools(
            &mut resolved,
            &["read".to_string(), "glob".to_string()],
            &[],
        );
        assert_eq!(resolved.allowed_tools, vec!["read"]); // Intersection

        // Block a tool
        resolver.merge_tools(&mut resolved, &[], &["bash".to_string()]);
        assert!(resolved.blocked_tools.contains(&"bash".to_string()));
    }

    #[test]
    fn test_resolver_reset() {
        let mut resolver = ContextResolver::new();
        let mut resolved = ResolvedContext::default();

        resolver.set_config(
            &mut resolved,
            "key1",
            serde_json::json!("value"),
            ContextPriority::Platform,
        );
        assert!(!resolver.key_priorities.is_empty());

        resolver.reset();
        assert!(resolver.key_priorities.is_empty());
    }

    #[test]
    fn test_generate_system_prompt() {
        let mut ctx = ResolvedContext::default();
        ctx.platform_rules = "## Platform\nUse English.".to_string();
        ctx.org_conventions = Some("## Organization\nFollow team standards.".to_string());
        ctx.user_preferences = Some("## User\nPrefer explicit types.".to_string());
        ctx.skill_descriptions = "## Skills\n- commit: Create commits".to_string();
        ctx.loaded_skills.push(LoadedSkill {
            name: "commit".to_string(),
            content: "Full commit skill content".to_string(),
            requires_browser: false,
            automation_tab: false,
        });
        ctx.task_instructions = Some("## Task\nFix the bug.".to_string());

        let prompt = ctx.generate_system_prompt();

        assert!(prompt.contains("## Platform"));
        assert!(prompt.contains("Use English"));
        assert!(prompt.contains("## Organization"));
        assert!(prompt.contains("## User"));
        assert!(prompt.contains("## Skills"));
        assert!(prompt.contains("[commit] Skill Loaded"));
        assert!(prompt.contains("Full commit skill content"));
        assert!(prompt.contains("## Task"));
        assert!(prompt.contains("Fix the bug"));

        // Verify order: platform should come before organization
        let platform_pos = prompt.find("## Platform").unwrap();
        let org_pos = prompt.find("## Organization").unwrap();
        assert!(platform_pos < org_pos);
    }
}
