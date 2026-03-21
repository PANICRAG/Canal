//! Integration Tests for Six-Layer Context Hierarchy
//!
//! Tests the complete flow of:
//! - Loading contexts at each layer
//! - Context resolution and merging
//! - System prompt generation
//! - Skill loading (two-layer)
//! - SubAgent forking
//! - Tool permission enforcement

use gateway_core::agent::{
    ContextForkMode,
    // Context types
    ContextIntegration,
    ContextPriority,
    ContextResolver,
    LoadedSkill,
    OrganizationContext,
    // Layer contexts
    PlatformContext,
    PromptSection,
    ResolvedContext,
    SessionContext,
    // Skills
    SkillRegistry,
    SubAgentContext,
    SubAgentContextBuilder,
    SystemPromptGenerator,
    TaskContext,
    TaskContextBuilder,
};
use std::sync::Arc;
use uuid::Uuid;

// ============================================================================
// Test: Full Context Hierarchy Resolution
// ============================================================================

#[test]
fn test_full_context_hierarchy_resolution() {
    // Create contexts for each layer
    let platform = PlatformContext::default();
    let org = OrganizationContext::default();
    let session = SessionContext::new(Uuid::new_v4());
    let task = TaskContext::new("Fix authentication bug in login flow");
    let subagent = SubAgentContext::new("code-reviewer");

    // Resolve using ContextResolver
    let mut resolver = ContextResolver::new();
    let resolved = resolver.resolve_full(
        Some(&platform),
        Some(&org),
        None, // No user context in this test
        Some(&session),
        Some(&task),
        Some(&subagent),
    );

    // Verify all layers were applied
    assert!(resolved.has_layer("platform"));
    assert!(resolved.has_layer("organization"));
    assert!(resolved.has_layer("session"));
    assert!(resolved.has_layer("task"));
    assert!(resolved.has_layer("subagent"));
    assert_eq!(resolved.active_layers.len(), 5);

    // Verify task instructions are set
    assert!(resolved.task_instructions.is_some());
    assert!(resolved
        .task_instructions
        .as_ref()
        .unwrap()
        .contains("authentication"));
}

#[test]
fn test_context_priority_enforcement() {
    // Platform sets a blocked tool
    let mut resolved = ResolvedContext::default();
    resolved.blocked_tools.push("dangerous_tool".to_string());
    resolved.allowed_tools.push("read".to_string());
    resolved.allowed_tools.push("write".to_string());

    // Verify tool permission checks
    assert!(resolved.is_tool_allowed("read"));
    assert!(resolved.is_tool_allowed("write"));
    assert!(!resolved.is_tool_allowed("dangerous_tool")); // Blocked
    assert!(!resolved.is_tool_allowed("unknown_tool")); // Not in allow list

    // Verify priority ordering
    assert!(ContextPriority::Platform < ContextPriority::Organization);
    assert!(ContextPriority::Organization < ContextPriority::User);
    assert!(ContextPriority::User < ContextPriority::Session);
    assert!(ContextPriority::Session < ContextPriority::Task);
    assert!(ContextPriority::Task < ContextPriority::SubAgent);

    // Verify can_override logic
    assert!(ContextPriority::Platform.can_override(ContextPriority::User));
    assert!(!ContextPriority::User.can_override(ContextPriority::Platform));
}

// ============================================================================
// Test: Context Integration Layer
// ============================================================================

#[test]
fn test_context_integration_basic_flow() {
    let registry = Arc::new(SkillRegistry::with_builtins());

    let mut integration = ContextIntegration::new().with_skill_registry(registry);

    // Create session and task
    let session_id = Uuid::new_v4();
    integration.create_session(session_id);
    integration.create_task("Implement user authentication");

    // Resolve context
    let resolved = integration.resolved_context();

    // Verify session and task are active
    assert!(resolved.has_layer("session"));
    assert!(resolved.has_layer("task"));
    assert!(resolved.task_instructions.is_some());

    // Verify skill descriptions are included
    assert!(!resolved.skill_descriptions.is_empty());
}

