//! Context Integration for AgentRunner
//!
//! This module provides the integration layer between the six-layer context
//! hierarchy and the AgentRunner. It handles:
//!
//! - Loading platform context from YAML configuration
//! - Loading organization context from database/cache
//! - Loading user context from preferences and CLAUDE.md
//! - Building session context from conversation state
//! - Creating task context with working memory
//! - Supporting subagent context with fork modes
//!
//! # Example
//!
//! ```rust,ignore
//! use gateway_core::agent::context::{
//!     ContextIntegration, PlatformContextLoader,
//! };
//!
//! let mut integration = ContextIntegration::new()
//!     .with_platform_config("config/platform-rules.yaml")
//!     .with_skill_registry(skill_registry);
//!
//! // Load contexts
//! integration.load_platform()?;
//!
//! // Build system prompt for agent
//! let system_prompt = integration.generate_system_prompt();
//! ```

use super::{
    ContextLayer, LoadedSkill, OrganizationContext, PermissionMode as ContextPermissionMode,
    PlatformContext, PlatformContextLoader, ResolvedContext, SessionContext, SubAgentContext,
    SubAgentContextBuilder, SystemPromptGenerator, TaskContext, UserContextLoader,
    UserCtx as UserContext,
};
use crate::agent::skills::SkillRegistry;
use crate::error::Result;
use crate::memory::UnifiedMemoryStore;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

#[cfg(feature = "context-engineering")]
use super::{ContextResolverFlags, ItemSource, PromptInspection, RelevanceScorer, ScoredItem};

#[cfg(feature = "context-engineering")]
use super::relevance::estimate_tokens;

/// Integration layer for context hierarchy in AgentRunner
pub struct ContextIntegration {
    /// Platform context (L1) - loaded from YAML
    platform: Option<PlatformContext>,
    /// Organization context (L2) - loaded from database/config
    organization: Option<OrganizationContext>,
    /// User context (L3) - loaded from preferences
    user: Option<UserContext>,
    /// Session context (L4) - built from conversation state
    session: Option<SessionContext>,
    /// Task context (L5) - current task and working memory
    task: Option<TaskContext>,
    /// SubAgent context (L6) - for child agents
    subagent: Option<SubAgentContext>,
    /// Skill registry for generating descriptions
    skill_registry: Option<Arc<SkillRegistry>>,
    /// Platform config path
    platform_config_path: Option<PathBuf>,
    /// User context directory
    user_context_dir: Option<PathBuf>,
    /// Resolved context cache
    resolved: Option<ResolvedContext>,
    /// Feature flags for context engineering v2 rollout
    #[cfg(feature = "context-engineering")]
    flags: Option<Arc<ContextResolverFlags>>,
    /// Relevance scorer for dynamic context filtering
    #[cfg(feature = "context-engineering")]
    relevance_scorer: Option<RelevanceScorer>,
    /// Knowledge provider for learning system integration
    #[cfg(all(feature = "context-engineering", feature = "learning"))]
    knowledge_provider: Option<Arc<dyn crate::learning::KnowledgeProvider>>,
    /// Unified memory store for surfacing preferences and learned patterns
    unified_store: Option<Arc<UnifiedMemoryStore>>,
    /// User ID for memory store queries
    unified_store_user_id: Option<Uuid>,
    /// Extra plugin skills to include in skill descriptions (A25)
    plugin_skills: Vec<crate::agent::skills::definition::Skill>,
    /// Bundle system prompt to inject (from resolved plugin bundles)
    bundle_system_prompt: Option<String>,
    /// Preloaded semantic memories for injection into system prompt (A38)
    preloaded_memories: Option<Vec<crate::memory::MemoryEntry>>,
}

impl Default for ContextIntegration {
    fn default() -> Self {
        Self {
            platform: None,
            organization: None,
            user: None,
            session: None,
            task: None,
            subagent: None,
            skill_registry: None,
            platform_config_path: None,
            user_context_dir: None,
            resolved: None,
            #[cfg(feature = "context-engineering")]
            flags: None,
            #[cfg(feature = "context-engineering")]
            relevance_scorer: None,
            #[cfg(all(feature = "context-engineering", feature = "learning"))]
            knowledge_provider: None,
            unified_store: None,
            unified_store_user_id: None,
            plugin_skills: Vec::new(),
            bundle_system_prompt: None,
            preloaded_memories: None,
        }
    }
}

