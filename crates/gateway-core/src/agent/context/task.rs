//! Task Context Manager
//!
//! Manages task-level context including task description, loaded skill content,
//! working memory, and constraints. Task context is scoped to the current task
//! and contains all the information needed to execute that specific task.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use super::resolver::{ContextLayer, ContextPriority, LoadedSkill, ResolvedContext};

// ============================================================================
// Task Context Types
// ============================================================================

/// Task-level context configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    /// Task unique identifier
    pub task_id: String,

    /// Parent session ID
    pub session_id: Option<Uuid>,

    /// Task description/goal
    pub description: String,

    /// Skills loaded for this task (full content)
    pub loaded_skills: Vec<LoadedSkill>,

    /// Working memory for the task
    pub working_memory: WorkingMemory,

    /// Task constraints
    pub constraints: TaskConstraints,

    /// Task-specific instructions (injected into system prompt)
    pub instructions: Option<String>,
}

/// Working memory during task execution
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkingMemory {
    /// Current file being worked on
    pub current_file: Option<String>,

    /// Files discovered/read during task
    pub discovered_files: Vec<String>,

    /// Tool execution states
    pub tool_states: HashMap<String, serde_json::Value>,

    /// Discoveries made during task
    pub discoveries: Vec<Discovery>,

    /// Pending verifications
    pub pending_verifications: Vec<Verification>,

    /// Intermediate results
    pub results: HashMap<String, serde_json::Value>,
}

/// A discovery made during task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Discovery {
    /// What was discovered
    pub content: String,

    /// Source of the discovery (file, tool, etc.)
    pub source: String,

    /// Relevance score (0-1)
    pub relevance: f32,
}

/// A pending verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    /// What needs to be verified
    pub target: String,

    /// Verification type
    pub verification_type: VerificationType,

    /// Current status
    pub status: VerificationStatus,
}

/// Type of verification
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VerificationType {
    /// Compile check
    Compile,
    /// Lint check
    Lint,
    /// Unit test
    UnitTest,
    /// Integration test
    IntegrationTest,
    /// Manual verification
    Manual,
}

/// Status of a verification
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum VerificationStatus {
    #[default]
    Pending,
    Running,
    Passed,
    Failed,
    Skipped,
}

/// Constraints for task execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskConstraints {
    /// Maximum files that can be modified
    pub max_files: Option<usize>,

    /// Allowed file patterns (glob)
    pub allowed_paths: Vec<String>,

    /// Blocked file patterns (glob)
    pub blocked_paths: Vec<String>,

    /// Allowed tools
    pub allowed_tools: Vec<String>,

    /// Blocked tools
    pub blocked_tools: Vec<String>,

    /// Time limit in seconds
    pub time_limit_secs: Option<u64>,

    /// Maximum iterations/retries
    pub max_iterations: Option<u32>,
}

impl Default for TaskConstraints {
    fn default() -> Self {
        Self {
            max_files: None,
            allowed_paths: Vec::new(),
            blocked_paths: Vec::new(),
            allowed_tools: Vec::new(),
            blocked_tools: Vec::new(),
            time_limit_secs: None,
            max_iterations: Some(10),
        }
    }
}

impl Default for TaskContext {
    fn default() -> Self {
        Self {
            task_id: Uuid::new_v4().to_string(),
            session_id: None,
            description: String::new(),
            loaded_skills: Vec::new(),
            working_memory: WorkingMemory::default(),
            constraints: TaskConstraints::default(),
            instructions: None,
        }
    }
}