#[test]
fn test_context_integration_with_platform() {
    let platform = PlatformContext::default();
    let registry = Arc::new(SkillRegistry::with_builtins());

    let mut integration = ContextIntegration::new()
        .with_platform(platform)
        .with_skill_registry(registry);

    integration.create_session(Uuid::new_v4());

    let resolved = integration.resolved_context();

    // Platform layer should be active
    assert!(resolved.has_layer("platform"));

    // Platform rules should be in the context
    assert!(resolved.active_layers.contains(&"platform".to_string()));
}

#[test]
fn test_context_cache_invalidation() {
    let mut integration = ContextIntegration::new();

    // First resolve
    integration.create_session(Uuid::new_v4());
    let resolved1 = integration.resolve();
    assert!(resolved1.has_layer("session"));
    assert!(!resolved1.has_layer("task"));

    // Add task - should invalidate cache
    integration.create_task("New task");
    let resolved2 = integration.resolve();

    // Now should have both
    assert!(resolved2.has_layer("session"));
    assert!(resolved2.has_layer("task"));
}

// ============================================================================
// Test: Two-Layer Skill Loading
// ============================================================================

#[test]
fn test_two_layer_skill_loading() {
    let registry = Arc::new(SkillRegistry::with_builtins());

    let mut integration = ContextIntegration::new().with_skill_registry(registry.clone());

    integration.create_session(Uuid::new_v4());

    // Layer 1: Skill descriptions should be in resolved context
    let resolved = integration.resolved_context();
    assert!(!resolved.skill_descriptions.is_empty());

    // Descriptions should contain builtin skill info
    let descriptions = &resolved.skill_descriptions;
    assert!(descriptions.len() > 0);

    // Layer 2: Simulate loading a skill on-demand
    let skill = LoadedSkill {
        name: "commit".to_string(),
        content: "Full commit skill instructions here...".to_string(),
        requires_browser: false,
        automation_tab: false,
    };

    integration.add_loaded_skill(skill);

    // Verify skill is added to loaded_skills
    let resolved = integration.resolved_context();
    assert!(resolved.has_skill("commit"));

    let loaded = resolved.get_skill("commit").unwrap();
    assert_eq!(loaded.name, "commit");
    assert!(loaded.content.contains("Full commit skill"));
}

#[test]
fn test_skill_registry_descriptions() {
    let registry = SkillRegistry::with_builtins();

    // Generate descriptions with a reasonable limit
    let descriptions = registry.generate_descriptions(15000);

    // Should contain formatted skill info
    assert!(!descriptions.is_empty());

    // Should be under the limit (approximately)
    assert!(descriptions.len() <= 20000); // Some buffer for formatting
}

// ============================================================================
// Test: SubAgent Forking
// ============================================================================

#[test]
fn test_subagent_fork_none_mode() {
    let subagent = SubAgentContextBuilder::new("explore")
        .id("sub-001")
        .fork_mode(ContextForkMode::None)
        .instructions("Search for relevant files")
        .build();

    assert_eq!(subagent.subagent_id, "sub-001");
    assert_eq!(subagent.agent_type, "explore");
    assert_eq!(subagent.fork_mode, ContextForkMode::None);
    assert!(subagent.forked_context.is_none());
}

#[test]
fn test_subagent_fork_inherit_mode() {
    let parent_session = SessionContext::new(Uuid::new_v4());

    let subagent = SubAgentContextBuilder::new("code-reviewer")
        .id("sub-002")
        .parent(parent_session.session_id)
        .fork_mode(ContextForkMode::Inherit)
        .instructions("Review the code changes")
        .allow_tool("read")
        .allow_tool("glob")
        .block_tool("bash")
        .build();

    assert_eq!(subagent.fork_mode, ContextForkMode::Inherit);
    assert_eq!(subagent.parent_session_id, Some(parent_session.session_id));
    assert!(subagent.allowed_tools.contains(&"read".to_string()));
    assert!(subagent.blocked_tools.contains(&"bash".to_string()));
}

#[test]
fn test_subagent_fork_with_context() {
    let parent_session = SessionContext::new(Uuid::new_v4());

    // Use SubAgentContext::fork to create with forked context
    let subagent = SubAgentContext::fork("worker", &parent_session);

    assert_eq!(subagent.fork_mode, ContextForkMode::Fork);
    assert!(subagent.forked_context.is_some());

    let forked = subagent.forked_context.as_ref().unwrap();
    assert!(forked.forked_at <= chrono::Utc::now());
}

