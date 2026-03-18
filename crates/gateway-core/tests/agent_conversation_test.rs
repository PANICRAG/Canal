//! Agent Conversation Tests with Six-Layer Context Hierarchy
//!
//! Tests the complete conversation flow with:
//! - Six-layer context integration
//! - System prompt generation
//! - Tool permission enforcement
//! - Skill two-layer loading
//! - SubAgent forking
//! - Session state tracking

use async_trait::async_trait;
use gateway_core::agent::r#loop::{LlmClient, LlmResponse, StopReason, ToolExecutor};
use gateway_core::agent::tools::ToolContext;
use gateway_core::agent::{
    // Agent types
    AgentError,
    // Types
    AgentMessage,
    ContentBlock,
    ContextForkMode,
    // Context types
    ContextIntegration,
    LoadedSkill,
    OrganizationContext,
    // Layer contexts
    PlatformContext,
    ResolvedContext,
    SessionContext,
    // Tools
    SkillRegistry,
    SubAgentContext,
    SubAgentContextBuilder,
    SystemPromptGenerator,
    TaskContextBuilder,
    Usage,
};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::RwLock;
use uuid::Uuid;

// ============================================================================
// Mock LLM Client
// ============================================================================

/// Mock LLM client for testing
struct MockLlmClient {
    /// Pre-configured responses
    responses: Arc<RwLock<Vec<LlmResponse>>>,
    /// Number of calls made
    call_count: AtomicUsize,
    /// Captured system prompts for verification
    captured_system_prompts: Arc<RwLock<Vec<String>>>,
    /// Captured user messages
    captured_messages: Arc<RwLock<Vec<AgentMessage>>>,
}

impl MockLlmClient {
    fn new() -> Self {
        Self {
            responses: Arc::new(RwLock::new(Vec::new())),
            call_count: AtomicUsize::new(0),
            captured_system_prompts: Arc::new(RwLock::new(Vec::new())),
            captured_messages: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add a text response
    async fn add_text_response(&self, text: &str) {
        let response = LlmResponse {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            model: "mock-model".to_string(),
            usage: Usage::default(),
            stop_reason: StopReason::EndTurn,
        };
        self.responses.write().await.push(response);
    }

    /// Add a tool use response
    async fn add_tool_use_response(&self, tool_name: &str, tool_input: serde_json::Value) {
        let response = LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: format!("tool_{}", Uuid::new_v4()),
                name: tool_name.to_string(),
                input: tool_input,
            }],
            model: "mock-model".to_string(),
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
        };
        self.responses.write().await.push(response);
    }

    /// Get call count
    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Get captured system prompts
    async fn get_system_prompts(&self) -> Vec<String> {
        self.captured_system_prompts.read().await.clone()
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn generate(
        &self,
        messages: Vec<AgentMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<LlmResponse, AgentError> {
        // Capture messages
        {
            let mut captured = self.captured_messages.write().await;
            captured.extend(messages.clone());
        }

        // Extract and capture system prompt
        for msg in &messages {
            if let AgentMessage::System(sys) = msg {
                if let Some(text) = sys.data.as_str() {
                    self.captured_system_prompts
                        .write()
                        .await
                        .push(text.to_string());
                }
            }
        }

        let count = self.call_count.fetch_add(1, Ordering::SeqCst);

        // Return pre-configured response or default
        let responses = self.responses.read().await;
        if count < responses.len() {
            Ok(responses[count].clone())
        } else {
            // Default response
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "Default mock response".to_string(),
                }],
                model: "mock-model".to_string(),
                usage: Usage::default(),
                stop_reason: StopReason::EndTurn,
            })
        }
    }
}

// ============================================================================
// Mock Tool Executor
// ============================================================================

/// Mock tool executor for testing
struct MockToolExecutor {
    /// Allowed tools
    allowed_tools: Arc<RwLock<std::collections::HashSet<String>>>,
    /// Call history
    call_history: Arc<RwLock<Vec<(String, serde_json::Value)>>>,
    /// Custom responses per tool
    tool_responses: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}