impl TaskContext {
    /// Create a new task context with description
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            ..Default::default()
        }
    }

    /// Create a task context with ID and description
    pub fn with_id(task_id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            description: description.into(),
            ..Default::default()
        }
    }

    /// Set the parent session
    pub fn set_session(&mut self, session_id: Uuid) {
        self.session_id = Some(session_id);
    }

    /// Load a skill for this task
    pub fn load_skill(&mut self, skill: LoadedSkill) {
        if !self.loaded_skills.iter().any(|s| s.name == skill.name) {
            self.loaded_skills.push(skill);
        }
    }

    /// Set the current working file
    pub fn set_current_file(&mut self, path: &str) {
        self.working_memory.current_file = Some(path.to_string());
        if !self
            .working_memory
            .discovered_files
            .contains(&path.to_string())
        {
            self.working_memory.discovered_files.push(path.to_string());
        }
    }

    /// Add a discovery
    pub fn add_discovery(&mut self, content: &str, source: &str, relevance: f32) {
        self.working_memory.discoveries.push(Discovery {
            content: content.to_string(),
            source: source.to_string(),
            relevance,
        });
    }

    /// Add a pending verification
    pub fn add_verification(&mut self, target: &str, verification_type: VerificationType) {
        self.working_memory
            .pending_verifications
            .push(Verification {
                target: target.to_string(),
                verification_type,
                status: VerificationStatus::Pending,
            });
    }

    /// Mark a verification as complete
    pub fn complete_verification(&mut self, target: &str, passed: bool) {
        for v in &mut self.working_memory.pending_verifications {
            if v.target == target {
                v.status = if passed {
                    VerificationStatus::Passed
                } else {
                    VerificationStatus::Failed
                };
                break;
            }
        }
    }

    /// Store a tool state
    pub fn set_tool_state(&mut self, tool: &str, state: serde_json::Value) {
        self.working_memory
            .tool_states
            .insert(tool.to_string(), state);
    }

    /// Get a tool state
    pub fn get_tool_state(&self, tool: &str) -> Option<&serde_json::Value> {
        self.working_memory.tool_states.get(tool)
    }

    /// Store an intermediate result
    pub fn set_result(&mut self, key: &str, value: serde_json::Value) {
        self.working_memory.results.insert(key.to_string(), value);
    }

    /// Get an intermediate result
    pub fn get_result(&self, key: &str) -> Option<&serde_json::Value> {
        self.working_memory.results.get(key)
    }

    /// Set task instructions
    pub fn set_instructions(&mut self, instructions: impl Into<String>) {
        self.instructions = Some(instructions.into());
    }

    /// Check if a tool is allowed
    pub fn is_tool_allowed(&self, tool: &str) -> bool {
        if self.constraints.blocked_tools.contains(&tool.to_string()) {
            return false;
        }
        if self.constraints.allowed_tools.is_empty() {
            return true;
        }
        self.constraints.allowed_tools.contains(&tool.to_string())
    }

    /// Generate working memory summary for context
    fn generate_working_memory_summary(&self) -> String {
        let mut summary = String::new();

        // Current file
        if let Some(file) = &self.working_memory.current_file {
            summary.push_str(&format!("Current file: {}\n\n", file));
        }

        // Discoveries
        if !self.working_memory.discoveries.is_empty() {
            summary.push_str("### Discoveries\n\n");
            for d in &self.working_memory.discoveries {
                summary.push_str(&format!("- {} (from {})\n", d.content, d.source));
            }
            summary.push('\n');
        }

        // Pending verifications
        let pending: Vec<_> = self
            .working_memory
            .pending_verifications
            .iter()
            .filter(|v| v.status == VerificationStatus::Pending)
            .collect();

        if !pending.is_empty() {
            summary.push_str("### Pending Verifications\n\n");
            for v in pending {
                summary.push_str(&format!("- {:?}: {}\n", v.verification_type, v.target));
            }
        }

        summary
    }
}

// ============================================================================
// ContextLayer Implementation
// ============================================================================

impl ContextLayer for TaskContext {
    fn layer_name(&self) -> &str {
        "task"
    }

    fn priority(&self) -> ContextPriority {
        ContextPriority::Task
    }