#[test]
fn test_context_integration_fork_for_subagent() {
    let mut integration = ContextIntegration::new();
    integration.create_session(Uuid::new_v4());
    integration.create_task("Parent task");

    // Fork for a subagent
    let forked = integration.fork_for_subagent("sub-explore", "explore", ContextForkMode::Inherit);

    // Forked integration should have subagent context
    assert!(forked.subagent().is_some());

    let subagent = forked.subagent().unwrap();
    assert_eq!(subagent.agent_type, "explore");
    // Note: fork_mode becomes Fork when forked_context is set
    // (even if Inherit was requested, having a forked context implies Fork)
    assert_eq!(subagent.fork_mode, ContextForkMode::Fork);

    // Forked should NOT have session/task (subagent creates its own)
    assert!(forked.session().is_none());
    assert!(forked.task().is_none());
}

// ============================================================================
// Test: System Prompt Generation
// ============================================================================

#[test]
fn test_system_prompt_generator_basic() {
    let mut ctx = ResolvedContext::default();
    ctx.platform_rules = "Always respond in English.".to_string();
    ctx.org_conventions = Some("Follow team coding standards.".to_string());
    ctx.user_preferences = Some("Prefer explicit type annotations.".to_string());
    ctx.task_instructions = Some("Fix the authentication bug.".to_string());
    ctx.skill_descriptions = "Available skills:\n- commit: Create git commits".to_string();

    let generator = SystemPromptGenerator::new();
    let prompt = generator.generate(&ctx);

    // Verify all sections are included
    assert!(prompt.contains("Always respond in English"));
    assert!(prompt.contains("Follow team coding standards"));
    assert!(prompt.contains("Prefer explicit type annotations"));
    assert!(prompt.contains("Fix the authentication bug"));
    assert!(prompt.contains("commit: Create git commits"));

    // Verify order: platform before org before user before task
    let platform_pos = prompt.find("Always respond in English").unwrap();
    let org_pos = prompt.find("Follow team coding standards").unwrap();
    let user_pos = prompt.find("Prefer explicit type annotations").unwrap();
    let task_pos = prompt.find("Fix the authentication bug").unwrap();

    assert!(platform_pos < org_pos);
    assert!(org_pos < user_pos);
    assert!(user_pos < task_pos);
}

#[test]
fn test_system_prompt_with_loaded_skills() {
    let mut ctx = ResolvedContext::default();
    ctx.platform_rules = "Platform rules here.".to_string();
    ctx.loaded_skills.push(LoadedSkill {
        name: "commit".to_string(),
        content: "When committing, use conventional commits format.".to_string(),
        requires_browser: false,
        automation_tab: false,
    });
    ctx.loaded_skills.push(LoadedSkill {
        name: "browser-action".to_string(),
        content: "Use browser automation for web tasks.".to_string(),
        requires_browser: true,
        automation_tab: true,
    });

    let generator = SystemPromptGenerator::new();
    let prompt = generator.generate(&ctx);

    // Verify loaded skills are included
    assert!(prompt.contains("commit (Active)"));
    assert!(prompt.contains("conventional commits"));
    assert!(prompt.contains("browser-action (Active)"));
    assert!(prompt.contains("requires browser access"));
}

#[test]
fn test_system_prompt_with_tool_permissions() {
    let mut ctx = ResolvedContext::default();
    ctx.platform_rules = "Platform rules.".to_string();
    ctx.allowed_tools = vec!["read".to_string(), "write".to_string()];
    ctx.blocked_tools = vec!["bash".to_string()];
    ctx.permission_mode = gateway_core::agent::ContextPermissionMode::Restricted;

    let generator = SystemPromptGenerator::new().with_tool_permissions(true);
    let prompt = generator.generate(&ctx);

    // Verify tool permissions section
    assert!(prompt.contains("Permission Mode"));
    assert!(prompt.contains("Restricted"));
    assert!(prompt.contains("`read`"));
    assert!(prompt.contains("`bash`"));
}