impl MockToolExecutor {
    fn new() -> Self {
        let mut allowed = std::collections::HashSet::new();
        // Default allowed tools
        allowed.insert("read".to_string());
        allowed.insert("write".to_string());
        allowed.insert("glob".to_string());
        allowed.insert("grep".to_string());
        allowed.insert("invoke_skill".to_string());

        Self {
            allowed_tools: Arc::new(RwLock::new(allowed)),
            call_history: Arc::new(RwLock::new(Vec::new())),
            tool_responses: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn allow_tool(&self, name: &str) {
        self.allowed_tools.write().await.insert(name.to_string());
    }

    async fn block_tool(&self, name: &str) {
        self.allowed_tools.write().await.remove(name);
    }

    async fn set_tool_response(&self, tool: &str, response: serde_json::Value) {
        self.tool_responses
            .write()
            .await
            .insert(tool.to_string(), response);
    }

    async fn get_call_history(&self) -> Vec<(String, serde_json::Value)> {
        self.call_history.read().await.clone()
    }
}

#[async_trait]
impl ToolExecutor for MockToolExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        tool_input: serde_json::Value,
        _context: &ToolContext,
    ) -> Result<serde_json::Value, AgentError> {
        // Check if tool is allowed
        if !self.allowed_tools.read().await.contains(tool_name) {
            return Err(AgentError::PermissionDenied(format!(
                "Tool '{}' is not allowed",
                tool_name
            )));
        }

        // Record call
        self.call_history
            .write()
            .await
            .push((tool_name.to_string(), tool_input.clone()));

        // Return custom response if configured
        if let Some(response) = self.tool_responses.read().await.get(tool_name) {
            return Ok(response.clone());
        }

        // Default responses
        Ok(match tool_name {
            "read" => serde_json::json!({
                "content": "// File content here",
                "path": tool_input.get("path").unwrap_or(&serde_json::json!("unknown"))
            }),
            "invoke_skill" => {
                let skill_name = tool_input
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                serde_json::json!({
                    "success": true,
                    "content": format!("## [{}] Skill Loaded\n\nFull content for {} skill.", skill_name, skill_name),
                    "requires_browser": false,
                    "automation_tab": false
                })
            }
            _ => serde_json::json!({"status": "ok", "tool": tool_name}),
        })
    }

    fn get_tool_schemas(&self) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({
                "name": "read",
                "description": "Read a file",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    }
                }
            }),
            serde_json::json!({
                "name": "invoke_skill",
                "description": "Load skill content on-demand",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"}
                    }
                }
            }),
        ]
    }
}

// ============================================================================
// Test Harness
// ============================================================================

/// Test harness for agent conversation tests
struct AgentTestHarness {
    integration: ContextIntegration,
    mock_llm: Arc<MockLlmClient>,
    mock_tools: Arc<MockToolExecutor>,
    session_id: Uuid,
}

impl AgentTestHarness {
    /// Create a new test harness with default setup
    fn new() -> Self {
        let session_id = Uuid::new_v4();
        let skill_registry = Arc::new(SkillRegistry::with_builtins());

        let mut integration = ContextIntegration::new()
            .with_platform(PlatformContext::default())
            .with_organization(OrganizationContext::default())
            .with_skill_registry(skill_registry);

        integration.create_session(session_id);

        Self {
            integration,
            mock_llm: Arc::new(MockLlmClient::new()),
            mock_tools: Arc::new(MockToolExecutor::new()),
            session_id,
        }
    }

    /// Create harness with custom platform context
    fn with_platform(mut self, platform: PlatformContext) -> Self {
        self.integration = self.integration.with_platform(platform);
        self
    }

    /// Create harness with custom organization context
    fn with_organization(mut self, org: OrganizationContext) -> Self {
        self.integration = self.integration.with_organization(org);
        self
    }

    /// Create a task
    fn create_task(&mut self, description: &str) {
        self.integration.create_task(description);
    }

    /// Get the generated system prompt
    fn get_system_prompt(&mut self) -> String {
        self.integration.generate_system_prompt()
    }

    /// Get resolved context
    fn get_resolved_context(&mut self) -> &ResolvedContext {
        self.integration.resolved_context()
    }

    /// Check if tool is allowed
    fn is_tool_allowed(&mut self, tool: &str) -> bool {
        self.integration.is_tool_allowed(tool)
    }

    /// Add a loaded skill
    fn add_loaded_skill(&mut self, name: &str, content: &str) {
        self.integration.add_loaded_skill(LoadedSkill {
            name: name.to_string(),
            content: content.to_string(),
            requires_browser: false,
            automation_tab: false,
        });
    }