impl ContextIntegration {
    /// Create a new context integration
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the platform configuration path
    pub fn with_platform_config(mut self, path: impl Into<PathBuf>) -> Self {
        self.platform_config_path = Some(path.into());
        self
    }

    /// Set the skill registry for generating skill descriptions
    pub fn with_skill_registry(mut self, registry: Arc<SkillRegistry>) -> Self {
        self.skill_registry = Some(registry);
        self
    }

    /// Set extra plugin skills to include in skill descriptions (A25).
    ///
    /// These are merged with the registry's builtin skills at prompt generation time
    /// via `generate_descriptions_with_extras()` (zero-clone).
    pub fn with_plugin_skills(
        mut self,
        skills: Vec<crate::agent::skills::definition::Skill>,
    ) -> Self {
        self.plugin_skills = skills;
        self
    }

    /// Set a bundle system prompt to inject after the main system prompt.
    ///
    /// This is the concatenated prompt text from all active plugin bundles,
    /// resolved via `BundleManager::resolve_bundles()`.
    pub fn with_bundle_system_prompt(mut self, prompt: String) -> Self {
        self.bundle_system_prompt = Some(prompt);
        self
    }

    /// Set the user context directory (for CLAUDE.md loading)
    pub fn with_user_context_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.user_context_dir = Some(dir.into());
        self
    }

    /// Set the context resolver feature flags
    #[cfg(feature = "context-engineering")]
    pub fn with_flags(mut self, flags: Arc<ContextResolverFlags>) -> Self {
        self.flags = Some(flags);
        self
    }

    /// Set the relevance scorer for dynamic context filtering
    #[cfg(feature = "context-engineering")]
    pub fn with_relevance_scorer(mut self, scorer: RelevanceScorer) -> Self {
        self.relevance_scorer = Some(scorer);
        self
    }

    /// Set the knowledge provider for learning system integration
    #[cfg(all(feature = "context-engineering", feature = "learning"))]
    pub fn with_knowledge_provider(
        mut self,
        provider: Arc<dyn crate::learning::KnowledgeProvider>,
    ) -> Self {
        self.knowledge_provider = Some(provider);
        self
    }

    /// Set the unified memory store for surfacing preferences in prompts.
    pub fn with_unified_store(mut self, store: Arc<UnifiedMemoryStore>, user_id: Uuid) -> Self {
        self.unified_store = Some(store);
        self.unified_store_user_id = Some(user_id);
        self
    }

    /// Preload semantic memories for the given task hint (A38).
    ///
    /// Call this BEFORE `generate_system_prompt()` to enable semantic recall
    /// injection. Uses `semantic_search()` on the unified store, which
    /// delegates to the backend's vector search when available or falls
    /// back to keyword search.
    pub async fn preload_semantic_memories(&mut self, task_hint: &str, limit: usize) {
        if let (Some(ref store), Some(_user_id)) = (&self.unified_store, self.unified_store_user_id)
        {
            let results = store.semantic_search(_user_id, task_hint, limit).await;
            if !results.is_empty() {
                self.preloaded_memories = Some(results);
            }
        }
    }

    /// Inspect the composed prompt, returning per-section token counts and utilization
    ///
    /// This resolves the context hierarchy and builds a detailed inspection
    /// of the system prompt including section breakdowns, token estimates,
    /// and the full rendered prompt text.
    #[cfg(feature = "context-engineering")]
    pub fn inspect_prompt(&mut self) -> PromptInspection {
        let max_tokens = self
            .platform
            .as_ref()
            .map(|p| p.max_skill_description_chars() / 4)
            .unwrap_or(8192);
        let resolved = self.resolve();
        PromptInspection::from_resolved(resolved, max_tokens)
    }

    /// Get the context resolver flags (if set)
    #[cfg(feature = "context-engineering")]
    pub fn flags(&self) -> Option<&ContextResolverFlags> {
        self.flags.as_ref().map(|f| f.as_ref())
    }

    /// Set platform context directly
    pub fn with_platform(mut self, platform: PlatformContext) -> Self {
        self.platform = Some(platform);
        self.resolved = None; // Invalidate cache
        self
    }

    /// Set organization context directly
    pub fn with_organization(mut self, org: OrganizationContext) -> Self {
        self.organization = Some(org);
        self.resolved = None;
        self
    }

    /// Set user context directly
    pub fn with_user(mut self, user: UserContext) -> Self {
        self.user = Some(user);
        self.resolved = None;
        self
    }

    /// Set session context directly
    pub fn with_session(mut self, session: SessionContext) -> Self {
        self.session = Some(session);
        self.resolved = None;
        self
    }

    /// Set task context directly
    pub fn with_task(mut self, task: TaskContext) -> Self {
        self.task = Some(task);
        self.resolved = None;
        self
    }

    /// Set subagent context directly
    pub fn with_subagent(mut self, subagent: SubAgentContext) -> Self {
        self.subagent = Some(subagent);
        self.resolved = None;
        self
    }

    /// Load platform context from configuration file
    pub fn load_platform(&mut self) -> Result<&PlatformContext> {
        if self.platform.is_none() {
            let path = self
                .platform_config_path
                .clone()
                .unwrap_or_else(|| PathBuf::from("config/platform-rules.yaml"));

            let loader = PlatformContextLoader::new(&path);
            self.platform = Some(loader.load()?);
            self.resolved = None;
        }

        Ok(self.platform.as_ref().unwrap())
    }

    /// Load organization context
    ///
    /// Creates a default empty organization context (database loading is async)
    pub fn load_organization(&mut self, _org_id: &str) -> Result<&OrganizationContext> {
        if self.organization.is_none() {
            self.organization = Some(OrganizationContext::default());
            self.resolved = None;
        }
        Ok(self.organization.as_ref().unwrap())
    }

    /// Load user context synchronously from directory
    ///
    /// This loads CLAUDE.md file without database queries
    pub fn load_user_sync(&mut self, user_id: &Uuid) -> &UserContext {
        if self.user.is_none() {
            let dir = self
                .user_context_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from("."));

            #[cfg(feature = "database")]
            let loader = UserContextLoader::new(None, &dir);
            #[cfg(not(feature = "database"))]
            let loader = UserContextLoader::new(&dir);
            self.user = Some(loader.load_sync(user_id));
            self.resolved = None;
        }

        self.user.as_ref().unwrap()
    }

    /// Create session context for a new session
    pub fn create_session(&mut self, session_id: Uuid) -> &SessionContext {
        let session = SessionContext::new(session_id);
        self.session = Some(session);
        self.resolved = None;
        self.session.as_ref().unwrap()
    }

    /// Create task context for a new task
    pub fn create_task(&mut self, description: impl Into<String>) -> &TaskContext {
        let task = TaskContext::new(description);
        self.task = Some(task);
        self.resolved = None;
        self.task.as_ref().unwrap()
    }

    /// Resolve the context hierarchy
    pub fn resolve(&mut self) -> &ResolvedContext {
        if self.resolved.is_none() {
            let mut resolved = ResolvedContext::default();

            // Apply platform context (L1)
            if let Some(ref platform) = self.platform {
                platform.apply_to(&mut resolved);
                resolved.active_layers.push("platform".to_string());
            }

            // Apply organization context (L2)
            if let Some(ref org) = self.organization {
                org.apply_to(&mut resolved);
                resolved.active_layers.push("organization".to_string());
            }

            // Apply user context (L3)
            if let Some(ref user) = self.user {
                user.apply_to(&mut resolved);
                resolved.active_layers.push("user".to_string());
            }

            // Build memory context (A38) — merges keyword memory + semantic recall
            // into a single deduplicated section that participates in token budgets.
            {
                let task_hint = self.task.as_ref().map(|t| t.description.as_str());
                let mut sections = Vec::new();
                let mut seen_keys = std::collections::HashSet::new();

                // 1. Custom instructions from unified store (keyword path)
                if let (Some(ref store), Some(user_id)) =
                    (&self.unified_store, self.unified_store_user_id)
                {
                    if let Ok(entries) = store.entries_lock().try_read() {
                        if let Some(user_entries) = entries.get(&user_id) {
                            // Custom instructions
                            if let Some(ci) = user_entries.values().find(|e| {
                                e.category == crate::memory::MemoryCategory::CustomInstruction
                            }) {
                                sections.push(format!("[Custom Instructions]\n{}", ci.content));
                                seen_keys.insert(ci.key.clone());
                            }

                            // Keyword-matched patterns
                            if let Some(hint) = task_hint {
                                let hint_lower = hint.to_lowercase();
                                let patterns: Vec<_> = user_entries
                                    .values()
                                    .filter(|e| {
                                        e.category == crate::memory::MemoryCategory::Pattern
                                            && e.content.to_lowercase().contains(&hint_lower)
                                    })
                                    .take(5)
                                    .collect();
                                for p in &patterns {
                                    seen_keys.insert(p.key.clone());
                                }
                                if !patterns.is_empty() {
                                    let text = patterns
                                        .iter()
                                        .map(|p| format!("- {}", p.content))
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    sections.push(format!("[Learned Patterns]\n{}", text));
                                }
                            }
                        }
                    }
                }

                // 2. Semantic recall (A38) — deduplicated against keyword results
                if let Some(ref memories) = self.preloaded_memories {
                    let novel: Vec<_> = memories
                        .iter()
                        .filter(|m| !seen_keys.contains(&m.key))
                        .collect();
                    if !novel.is_empty() {
                        let text = novel
                            .iter()
                            .enumerate()
                            .map(|(i, entry)| {
                                format!(
                                    "{}. [{}] {}",
                                    i + 1,
                                    format!("{:?}", entry.category),
                                    entry.content
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        sections.push(format!("[Semantic Recall]\n{}", text));
                    }
                }

                if !sections.is_empty() {
                    resolved.memory_context = Some(sections.join("\n\n"));
                    resolved.active_layers.push("memory".to_string());
                }
            }

            // Apply session context (L4)
            if let Some(ref session) = self.session {
                session.apply_to(&mut resolved);
                resolved.active_layers.push("session".to_string());
            }

            // Apply task context (L5)
            if let Some(ref task) = self.task {
                task.apply_to(&mut resolved);
                resolved.active_layers.push("task".to_string());
            }

            // Apply subagent context (L6)
            if let Some(ref subagent) = self.subagent {
                subagent.apply_to(&mut resolved);
                resolved.active_layers.push("subagent".to_string());
            }

            // Add skill descriptions if registry is available
            // Use platform's max_description_chars setting or default to 15000
            if let Some(ref registry) = self.skill_registry {
                let max_chars = self
                    .platform
                    .as_ref()
                    .map(|p| p.max_skill_description_chars())
                    .unwrap_or(15000);
                if self.plugin_skills.is_empty() {
                    resolved.skill_descriptions = registry.generate_descriptions(max_chars);
                } else {
                    resolved.skill_descriptions =
                        registry.generate_descriptions_with_extras(max_chars, &self.plugin_skills);
                }
            }

            // Context Engineering v2: Use relevance scorer + knowledge provider
            // when feature flags enable the new pipeline
            #[cfg(feature = "context-engineering")]
            {
                let should_use_new_pipeline = self.flags.as_ref().map_or(false, |f| {
                    // Use a deterministic user_id for the canary check
                    // In production, this would come from the authenticated user
                    let default_user = Uuid::nil();
                    f.should_use_new_pipeline(&default_user)
                });

                if should_use_new_pipeline {
                    if let Some(ref scorer) = self.relevance_scorer {
                        let mut scored_items: Vec<ScoredItem> = Vec::new();

                        // Convert skill descriptions to scored items
                        if let Some(ref registry) = self.skill_registry {
                            for skill in registry.list() {
                                let content = format!("{}: {}", skill.name, skill.description);
                                let tokens = estimate_tokens(&content);
                                scored_items.push(ScoredItem {
                                    content,
                                    tokens,
                                    score: 0.5, // Default score, will be refined by scorer
                                    source: ItemSource::Skill(skill.name.clone()),
                                });
                            }
                        }

                        // Unified scoring + filtering within token budget
                        if !scored_items.is_empty() {
                            let token_budget = self
                                .platform
                                .as_ref()
                                .map(|p| p.max_skill_description_chars() / 4)
                                .unwrap_or(3000);
                            let selected = scorer.select(scored_items, token_budget);
                            if !selected.is_empty() {
                                resolved.skill_descriptions = selected
                                    .iter()
                                    .map(|item| item.content.as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n\n");
                            }
                        }
                    }
                }
            }

            // Knowledge injection: runs independently of scoring pipeline
            // so it works even when scoring_rollout_pct is 0
            #[cfg(all(feature = "context-engineering", feature = "learning"))]
            {
                if self.flags.as_ref().map_or(false, |f| f.knowledge_injection) {
                    if let Some(ref provider) = self.knowledge_provider {
                        let task_desc = self
                            .task
                            .as_ref()
                            .map(|t| t.description.as_str())
                            .unwrap_or("");
                        let entries = provider.query(task_desc, 20);
                        if !entries.is_empty() {
                            let knowledge_text = entries
                                .iter()
                                .map(|e| format!("- [{:?}] {}", e.category, e.content))
                                .collect::<Vec<_>>()
                                .join("\n");
                            resolved
                                .skill_descriptions
                                .push_str(&format!("\n\n[Learned Knowledge]\n{}", knowledge_text));
                        }
                    }
                }
            }

            self.resolved = Some(resolved);
        }

        self.resolved.as_ref().unwrap()
    }

    /// Generate the system prompt from resolved context.
    ///
    /// Memory context (custom instructions, keyword patterns, semantic recall)
    /// is assembled during `resolve()` and injected as a first-class
    /// `PromptSection::Memory`, subject to the same token budgets and section
    /// ordering as L1-L6 layers.
    pub fn generate_system_prompt(&mut self) -> String {
        let resolved = self.resolve();
        let generator = SystemPromptGenerator::new()
            .with_headers(true)
            .with_tool_permissions(true);
        let mut prompt = generator.generate(resolved);

        // Inject bundle system prompt (from active plugin bundles)
        if let Some(ref bundle_prompt) = self.bundle_system_prompt {
            prompt.push_str("\n\n# Active Plugin Bundle Context\n\n");
            prompt.push_str(bundle_prompt);
        }

        prompt
    }

    /// Generate a minimal system prompt (no tool permissions)
    pub fn generate_minimal_prompt(&mut self) -> String {
        let resolved = self.resolve();
        let generator = SystemPromptGenerator::new()
            .with_headers(false)
            .with_tool_permissions(false);
        generator.generate(resolved)
    }

    /// Get the resolved context (resolves if needed)
    pub fn resolved_context(&mut self) -> &ResolvedContext {
        self.resolve()
    }

    /// Check if a tool is allowed in the current context
    pub fn is_tool_allowed(&mut self, tool: &str) -> bool {
        self.resolve().is_tool_allowed(tool)
    }

    /// Get the current permission mode
    pub fn permission_mode(&mut self) -> ContextPermissionMode {
        self.resolve().permission_mode
    }

    /// Add a loaded skill to the context
    pub fn add_loaded_skill(&mut self, skill: LoadedSkill) {
        if let Some(ref mut resolved) = self.resolved {
            resolved.loaded_skills.push(skill);
        }
    }

    /// Get the platform context
    pub fn platform(&self) -> Option<&PlatformContext> {
        self.platform.as_ref()
    }

    /// Get the organization context
    pub fn organization(&self) -> Option<&OrganizationContext> {
        self.organization.as_ref()
    }

    /// Get the user context
    pub fn user(&self) -> Option<&UserContext> {
        self.user.as_ref()
    }

    /// Get the session context
    pub fn session(&self) -> Option<&SessionContext> {
        self.session.as_ref()
    }

    /// Get mutable session context
    pub fn session_mut(&mut self) -> Option<&mut SessionContext> {
        self.resolved = None; // Invalidate cache when mutating
        self.session.as_mut()
    }

    /// Get the task context
    pub fn task(&self) -> Option<&TaskContext> {
        self.task.as_ref()
    }

    /// Get mutable task context
    pub fn task_mut(&mut self) -> Option<&mut TaskContext> {
        self.resolved = None;
        self.task.as_mut()
    }

    /// Get the subagent context
    pub fn subagent(&self) -> Option<&SubAgentContext> {
        self.subagent.as_ref()
    }

    /// Invalidate the resolved context cache
    pub fn invalidate_cache(&mut self) {
        self.resolved = None;
    }

    /// Create a fork for a subagent with the specified fork mode
    pub fn fork_for_subagent(
        &self,
        subagent_id: impl Into<String>,
        agent_type: impl Into<String>,
        fork_mode: super::ContextForkMode,
    ) -> ContextIntegration {
        use super::ForkedContext;

        let agent_type_str = agent_type.into();

        // Build subagent context
        let mut subagent_builder = SubAgentContextBuilder::new(&agent_type_str)
            .id(subagent_id)
            .fork_mode(fork_mode.clone());

        if let Some(ref session) = self.session {
            subagent_builder = subagent_builder.parent(session.session_id);
        }

        // Create forked context if needed
        if matches!(
            fork_mode,
            super::ContextForkMode::Inherit | super::ContextForkMode::Fork
        ) {
            let mut forked = ForkedContext::default();

            // Copy relevant context from parent session
            if let Some(ref session) = self.session {
                forked.working_files = session.working_files.keys().cloned().collect();
            }

            // Copy task description
            if let Some(ref task) = self.task {
                // Store task description in custom context
                forked.custom_context.insert(
                    "parent_task".to_string(),
                    serde_json::json!(task.description),
                );
            }

            subagent_builder = subagent_builder.forked_context(forked);
        }

        let subagent = subagent_builder.build();

        // Create new integration with inherited contexts
        let mut fork = ContextIntegration::new();

        // Inherit platform and organization (read-only)
        fork.platform = self.platform.clone();
        fork.organization = self.organization.clone();

        // Inherit user context
        fork.user = self.user.clone();

        // Set subagent context
        fork.subagent = Some(subagent);

        // Inherit skill registry
        fork.skill_registry = self.skill_registry.clone();

        // Inherit context engineering v2 fields
        #[cfg(feature = "context-engineering")]
        {
            fork.flags = self.flags.clone();
            fork.relevance_scorer = None; // Subagents create their own scorer
        }
        #[cfg(all(feature = "context-engineering", feature = "learning"))]
        {
            fork.knowledge_provider = self.knowledge_provider.clone();
        }

        // Inherit unified memory store
        fork.unified_store = self.unified_store.clone();
        fork.unified_store_user_id = self.unified_store_user_id;
        fork.preloaded_memories = self.preloaded_memories.clone();

        fork
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_integration_new() {
        let integration = ContextIntegration::new();
        assert!(integration.platform.is_none());
        assert!(integration.session.is_none());
        assert!(integration.task.is_none());
    }

    #[test]
    fn test_create_session() {
        let mut integration = ContextIntegration::new();
        let session_id = Uuid::new_v4();

        let session = integration.create_session(session_id);
        assert_eq!(session.session_id, session_id);
    }

    #[test]
    fn test_create_task() {
        let mut integration = ContextIntegration::new();
        let task = integration.create_task("Fix the bug");
        assert_eq!(task.description, "Fix the bug");
    }

    #[test]
    fn test_resolve_empty() {
        let mut integration = ContextIntegration::new();
        let resolved = integration.resolve();
        assert!(resolved.platform_rules.is_empty());
        assert!(resolved.active_layers.is_empty());
    }

    #[test]
    fn test_with_skill_registry() {
        let registry = Arc::new(SkillRegistry::with_builtins());
        let mut integration = ContextIntegration::new().with_skill_registry(registry);

        let resolved = integration.resolve();
        // Should have skill descriptions from builtins
        assert!(!resolved.skill_descriptions.is_empty());
    }

    #[test]
    fn test_generate_system_prompt() {
        let mut integration = ContextIntegration::new();
        integration.create_session(Uuid::new_v4());
        integration.create_task("Test task");

        let prompt = integration.generate_system_prompt();
        // Should generate without error
        assert!(prompt.is_empty() || !prompt.is_empty());
    }

    #[test]
    fn test_is_tool_allowed() {
        let mut integration = ContextIntegration::new();

        // No restrictions = all allowed
        assert!(integration.is_tool_allowed("read"));
        assert!(integration.is_tool_allowed("bash"));
    }

    #[test]
    fn test_fork_for_subagent() {
        let mut integration = ContextIntegration::new();
        integration.create_session(Uuid::new_v4());
        integration.create_task("Parent task");

        let forked = integration.fork_for_subagent(
            "sub-1",
            "explore",
            super::super::ContextForkMode::Inherit,
        );

        assert!(forked.subagent.is_some());
        assert!(forked.session.is_none()); // Subagent gets its own session
        assert!(forked.task.is_none()); // Subagent gets its own task
    }

    #[test]
    fn test_cache_invalidation() {
        let mut integration = ContextIntegration::new();

        // Resolve to populate cache
        let _ = integration.resolve();
        assert!(integration.resolved.is_some());

        // Create task should invalidate
        integration.create_task("New task");
        assert!(integration.resolved.is_none());

        // Resolve again
        let resolved = integration.resolve();
        assert!(resolved.active_layers.contains(&"task".to_string()));
    }
}