#[test]
fn test_system_prompt_custom_section_order() {
    let mut ctx = ResolvedContext::default();
    ctx.platform_rules = "Platform rules.".to_string();
    ctx.task_instructions = Some("Task instructions.".to_string());

    // Custom order: Task before Platform
    let generator = SystemPromptGenerator::new()
        .with_section_order(vec![PromptSection::Task, PromptSection::Platform]);
    let prompt = generator.generate(&ctx);

    let task_pos = prompt.find("Task instructions").unwrap();
    let platform_pos = prompt.find("Platform rules").unwrap();

    // Task should come before Platform with custom order
    assert!(task_pos < platform_pos);
}

#[test]
fn test_system_prompt_skip_sections() {
    let mut ctx = ResolvedContext::default();
    ctx.platform_rules = "Platform rules.".to_string();
    ctx.org_conventions = Some("Org conventions.".to_string());
    ctx.task_instructions = Some("Task instructions.".to_string());

    let generator = SystemPromptGenerator::new().skip_section(PromptSection::Organization);
    let prompt = generator.generate(&ctx);

    // Organization should be skipped
    assert!(!prompt.contains("Org conventions"));
    assert!(prompt.contains("Platform rules"));
    assert!(prompt.contains("Task instructions"));
}

#[test]
fn test_system_prompt_token_limit() {
    let mut ctx = ResolvedContext::default();
    ctx.platform_rules = "A".repeat(10000); // Long content

    let generator = SystemPromptGenerator::new().with_max_tokens(100); // Very low limit
    let prompt = generator.generate(&ctx);

    // Should be truncated
    assert!(prompt.len() < 1000);
    assert!(prompt.contains("truncated"));
}

// ============================================================================
// Test: Task Context Features
// ============================================================================

#[test]
fn test_task_context_working_memory() {
    let mut task = TaskContextBuilder::new()
        .id("task-001")
        .description("Implement feature X")
        .build();

    // Add discoveries
    task.add_discovery("Found relevant file: src/auth.rs", "code", 0.9);
    task.add_discovery("API endpoint: /api/login", "api", 0.8);

    // Add verifications
    task.add_verification(
        "unit-tests",
        gateway_core::agent::context::VerificationType::UnitTest,
    );
    task.add_verification(
        "lint-check",
        gateway_core::agent::context::VerificationType::Lint,
    );

    // Check working memory
    let memory = &task.working_memory;
    assert_eq!(memory.discoveries.len(), 2);
    assert_eq!(memory.pending_verifications.len(), 2);
}

#[test]
fn test_task_context_constraints() {
    let task = TaskContextBuilder::new()
        .description("Review code")
        .max_iterations(10)
        .build();

    assert_eq!(task.constraints.max_iterations, Some(10));
}

#[test]
fn test_task_context_skill_loading() {
    let mut task = TaskContext::new("Test skill loading");

    // Load a skill
    task.load_skill(LoadedSkill {
        name: "test-skill".to_string(),
        content: "Test skill content".to_string(),
        requires_browser: false,
        automation_tab: false,
    });

    // Check via loaded_skills vector
    assert_eq!(task.loaded_skills.len(), 1);
    assert_eq!(task.loaded_skills[0].name, "test-skill");
}

// ============================================================================
// Test: Session Context Features
// ============================================================================

#[test]
fn test_session_context_file_tracking() {
    let mut session = SessionContext::new(Uuid::new_v4());

    // Track files
    session.track_file("src/main.rs");
    session.track_file("src/lib.rs");
    session.mark_file_modified("src/lib.rs");

    assert_eq!(session.working_files.len(), 2);
    assert!(session.working_files.contains_key("src/main.rs"));

    // Check modified flag
    let lib_state = session.working_files.get("src/lib.rs").unwrap();
    assert!(lib_state.modified);
}

#[test]
fn test_session_context_tool_state() {
    let mut session = SessionContext::new(Uuid::new_v4());

    // Record tool usage
    session.record_tool_call("bash", Some("ls -la".to_string()));
    session.record_tool_call("bash", Some("cargo build".to_string()));
    session.record_tool_call("read", Some("src/main.rs".to_string()));

    assert_eq!(session.tool_states.len(), 2); // bash and read

    let bash_state = session.tool_states.get("bash").unwrap();
    assert_eq!(bash_state.call_count, 2);
    assert_eq!(bash_state.last_result, Some("cargo build".to_string()));
}