    /// Fork for subagent
    fn fork_for_subagent(&self, id: &str, agent_type: &str) -> ContextIntegration {
        self.integration
            .fork_for_subagent(id, agent_type, ContextForkMode::Inherit)
    }

    /// Get mock LLM for assertions
    fn mock_llm(&self) -> &Arc<MockLlmClient> {
        &self.mock_llm
    }

    /// Get mock tools for assertions
    fn mock_tools(&self) -> &Arc<MockToolExecutor> {
        &self.mock_tools
    }
}

// ============================================================================
// Test Case 1: Complete Conversation Flow with Six-Layer Context
// ============================================================================

#[tokio::test]
async fn test_conversation_with_six_layer_context() {
    let mut harness = AgentTestHarness::new();

    // Create a task
    harness.create_task("Fix the authentication bug in login.rs");

    // Generate system prompt
    let system_prompt = harness.get_system_prompt();

    // Verify system prompt contains expected sections
    // Platform rules should be present (may be minimal with default config)
    assert!(
        !system_prompt.is_empty(),
        "System prompt should not be empty"
    );

    // Verify resolved context has all layers
    let resolved = harness.get_resolved_context();
    assert!(resolved.has_layer("platform"));
    assert!(resolved.has_layer("organization"));
    assert!(resolved.has_layer("session"));
    assert!(resolved.has_layer("task"));

    // Verify task instructions are included
    assert!(resolved.task_instructions.is_some());
    let task_instr = resolved.task_instructions.as_ref().unwrap();
    assert!(task_instr.contains("authentication") || task_instr.contains("login"));

    // Verify skill descriptions are present
    assert!(!resolved.skill_descriptions.is_empty());
}

#[tokio::test]
async fn test_conversation_system_prompt_order() {
    let mut harness = AgentTestHarness::new();
    harness.create_task("Test task");

    // Add mock content to verify ordering
    let mut resolved = ResolvedContext::default();
    resolved.platform_rules = "PLATFORM_MARKER: Use English only.".to_string();
    resolved.org_conventions = Some("ORG_MARKER: Follow team standards.".to_string());
    resolved.user_preferences = Some("USER_MARKER: Prefer explicit types.".to_string());
    resolved.skill_descriptions = "SKILL_MARKER: Available skills here.".to_string();
    resolved.task_instructions = Some("TASK_MARKER: Fix the bug.".to_string());

    let generator = SystemPromptGenerator::new();
    let prompt = generator.generate(&resolved);

    // Verify order: Platform < Org < User < Skills < Task
    let platform_pos = prompt.find("PLATFORM_MARKER").unwrap();
    let org_pos = prompt.find("ORG_MARKER").unwrap();
    let user_pos = prompt.find("USER_MARKER").unwrap();
    let skill_pos = prompt.find("SKILL_MARKER").unwrap();
    let task_pos = prompt.find("TASK_MARKER").unwrap();

    assert!(platform_pos < org_pos, "Platform should come before Org");
    assert!(org_pos < user_pos, "Org should come before User");
    assert!(user_pos < skill_pos, "User should come before Skills");
    assert!(skill_pos < task_pos, "Skills should come before Task");
}

// ============================================================================
// Test Case 2: Two-Layer Skill Loading
// ============================================================================

#[tokio::test]
async fn test_two_layer_skill_loading() {
    let mut harness = AgentTestHarness::new();
    harness.create_task("Create a git commit");

    // Layer 1: Verify skill descriptions in system prompt
    let resolved = harness.get_resolved_context();
    let descriptions = &resolved.skill_descriptions;
    assert!(
        !descriptions.is_empty(),
        "Skill descriptions should be in Layer 1"
    );

    // Initially no loaded skills
    assert!(resolved.loaded_skills.is_empty(), "No skills loaded yet");

    // Layer 2: Simulate skill invocation
    harness.add_loaded_skill(
        "commit",
        "## Commit Skill\n\nUse conventional commits format:\n- feat: new feature\n- fix: bug fix",
    );

    // Verify skill is now loaded
    let resolved = harness.get_resolved_context();
    assert!(!resolved.loaded_skills.is_empty(), "Skill should be loaded");
    assert!(
        resolved.has_skill("commit"),
        "commit skill should be available"
    );

    let skill = resolved.get_skill("commit").unwrap();
    assert!(skill.content.contains("conventional commits"));

    // Generate prompt - should include loaded skill
    let prompt = harness.get_system_prompt();
    assert!(
        prompt.contains("commit") && prompt.contains("Active"),
        "System prompt should include active skill"
    );
}