    fn apply_to(&self, resolved: &mut ResolvedContext) {
        // Add loaded skills
        for skill in &self.loaded_skills {
            if !resolved.loaded_skills.iter().any(|s| s.name == skill.name) {
                resolved.loaded_skills.push(skill.clone());
            }
        }

        // Apply tool constraints
        if !self.constraints.allowed_tools.is_empty() {
            resolved.allowed_tools = self.constraints.allowed_tools.clone();
        }
        if !self.constraints.blocked_tools.is_empty() {
            resolved
                .blocked_tools
                .extend(self.constraints.blocked_tools.clone());
        }

        // Build task instructions
        let mut instructions = String::new();

        // Add task description
        instructions.push_str("## Task\n\n");
        instructions.push_str(&self.description);
        instructions.push_str("\n\n");

        // Add explicit instructions if set
        if let Some(inst) = &self.instructions {
            instructions.push_str("## Instructions\n\n");
            instructions.push_str(inst);
            instructions.push_str("\n\n");
        }

        // Add working memory summary
        let wm_summary = self.generate_working_memory_summary();
        if !wm_summary.is_empty() {
            instructions.push_str("## Working Memory\n\n");
            instructions.push_str(&wm_summary);
        }

        // Add constraints if any
        if self.constraints.max_files.is_some()
            || !self.constraints.allowed_paths.is_empty()
            || !self.constraints.blocked_paths.is_empty()
        {
            instructions.push_str("\n## Constraints\n\n");
            if let Some(max) = self.constraints.max_files {
                instructions.push_str(&format!("- Max files to modify: {}\n", max));
            }
            if !self.constraints.allowed_paths.is_empty() {
                instructions.push_str(&format!(
                    "- Allowed paths: {}\n",
                    self.constraints.allowed_paths.join(", ")
                ));
            }
            if !self.constraints.blocked_paths.is_empty() {
                instructions.push_str(&format!(
                    "- Blocked paths: {}\n",
                    self.constraints.blocked_paths.join(", ")
                ));
            }
        }

        // Set or append to task instructions
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

/// Builder for TaskContext
pub struct TaskContextBuilder {
    context: TaskContext,
}

impl TaskContextBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            context: TaskContext::default(),
        }
    }

    /// Set task ID
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.context.task_id = id.into();
        self
    }

    /// Set description
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.context.description = desc.into();
        self
    }

    /// Set session ID
    pub fn session(mut self, session_id: Uuid) -> Self {
        self.context.session_id = Some(session_id);
        self
    }

    /// Add a skill
    pub fn skill(mut self, skill: LoadedSkill) -> Self {
        self.context.loaded_skills.push(skill);
        self
    }

    /// Set instructions
    pub fn instructions(mut self, inst: impl Into<String>) -> Self {
        self.context.instructions = Some(inst.into());
        self
    }

    /// Set max files constraint
    pub fn max_files(mut self, max: usize) -> Self {
        self.context.constraints.max_files = Some(max);
        self
    }

    /// Add allowed path
    pub fn allow_path(mut self, path: impl Into<String>) -> Self {
        self.context.constraints.allowed_paths.push(path.into());
        self
    }

    /// Add blocked path
    pub fn block_path(mut self, path: impl Into<String>) -> Self {
        self.context.constraints.blocked_paths.push(path.into());
        self
    }

    /// Add allowed tool
    pub fn allow_tool(mut self, tool: impl Into<String>) -> Self {
        self.context.constraints.allowed_tools.push(tool.into());
        self
    }

    /// Add blocked tool
    pub fn block_tool(mut self, tool: impl Into<String>) -> Self {
        self.context.constraints.blocked_tools.push(tool.into());
        self
    }

    /// Set max iterations
    pub fn max_iterations(mut self, max: u32) -> Self {
        self.context.constraints.max_iterations = Some(max);
        self
    }

    /// Build the context
    pub fn build(self) -> TaskContext {
        self.context
    }
}