#[test]
fn test_session_context_skill_loading() {
    let mut session = SessionContext::new(Uuid::new_v4());

    // Load a skill
    session.load_skill(LoadedSkill {
        name: "commit".to_string(),
        content: "Commit skill content".to_string(),
        requires_browser: false,
        automation_tab: false,
    });

    assert!(session.is_skill_loaded("commit"));
    assert!(!session.is_skill_loaded("unknown"));

    let names = session.loaded_skill_names();
    assert!(names.contains(&"commit"));
}

// ============================================================================
// Test: End-to-End Integration
// ============================================================================

#[test]
fn test_e2e_context_flow() {
    // 1. Setup: Create all context layers
    let platform = PlatformContext::default();
    let org = OrganizationContext::default();
    let registry = Arc::new(SkillRegistry::with_builtins());

    // 2. Create integration with platform and skills
    let mut integration = ContextIntegration::new()
        .with_platform(platform)
        .with_organization(org)
        .with_skill_registry(registry);

    // 3. Create session
    let session_id = Uuid::new_v4();
    integration.create_session(session_id);

    // 4. Create task
    integration.create_task("Implement user login feature with OAuth2");

    // 5. Resolve and generate system prompt
    let system_prompt = integration.generate_system_prompt();

    // 6. Verify prompt contains expected sections
    assert!(!system_prompt.is_empty());

    // 7. Check tool permissions
    assert!(integration.is_tool_allowed("read"));
    assert!(integration.is_tool_allowed("write"));

    // 8. Fork for subagent
    let subagent_ctx =
        integration.fork_for_subagent("oauth-impl", "code-writer", ContextForkMode::Inherit);

    // 9. Verify subagent has isolated context
    assert!(subagent_ctx.subagent().is_some());
    assert!(subagent_ctx.platform().is_some()); // Inherited
    assert!(subagent_ctx.session().is_none()); // Not inherited

    // 10. Subagent can create its own session/task
    let mut subagent_integration = subagent_ctx;
    subagent_integration.create_session(Uuid::new_v4());
    subagent_integration.create_task("Implement OAuth2 callback handler");

    let subagent_prompt = subagent_integration.generate_system_prompt();
    assert!(!subagent_prompt.is_empty());
}

#[test]
fn test_e2e_skill_invocation_flow() {
    // Simulate the two-layer skill loading flow
    let registry = Arc::new(SkillRegistry::with_builtins());

    let mut integration = ContextIntegration::new().with_skill_registry(registry.clone());

    integration.create_session(Uuid::new_v4());
    integration.create_task("Create a git commit");

    // Layer 1: Check skill descriptions are available
    let resolved = integration.resolved_context();
    let descriptions = &resolved.skill_descriptions;

    // Should have skill info in descriptions
    assert!(!descriptions.is_empty());

    // Layer 2: Simulate LLM invoking a skill
    if let Some(skill) = registry.get("commit") {
        let loaded = LoadedSkill {
            name: skill.name.clone(),
            content: skill.prompt_template.clone(),
            requires_browser: false,
            automation_tab: false,
        };
        integration.add_loaded_skill(loaded);
    }

    // Generate final prompt with loaded skill
    let final_prompt = integration.generate_system_prompt();

    // Prompt should be non-empty
    assert!(!final_prompt.is_empty());
}

#[test]
fn test_resolved_context_methods() {
    let mut ctx = ResolvedContext::default();

    // Test config values
    ctx.config_values
        .insert("key1".to_string(), serde_json::json!("value1"));
    assert_eq!(ctx.get_config("key1"), Some(&serde_json::json!("value1")));
    assert_eq!(ctx.get_config("key2"), None);

    let default = serde_json::json!("default");
    assert_eq!(ctx.get_config_or("key2", &default), &default);

    // Test active layers
    ctx.active_layers.push("platform".to_string());
    ctx.active_layers.push("user".to_string());
    assert!(ctx.has_layer("platform"));
    assert!(ctx.has_layer("user"));
    assert!(!ctx.has_layer("organization"));

    // Test skill management
    ctx.loaded_skills.push(LoadedSkill {
        name: "skill1".to_string(),
        content: "content1".to_string(),
        requires_browser: false,
        automation_tab: false,
    });

    assert!(ctx.has_skill("skill1"));
    assert!(!ctx.has_skill("skill2"));

    let skill = ctx.get_skill("skill1").unwrap();
    assert_eq!(skill.content, "content1");
}