#[tokio::test]
async fn test_skill_invocation_via_tool() {
    let harness = AgentTestHarness::new();

    // Configure mock tool response for invoke_skill
    harness
        .mock_tools()
        .set_tool_response(
            "invoke_skill",
            serde_json::json!({
                "success": true,
                "content": "## [review] Skill Loaded\n\nReview code changes carefully.",
                "requires_browser": false,
                "automation_tab": false
            }),
        )
        .await;

    // Simulate tool execution
    let tool_input = serde_json::json!({"name": "review"});
    let context = ToolContext::default();

    let result = harness
        .mock_tools()
        .execute("invoke_skill", tool_input, &context)
        .await
        .unwrap();

    assert!(result.get("success").unwrap().as_bool().unwrap());
    assert!(result
        .get("content")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("review"));

    // Verify call was recorded
    let history = harness.mock_tools().get_call_history().await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].0, "invoke_skill");
}

// ============================================================================
// Test Case 3: Tool Permission Enforcement
// ============================================================================

#[tokio::test]
async fn test_tool_permission_enforcement() {
    let mut harness = AgentTestHarness::new();
    harness.create_task("Test permissions");

    // Default: common tools should be allowed
    assert!(harness.is_tool_allowed("read"));
    assert!(harness.is_tool_allowed("write"));
    assert!(harness.is_tool_allowed("glob"));

    // Verify resolved context tool lists
    let _resolved = harness.get_resolved_context();

    // Test the is_tool_allowed logic
    let mut test_ctx = ResolvedContext::default();
    test_ctx.allowed_tools = vec!["read".to_string(), "write".to_string()];
    test_ctx.blocked_tools = vec!["dangerous".to_string()];

    assert!(test_ctx.is_tool_allowed("read"));
    assert!(test_ctx.is_tool_allowed("write"));
    assert!(!test_ctx.is_tool_allowed("dangerous")); // Blocked
    assert!(!test_ctx.is_tool_allowed("unknown")); // Not in allow list
}

#[tokio::test]
async fn test_tool_permission_with_mock_executor() {
    let harness = AgentTestHarness::new();
    let context = ToolContext::default();

    // Allowed tool should work
    let result = harness
        .mock_tools()
        .execute("read", serde_json::json!({"path": "test.txt"}), &context)
        .await;
    assert!(result.is_ok());

    // Block a tool
    harness.mock_tools().block_tool("read").await;

    // Now should fail
    let result = harness
        .mock_tools()
        .execute("read", serde_json::json!({"path": "test.txt"}), &context)
        .await;
    assert!(result.is_err());

    if let Err(AgentError::PermissionDenied(msg)) = result {
        assert!(msg.contains("read"));
    } else {
        panic!("Expected PermissionDenied error");
    }
}

// ============================================================================
// Test Case 4: SubAgent Forking
// ============================================================================

#[tokio::test]
async fn test_subagent_fork_isolation() {
    let mut harness = AgentTestHarness::new();
    harness.create_task("Main task: implement feature");

    // Verify main agent has session and task
    assert!(harness.integration.session().is_some());
    assert!(harness.integration.task().is_some());

    // Fork for subagent
    let forked = harness.fork_for_subagent("sub-1", "code-reviewer");

    // Forked should have subagent context but not session/task
    assert!(forked.subagent().is_some());
    assert!(
        forked.session().is_none(),
        "SubAgent should not inherit session"
    );
    assert!(forked.task().is_none(), "SubAgent should not inherit task");

    // But should inherit platform and organization
    assert!(
        forked.platform().is_some(),
        "SubAgent should inherit platform"
    );
    assert!(
        forked.organization().is_some(),
        "SubAgent should inherit organization"
    );

    // Verify subagent context
    let subagent = forked.subagent().unwrap();
    assert_eq!(subagent.agent_type, "code-reviewer");
}