impl Default for TaskContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_context_default() {
        let ctx = TaskContext::default();
        assert!(!ctx.task_id.is_empty());
        assert!(ctx.description.is_empty());
        assert!(ctx.loaded_skills.is_empty());
    }

    #[test]
    fn test_task_context_new() {
        let ctx = TaskContext::new("Fix the bug");
        assert_eq!(ctx.description, "Fix the bug");
    }

    #[test]
    fn test_task_context_with_id() {
        let ctx = TaskContext::with_id("task-123", "Implement feature");
        assert_eq!(ctx.task_id, "task-123");
        assert_eq!(ctx.description, "Implement feature");
    }

    #[test]
    fn test_load_skill() {
        let mut ctx = TaskContext::default();
        let skill = LoadedSkill {
            name: "test-skill".to_string(),
            content: "content".to_string(),
            requires_browser: false,
            automation_tab: false,
        };

        ctx.load_skill(skill.clone());
        assert_eq!(ctx.loaded_skills.len(), 1);

        // Duplicate should not add
        ctx.load_skill(skill);
        assert_eq!(ctx.loaded_skills.len(), 1);
    }

    #[test]
    fn test_set_current_file() {
        let mut ctx = TaskContext::default();
        ctx.set_current_file("/path/to/file.rs");

        assert_eq!(
            ctx.working_memory.current_file,
            Some("/path/to/file.rs".to_string())
        );
        assert!(ctx
            .working_memory
            .discovered_files
            .contains(&"/path/to/file.rs".to_string()));
    }

    #[test]
    fn test_add_discovery() {
        let mut ctx = TaskContext::default();
        ctx.add_discovery("Found pattern X", "file.rs:42", 0.8);

        assert_eq!(ctx.working_memory.discoveries.len(), 1);
        assert_eq!(ctx.working_memory.discoveries[0].content, "Found pattern X");
        assert!((ctx.working_memory.discoveries[0].relevance - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn test_verification_lifecycle() {
        let mut ctx = TaskContext::default();
        ctx.add_verification("cargo check", VerificationType::Compile);

        assert_eq!(ctx.working_memory.pending_verifications.len(), 1);
        assert_eq!(
            ctx.working_memory.pending_verifications[0].status,
            VerificationStatus::Pending
        );

        ctx.complete_verification("cargo check", true);
        assert_eq!(
            ctx.working_memory.pending_verifications[0].status,
            VerificationStatus::Passed
        );
    }

    #[test]
    fn test_tool_state() {
        let mut ctx = TaskContext::default();
        ctx.set_tool_state("bash", serde_json::json!({"cwd": "/home"}));

        let state = ctx.get_tool_state("bash");
        assert!(state.is_some());
        assert_eq!(
            state.unwrap()["cwd"],
            "home".to_string().replace("home", "/home")
        );
    }

    #[test]
    fn test_is_tool_allowed() {
        let mut ctx = TaskContext::default();

        // No constraints = all allowed
        assert!(ctx.is_tool_allowed("read"));

        // Block a tool
        ctx.constraints.blocked_tools.push("bash".to_string());
        assert!(!ctx.is_tool_allowed("bash"));
        assert!(ctx.is_tool_allowed("read"));

        // Allow list takes precedence
        ctx.constraints.allowed_tools.push("read".to_string());
        assert!(ctx.is_tool_allowed("read"));
        assert!(!ctx.is_tool_allowed("write")); // Not in allow list
    }

    #[test]
    fn test_context_layer_metadata() {
        let ctx = TaskContext::default();
        assert_eq!(ctx.layer_name(), "task");
        assert_eq!(ctx.priority(), ContextPriority::Task);
    }

    #[test]
    fn test_apply_to() {
        let mut ctx = TaskContext::new("Fix the bug in auth module");
        ctx.load_skill(LoadedSkill {
            name: "bug-fix".to_string(),
            content: "Bug fix skill".to_string(),
            requires_browser: false,
            automation_tab: false,
        });
        ctx.constraints.blocked_tools.push("bash".to_string());
        ctx.constraints.max_files = Some(5);

        let mut resolved = ResolvedContext::default();
        ctx.apply_to(&mut resolved);

        assert_eq!(resolved.loaded_skills.len(), 1);
        assert!(resolved.blocked_tools.contains(&"bash".to_string()));
        assert!(resolved.task_instructions.is_some());

        let instructions = resolved.task_instructions.unwrap();
        assert!(instructions.contains("Fix the bug in auth module"));
        assert!(instructions.contains("Max files"));
        assert!(instructions.contains("5"));
    }

    #[test]
    fn test_builder() {
        let ctx = TaskContextBuilder::new()
            .id("task-1")
            .description("Build feature X")
            .max_files(3)
            .allow_path("src/**")
            .block_tool("bash")
            .build();

        assert_eq!(ctx.task_id, "task-1");
        assert_eq!(ctx.description, "Build feature X");
        assert_eq!(ctx.constraints.max_files, Some(3));
        assert!(ctx
            .constraints
            .allowed_paths
            .contains(&"src/**".to_string()));
        assert!(ctx.constraints.blocked_tools.contains(&"bash".to_string()));
    }

    #[test]
    fn test_serde_round_trip() {
        let mut ctx = TaskContext::new("Test task");
        ctx.set_current_file("/test.rs");
        ctx.add_discovery("Found X", "source", 0.5);

        let json = serde_json::to_string(&ctx).expect("serialize");
        let restored: TaskContext = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(ctx.task_id, restored.task_id);
        assert_eq!(ctx.description, restored.description);
        assert_eq!(
            ctx.working_memory.current_file,
            restored.working_memory.current_file
        );
    }

    #[test]
    fn test_working_memory_summary() {
        let mut ctx = TaskContext::default();
        ctx.set_current_file("/file.rs");
        ctx.add_discovery("Important pattern", "analysis", 0.9);
        ctx.add_verification("cargo test", VerificationType::UnitTest);

        let summary = ctx.generate_working_memory_summary();
        assert!(summary.contains("/file.rs"));
        assert!(summary.contains("Important pattern"));
        assert!(summary.contains("cargo test"));
    }
}