#[tokio::test]
async fn test_subagent_creates_own_context() {
    let mut harness = AgentTestHarness::new();
    harness.create_task("Main task");

    // Fork for subagent
    let mut forked = harness.fork_for_subagent("sub-2", "explorer");

    // SubAgent creates its own session and task
    let sub_session_id = Uuid::new_v4();
    forked.create_session(sub_session_id);
    forked.create_task("SubAgent task: find relevant files");

    // Verify subagent has its own session and task
    assert!(forked.session().is_some());
    assert!(forked.task().is_some());

    let sub_session = forked.session().unwrap();
    assert_eq!(sub_session.session_id, sub_session_id);

    let sub_task = forked.task().unwrap();
    assert!(sub_task.description.contains("SubAgent task"));

    // Generate system prompt for subagent
    let sub_prompt = forked.generate_system_prompt();
    assert!(!sub_prompt.is_empty());
}

#[tokio::test]
async fn test_subagent_builder_modes() {
    // Test None mode - complete isolation
    let subagent_none = SubAgentContextBuilder::new("isolated")
        .id("sub-none")
        .fork_mode(ContextForkMode::None)
        .build();

    assert_eq!(subagent_none.fork_mode, ContextForkMode::None);
    assert!(subagent_none.forked_context.is_none());

    // Test Inherit mode with parent
    let parent_session = SessionContext::new(Uuid::new_v4());
    let subagent_inherit = SubAgentContextBuilder::new("inheritor")
        .id("sub-inherit")
        .parent(parent_session.session_id)
        .fork_mode(ContextForkMode::Inherit)
        .instructions("Review the code")
        .allow_tool("read")
        .block_tool("bash")
        .build();

    assert_eq!(
        subagent_inherit.parent_session_id,
        Some(parent_session.session_id)
    );
    assert!(subagent_inherit.allowed_tools.contains(&"read".to_string()));
    assert!(subagent_inherit.blocked_tools.contains(&"bash".to_string()));

    // Test Fork mode with full context copy
    let subagent_fork = SubAgentContext::fork("worker", &parent_session);
    assert_eq!(subagent_fork.fork_mode, ContextForkMode::Fork);
    assert!(subagent_fork.forked_context.is_some());
}

// ============================================================================
// Test Case 5: Session State Tracking
// ============================================================================

#[tokio::test]
async fn test_session_file_tracking() {
    let mut session = SessionContext::new(Uuid::new_v4());

    // Track file reads
    session.track_file("src/main.rs");
    session.track_file("src/lib.rs");
    session.track_file("Cargo.toml");

    assert_eq!(session.working_files.len(), 3);
    assert!(session.working_files.contains_key("src/main.rs"));

    // Mark file as modified
    session.mark_file_modified("src/lib.rs");

    let lib_state = session.working_files.get("src/lib.rs").unwrap();
    assert!(lib_state.modified);

    // Unmodified file should have modified = false
    let main_state = session.working_files.get("src/main.rs").unwrap();
    assert!(!main_state.modified);
}

#[tokio::test]
async fn test_session_tool_state_tracking() {
    let mut session = SessionContext::new(Uuid::new_v4());

    // Record tool calls
    session.record_tool_call("read", Some("src/main.rs".to_string()));
    session.record_tool_call("read", Some("src/lib.rs".to_string()));
    session.record_tool_call("write", Some("output.txt".to_string()));
    session.record_tool_call("read", Some("Cargo.toml".to_string()));

    // Verify tool states
    assert_eq!(session.tool_states.len(), 2); // read and write

    let read_state = session.tool_states.get("read").unwrap();
    assert_eq!(read_state.call_count, 3);
    assert_eq!(read_state.last_result, Some("Cargo.toml".to_string()));

    let write_state = session.tool_states.get("write").unwrap();
    assert_eq!(write_state.call_count, 1);
}

#[tokio::test]
async fn test_session_skill_loading() {
    let mut session = SessionContext::new(Uuid::new_v4());

    // Initially no skills loaded
    assert!(session.loaded_skills.is_empty());
    assert!(!session.is_skill_loaded("commit"));

    // Load a skill
    session.load_skill(LoadedSkill {
        name: "commit".to_string(),
        content: "Commit skill content".to_string(),
        requires_browser: false,
        automation_tab: false,
    });

    // Verify skill is loaded
    assert!(session.is_skill_loaded("commit"));
    assert!(!session.is_skill_loaded("review"));

    let names = session.loaded_skill_names();
    assert!(names.contains(&"commit"));

    // Load another skill
    session.load_skill(LoadedSkill {
        name: "review".to_string(),
        content: "Review skill content".to_string(),
        requires_browser: false,
        automation_tab: false,
    });

    assert!(session.is_skill_loaded("review"));
    assert_eq!(session.loaded_skills.len(), 2);
}

// ============================================================================
// Test Case 6: Task Context Working Memory
// ============================================================================

#[tokio::test]
async fn test_task_working_memory() {
    let mut task = TaskContextBuilder::new()
        .id("task-001")
        .description("Implement user authentication")
        .build();

    // Add discoveries
    task.add_discovery("Found auth module at src/auth.rs", "file_search", 0.95);
    task.add_discovery(
        "User model defined in src/models/user.rs",
        "code_analysis",
        0.88,
    );

    assert_eq!(task.working_memory.discoveries.len(), 2);

    // Add verifications
    task.add_verification(
        "compile",
        gateway_core::agent::context::VerificationType::Compile,
    );
    task.add_verification("lint", gateway_core::agent::context::VerificationType::Lint);
    task.add_verification(
        "unit-tests",
        gateway_core::agent::context::VerificationType::UnitTest,
    );

    assert_eq!(task.working_memory.pending_verifications.len(), 3);

    // Complete a verification
    task.complete_verification("compile", true);

    // Set tool state
    task.set_tool_state("bash", serde_json::json!({"last_command": "cargo build"}));

    let bash_state = task.get_tool_state("bash").unwrap();
    assert_eq!(bash_state["last_command"], "cargo build");

    // Set result
    task.set_result("auth_implemented", serde_json::json!(true));
    assert_eq!(
        task.get_result("auth_implemented").unwrap(),
        &serde_json::json!(true)
    );
}

#[tokio::test]
async fn test_task_constraints() {
    let task = TaskContextBuilder::new()
        .description("Constrained task")
        .max_iterations(10)
        .max_files(5)
        .allow_path("src/")
        .block_path("secrets/")
        .allow_tool("read")
        .allow_tool("glob")
        .block_tool("bash")
        .build();

    assert_eq!(task.constraints.max_iterations, Some(10));
    assert_eq!(task.constraints.max_files, Some(5));
    assert!(task.constraints.allowed_paths.contains(&"src/".to_string()));
    assert!(task
        .constraints
        .blocked_paths
        .contains(&"secrets/".to_string()));
    assert!(task.constraints.allowed_tools.contains(&"read".to_string()));
    assert!(task.constraints.blocked_tools.contains(&"bash".to_string()));

    // Test tool permission check
    assert!(task.is_tool_allowed("read"));
    assert!(task.is_tool_allowed("glob"));
    assert!(!task.is_tool_allowed("bash"));
}

// ============================================================================
// Test Case 7: End-to-End Conversation Simulation
// ============================================================================

#[tokio::test]
async fn test_e2e_conversation_simulation() {
    // Setup
    let mut harness = AgentTestHarness::new();
    harness.create_task("Fix the login bug in auth.rs");

    // Configure mock LLM responses
    harness.mock_llm().add_text_response(
        "I'll help you fix the login bug. Let me first read the auth.rs file to understand the issue."
    ).await;

    harness
        .mock_llm()
        .add_tool_use_response("read", serde_json::json!({"path": "src/auth.rs"}))
        .await;

    harness
        .mock_llm()
        .add_text_response(
            "I found the issue. The password comparison is case-sensitive. I'll fix it.",
        )
        .await;

    // Verify system prompt is generated correctly
    let system_prompt = harness.get_system_prompt();
    assert!(!system_prompt.is_empty());

    // Verify context layers
    let resolved = harness.get_resolved_context();
    assert!(resolved.has_layer("platform"));
    assert!(resolved.has_layer("session"));
    assert!(resolved.has_layer("task"));
    assert!(resolved
        .task_instructions
        .as_ref()
        .unwrap()
        .contains("login bug"));

    // Simulate tool execution
    let context = ToolContext::default();
    let read_result = harness
        .mock_tools()
        .execute("read", serde_json::json!({"path": "src/auth.rs"}), &context)
        .await
        .unwrap();

    assert!(read_result.get("content").is_some());

    // Verify call history
    let history = harness.mock_tools().get_call_history().await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].0, "read");
}

#[tokio::test]
async fn test_e2e_with_skill_invocation() {
    let mut harness = AgentTestHarness::new();
    harness.create_task("Create a commit for the bug fix");

    // Step 1: Check skill descriptions in context
    let resolved = harness.get_resolved_context();
    assert!(!resolved.skill_descriptions.is_empty());

    // Step 2: Simulate LLM deciding to invoke a skill
    harness
        .mock_llm()
        .add_tool_use_response("invoke_skill", serde_json::json!({"name": "commit"}))
        .await;

    // Step 3: Execute skill invocation
    let context = ToolContext::default();
    let skill_result = harness
        .mock_tools()
        .execute(
            "invoke_skill",
            serde_json::json!({"name": "commit"}),
            &context,
        )
        .await
        .unwrap();

    assert!(skill_result.get("success").unwrap().as_bool().unwrap());

    // Step 4: Add loaded skill to context
    let content = skill_result.get("content").unwrap().as_str().unwrap();
    harness.add_loaded_skill("commit", content);

    // Step 5: Verify skill is now in context
    let resolved = harness.get_resolved_context();
    assert!(resolved.has_skill("commit"));

    // Step 6: Generate prompt with loaded skill
    let prompt = harness.get_system_prompt();
    assert!(prompt.contains("commit") || prompt.contains("Skill Loaded"));
}

#[tokio::test]
async fn test_e2e_subagent_delegation() {
    let mut harness = AgentTestHarness::new();
    harness.create_task("Review and improve the codebase");

    // Main agent decides to delegate to subagent
    let mut subagent_ctx = harness.fork_for_subagent("reviewer-1", "code-reviewer");

    // SubAgent creates its own context
    subagent_ctx.create_session(Uuid::new_v4());
    subagent_ctx.create_task("Review auth module for security issues");

    // Verify isolation
    assert!(subagent_ctx.subagent().is_some());
    assert!(subagent_ctx.session().is_some());
    assert!(subagent_ctx.task().is_some());

    // SubAgent generates its own prompt
    let sub_prompt = subagent_ctx.generate_system_prompt();
    assert!(!sub_prompt.is_empty());

    // SubAgent task should be different from main task
    let sub_task = subagent_ctx.task().unwrap();
    assert!(sub_task.description.contains("security"));

    // Main agent task should be unchanged
    let main_task = harness.integration.task().unwrap();
    assert!(main_task.description.contains("Review and improve"));
}

// ============================================================================
// Test Case 8: Context Priority and Override
// ============================================================================

#[tokio::test]
async fn test_context_priority_override() {
    // Create contexts with overlapping settings
    let mut resolved = ResolvedContext::default();

    // Platform sets enforce_english = true (highest priority)
    resolved.enforce_english = true;

    // Platform blocks dangerous_tool
    resolved.blocked_tools.push("dangerous_tool".to_string());

    // Organization allows specific tools
    resolved.allowed_tools.push("read".to_string());
    resolved.allowed_tools.push("write".to_string());

    // User tries to unblock (should NOT work - lower priority)
    // In a real scenario, lower priority cannot remove from blocked_tools

    // Verify platform rules are enforced
    assert!(resolved.enforce_english);
    assert!(!resolved.is_tool_allowed("dangerous_tool")); // Still blocked
    assert!(resolved.is_tool_allowed("read")); // Allowed by org
}

#[tokio::test]
async fn test_config_value_priority() {
    use gateway_core::agent::context::ContextPriority;
    use gateway_core::agent::ContextResolver;

    let mut resolver = ContextResolver::new();
    let mut resolved = ResolvedContext::default();

    // Platform sets a config value (highest priority)
    resolver.set_config(
        &mut resolved,
        "max_tokens",
        serde_json::json!(4096),
        ContextPriority::Platform,
    );

    // User tries to override (should fail - lower priority)
    resolver.set_config(
        &mut resolved,
        "max_tokens",
        serde_json::json!(8192),
        ContextPriority::User,
    );

    // Platform value should remain
    assert_eq!(resolved.config_values["max_tokens"], 4096);

    // User can set new keys
    resolver.set_config(
        &mut resolved,
        "user_preference",
        serde_json::json!("dark_mode"),
        ContextPriority::User,
    );

    assert_eq!(resolved.config_values["user_preference"], "dark_mode");
}
