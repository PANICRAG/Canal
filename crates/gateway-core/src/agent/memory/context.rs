//! Hierarchical Context Memory System
//!
//! This module implements the Manus-style hierarchical memory pattern:
//! - WorkingMemory: Immediate task context (current task, tool states)
//! - SessionMemory: Conversation history within a session
//! - LongTermMemory: Persistent user preferences and learned patterns
//! - TeamMemory: Shared workflows and templates (optional)
//!
//! ## Context Window Management
//!
//! The ContextManager handles intelligent context compression when
//! approaching token limits, using importance-based prioritization.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::store::MemoryError;
use crate::agent::types::UserMemory;

// ============================================================================
// Working Memory
// ============================================================================

/// Working memory for immediate task context
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkingMemory {
    /// Current task tree
    pub task_tree: TaskTree,
    /// Active tool states
    pub tool_states: HashMap<String, ToolState>,
    /// Pending verifications
    pub pending_verifications: Vec<Verification>,
    /// Checkpoint IDs for potential rollback
    pub checkpoints: Vec<String>,
    /// Variables for passing data between tool calls
    pub variables: HashMap<String, serde_json::Value>,
    /// Last updated timestamp
    pub updated_at: DateTime<Utc>,
}

impl WorkingMemory {
    /// Create new working memory
    pub fn new() -> Self {
        Self {
            task_tree: TaskTree::new(),
            tool_states: HashMap::new(),
            pending_verifications: Vec::new(),
            checkpoints: Vec::new(),
            variables: HashMap::new(),
            updated_at: Utc::now(),
        }
    }

    /// Start a new task
    pub fn start_task(&mut self, description: impl Into<String>) -> String {
        let task = TaskNode::new(description);
        let id = task.id.clone();
        self.task_tree.add_node(task);
        self.updated_at = Utc::now();
        id
    }

    /// Complete a task
    pub fn complete_task(&mut self, task_id: &str, result: TaskResult) {
        if let Some(task) = self.task_tree.get_mut(task_id) {
            task.status = TaskStatus::Completed;
            task.result = Some(result);
        }
        self.updated_at = Utc::now();
    }

    /// Fail a task
    pub fn fail_task(&mut self, task_id: &str, error: impl Into<String>) {
        if let Some(task) = self.task_tree.get_mut(task_id) {
            task.status = TaskStatus::Failed;
            task.result = Some(TaskResult::Error(error.into()));
        }
        self.updated_at = Utc::now();
    }

    /// Record a tool call for a task
    pub fn record_tool_call(&mut self, task_id: &str, tool_call: ToolCallRecord) {
        if let Some(task) = self.task_tree.get_mut(task_id) {
            task.tool_calls.push(tool_call);
        }
        self.updated_at = Utc::now();
    }

    /// Set a tool state
    pub fn set_tool_state(&mut self, tool_name: impl Into<String>, state: ToolState) {
        self.tool_states.insert(tool_name.into(), state);
        self.updated_at = Utc::now();
    }

    /// Get a tool state
    pub fn get_tool_state(&self, tool_name: &str) -> Option<&ToolState> {
        self.tool_states.get(tool_name)
    }

    /// Add a pending verification
    pub fn add_verification(&mut self, verification: Verification) {
        self.pending_verifications.push(verification);
        self.updated_at = Utc::now();
    }

    /// Complete a verification
    pub fn complete_verification(&mut self, verification_id: &str, passed: bool) {
        if let Some(v) = self
            .pending_verifications
            .iter_mut()
            .find(|v| v.id == verification_id)
        {
            v.status = if passed {
                VerificationStatus::Passed
            } else {
                VerificationStatus::Failed
            };
        }
        self.updated_at = Utc::now();
    }

    /// Store a variable
    pub fn set_variable(&mut self, name: impl Into<String>, value: serde_json::Value) {
        self.variables.insert(name.into(), value);
        self.updated_at = Utc::now();
    }

    /// Get a variable
    pub fn get_variable(&self, name: &str) -> Option<&serde_json::Value> {
        self.variables.get(name)
    }

    /// Add a checkpoint ID
    pub fn add_checkpoint(&mut self, checkpoint_id: impl Into<String>) {
        self.checkpoints.push(checkpoint_id.into());
        self.updated_at = Utc::now();
    }

    /// Generate a status summary for context injection
    pub fn generate_status_summary(&self) -> String {
        let current_task = self.task_tree.current_task();
        let current_desc = current_task
            .map(|t| t.description.as_str())
            .unwrap_or("none");

        let (completed, total) = self.task_tree.progress();
        let pending_count = self
            .pending_verifications
            .iter()
            .filter(|v| v.status == VerificationStatus::Pending)
            .count();

        format!(
            r#"<working_memory>
current_task: {}
progress: {}/{}
active_tools: {:?}
pending_verifications: {}
checkpoints: {}
variables: {:?}
</working_memory>"#,
            current_desc,
            completed,
            total,
            self.tool_states.keys().collect::<Vec<_>>(),
            pending_count,
            self.checkpoints.len(),
            self.variables.keys().collect::<Vec<_>>(),
        )
    }

    /// Clear working memory for a new task
    pub fn clear(&mut self) {
        self.task_tree = TaskTree::new();
        self.tool_states.clear();
        self.pending_verifications.clear();
        self.variables.clear();
        // Keep checkpoints for potential rollback
        self.updated_at = Utc::now();
    }
}

// ============================================================================
// Task Tree
// ============================================================================

/// Task tree for tracking hierarchical tasks
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskTree {
    /// Root task nodes
    pub nodes: Vec<TaskNode>,
    /// Current active task ID
    pub current_id: Option<String>,
}

impl TaskTree {
    /// Create a new empty task tree
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            current_id: None,
        }
    }

    /// Add a task node
    pub fn add_node(&mut self, node: TaskNode) {
        if self.current_id.is_none() {
            self.current_id = Some(node.id.clone());
        }
        self.nodes.push(node);
    }

    /// Get a task node by ID
    pub fn get(&self, id: &str) -> Option<&TaskNode> {
        Self::find_recursive(&self.nodes, id)
    }

    /// Get a mutable task node by ID
    pub fn get_mut(&mut self, id: &str) -> Option<&mut TaskNode> {
        Self::find_recursive_mut(&mut self.nodes, id)
    }

    fn find_recursive<'a>(nodes: &'a [TaskNode], id: &str) -> Option<&'a TaskNode> {
        for node in nodes {
            if node.id == id {
                return Some(node);
            }
            if let Some(found) = Self::find_recursive(&node.children, id) {
                return Some(found);
            }
        }
        None
    }

    fn find_recursive_mut<'a>(nodes: &'a mut [TaskNode], id: &str) -> Option<&'a mut TaskNode> {
        for node in nodes {
            if node.id == id {
                return Some(node);
            }
            if let Some(found) = Self::find_recursive_mut(&mut node.children, id) {
                return Some(found);
            }
        }
        None
    }

    /// Get the current active task
    pub fn current_task(&self) -> Option<&TaskNode> {
        self.current_id.as_ref().and_then(|id| self.get(id))
    }

    /// Get progress as (completed, total)
    pub fn progress(&self) -> (usize, usize) {
        let mut completed = 0;
        let mut total = 0;
        self.count_recursive(&self.nodes, &mut completed, &mut total);
        (completed, total)
    }

    fn count_recursive(&self, nodes: &[TaskNode], completed: &mut usize, total: &mut usize) {
        for node in nodes {
            *total += 1;
            if node.status == TaskStatus::Completed {
                *completed += 1;
            }
            self.count_recursive(&node.children, completed, total);
        }
    }

    /// Get the tree depth
    pub fn depth(&self) -> usize {
        self.depth_recursive(&self.nodes)
    }

    fn depth_recursive(&self, nodes: &[TaskNode]) -> usize {
        nodes
            .iter()
            .map(|n| 1 + self.depth_recursive(&n.children))
            .max()
            .unwrap_or(0)
    }
}

/// A task node in the task tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskNode {
    /// Unique task ID
    pub id: String,
    /// Task description
    pub description: String,
    /// Task status
    pub status: TaskStatus,
    /// Child tasks
    pub children: Vec<TaskNode>,
    /// Tool calls made for this task
    pub tool_calls: Vec<ToolCallRecord>,
    /// Task result
    pub result: Option<TaskResult>,
    /// Created timestamp
    pub created_at: DateTime<Utc>,
}

impl TaskNode {
    /// Create a new task node
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            description: description.into(),
            status: TaskStatus::Pending,
            children: Vec::new(),
            tool_calls: Vec::new(),
            result: None,
            created_at: Utc::now(),
        }
    }

    /// Add a child task
    pub fn add_child(&mut self, child: TaskNode) {
        self.children.push(child);
    }
}

/// Task status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

impl Default for TaskStatus {
    fn default() -> Self {
        Self::Pending
    }
}

/// Task result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskResult {
    Success(serde_json::Value),
    Error(String),
    Skipped(String),
}

/// Record of a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    /// Tool name
    pub tool: String,
    /// Tool parameters
    pub params: serde_json::Value,
    /// Tool result
    pub result: Option<serde_json::Value>,
    /// Whether the call succeeded
    pub success: bool,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

impl ToolCallRecord {
    /// Create a new tool call record
    pub fn new(tool: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            tool: tool.into(),
            params,
            result: None,
            success: false,
            timestamp: Utc::now(),
        }
    }

    /// Set the result
    pub fn with_result(mut self, result: serde_json::Value, success: bool) -> Self {
        self.result = Some(result);
        self.success = success;
        self
    }
}

// ============================================================================
// Tool State
// ============================================================================

/// State of an active tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolState {
    /// Tool name
    pub tool_name: String,
    /// Current state
    pub state: ToolStateValue,
    /// Last activity timestamp
    pub last_activity: DateTime<Utc>,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

impl ToolState {
    /// Record a tool execution result, updating state and timestamp.
    pub fn record_execution(&mut self, success: bool) {
        self.last_activity = Utc::now();
        if success {
            self.state = ToolStateValue::Idle;
        } else {
            self.state = ToolStateValue::Error {
                message: "Execution failed".to_string(),
                occurred_at: Utc::now(),
            };
        }
    }
}

/// Tool state value
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStateValue {
    /// Tool is idle
    Idle,
    /// Tool is executing
    Executing {
        started_at: DateTime<Utc>,
        operation: String,
    },
    /// Tool has an error
    Error {
        message: String,
        occurred_at: DateTime<Utc>,
    },
    /// Tool is rate limited
    RateLimited { until: DateTime<Utc> },
}

impl ToolState {
    /// Create a new tool state
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            state: ToolStateValue::Idle,
            last_activity: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Set to executing state
    pub fn set_executing(&mut self, operation: impl Into<String>) {
        self.state = ToolStateValue::Executing {
            started_at: Utc::now(),
            operation: operation.into(),
        };
        self.last_activity = Utc::now();
    }

    /// Set to idle state
    pub fn set_idle(&mut self) {
        self.state = ToolStateValue::Idle;
        self.last_activity = Utc::now();
    }

    /// Set to error state
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.state = ToolStateValue::Error {
            message: message.into(),
            occurred_at: Utc::now(),
        };
        self.last_activity = Utc::now();
    }

    /// Check if the tool is executing
    pub fn is_executing(&self) -> bool {
        matches!(self.state, ToolStateValue::Executing { .. })
    }
}

// ============================================================================
// Verification
// ============================================================================

/// A pending verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    /// Unique verification ID
    pub id: String,
    /// Description of what needs to be verified
    pub description: String,
    /// Tool call that created this verification
    pub tool_call_id: Option<String>,
    /// Verification status
    pub status: VerificationStatus,
    /// Created timestamp
    pub created_at: DateTime<Utc>,
}

impl Verification {
    /// Create a new verification
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            description: description.into(),
            tool_call_id: None,
            status: VerificationStatus::Pending,
            created_at: Utc::now(),
        }
    }

    /// Set the tool call ID
    pub fn with_tool_call(mut self, tool_call_id: impl Into<String>) -> Self {
        self.tool_call_id = Some(tool_call_id.into());
        self
    }
}

/// Verification status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Pending,
    Passed,
    Failed,
    Skipped,
}

impl Default for VerificationStatus {
    fn default() -> Self {
        Self::Pending
    }
}

// ============================================================================
// Session Memory
// ============================================================================

/// A simple message for session memory (not wire-protocol AgentMessage)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    /// Message role (user, assistant, system)
    pub role: String,
    /// Message content
    pub content: String,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// Token estimate for this message
    pub estimated_tokens: usize,
}

impl SessionMessage {
    /// Create a new session message
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        let content = content.into();
        let estimated_tokens = content.len() / 4; // rough estimate
        Self {
            role: role.into(),
            content,
            timestamp: Utc::now(),
            estimated_tokens,
        }
    }

    /// Create a user message
    pub fn user(content: impl Into<String>) -> Self {
        Self::new("user", content)
    }

    /// Create an assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new("assistant", content)
    }

    /// Create a system message
    pub fn system(content: impl Into<String>) -> Self {
        Self::new("system", content)
    }
}

/// Session memory for conversation history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMemory {
    /// Session ID
    pub session_id: String,
    /// Conversation messages
    pub messages: Vec<SessionMessage>,
    /// Summary of earlier messages (if compacted)
    pub summary: Option<String>,
    /// Turn count
    pub turn_count: u32,
    /// Token estimate for messages
    pub estimated_tokens: usize,
    /// Created timestamp
    pub created_at: DateTime<Utc>,
    /// Updated timestamp
    pub updated_at: DateTime<Utc>,
}

impl SessionMemory {
    /// Create new session memory
    pub fn new(session_id: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            session_id: session_id.into(),
            messages: Vec::new(),
            summary: None,
            turn_count: 0,
            estimated_tokens: 0,
            created_at: now,
            updated_at: now,
        }
    }

    /// Add a message
    pub fn add_message(&mut self, message: SessionMessage) {
        self.estimated_tokens += message.estimated_tokens;
        self.messages.push(message);
        self.updated_at = Utc::now();
    }

    /// Add a user message
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.add_message(SessionMessage::user(content));
    }

    /// Add an assistant message
    pub fn add_assistant_message(&mut self, content: impl Into<String>) {
        self.add_message(SessionMessage::assistant(content));
    }

    /// Increment turn count
    pub fn increment_turn(&mut self) {
        self.turn_count += 1;
        self.updated_at = Utc::now();
    }

    /// Get recent messages (last n)
    pub fn recent_messages(&self, n: usize) -> &[SessionMessage] {
        let start = self.messages.len().saturating_sub(n);
        &self.messages[start..]
    }

    /// Set summary from compaction
    pub fn set_summary(&mut self, summary: impl Into<String>) {
        self.summary = Some(summary.into());
        self.updated_at = Utc::now();
    }

    /// Clear old messages after compaction
    pub fn compact(&mut self, keep_recent: usize) {
        if self.messages.len() > keep_recent {
            let removed_count = self.messages.len() - keep_recent;
            self.messages.drain(0..removed_count);
            // Recalculate token estimate
            self.estimated_tokens = self.messages.iter().map(|m| m.estimated_tokens).sum();
            self.updated_at = Utc::now();
        }
    }
}

// ============================================================================
// Context Memory (Hierarchical)
// ============================================================================

/// Hierarchical context memory combining all memory layers
#[derive(Debug, Clone)]
pub struct ContextMemory {
    /// Working memory (current task)
    pub working: WorkingMemory,
    /// Session memory (conversation history)
    pub session: SessionMemory,
    /// Long-term memory (user preferences, learned patterns)
    pub long_term: UserMemory,
    /// Team memory (shared workflows, templates)
    pub team: Option<TeamMemory>,
}

impl ContextMemory {
    /// Create new context memory
    pub fn new(session_id: impl Into<String>, user_id: impl Into<String>) -> Self {
        Self {
            working: WorkingMemory::new(),
            session: SessionMemory::new(session_id),
            long_term: UserMemory::new(user_id),
            team: None,
        }
    }

    /// Set team memory
    pub fn with_team_memory(mut self, team: TeamMemory) -> Self {
        self.team = Some(team);
        self
    }

    /// Get total estimated tokens
    pub fn estimated_tokens(&self) -> usize {
        // Working memory estimate (rough)
        let working_tokens = serde_json::to_string(&self.working)
            .map(|s| s.len() / 4)
            .unwrap_or(0);

        // Session tokens
        let session_tokens = self.session.estimated_tokens
            + self
                .session
                .summary
                .as_ref()
                .map(|s| s.len() / 4)
                .unwrap_or(0);

        // Long-term memory tokens (only include in context if needed)
        let long_term_tokens = self.long_term.format_for_prompt().len() / 4;

        working_tokens + session_tokens + long_term_tokens
    }

    /// Generate full context for LLM
    pub fn generate_context(&self) -> String {
        let mut context = String::new();

        // Add long-term memory
        let long_term_prompt = self.long_term.format_for_prompt();
        if !long_term_prompt.is_empty() {
            context.push_str(&long_term_prompt);
            context.push('\n');
        }

        // Add working memory status
        context.push_str(&self.working.generate_status_summary());
        context.push('\n');

        // Add session summary if available
        if let Some(summary) = &self.session.summary {
            context.push_str("<conversation_summary>\n");
            context.push_str(summary);
            context.push_str("\n</conversation_summary>\n");
        }

        context
    }
}

// ============================================================================
// Team Memory
// ============================================================================

/// Team memory for shared workflows and templates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMemory {
    /// Team ID
    pub team_id: String,
    /// Shared workflow template IDs
    pub workflow_ids: Vec<String>,
    /// Team preferences
    pub preferences: HashMap<String, serde_json::Value>,
    /// Last sync timestamp
    pub last_sync: DateTime<Utc>,
}

impl TeamMemory {
    /// Create new team memory
    pub fn new(team_id: impl Into<String>) -> Self {
        Self {
            team_id: team_id.into(),
            workflow_ids: Vec::new(),
            preferences: HashMap::new(),
            last_sync: Utc::now(),
        }
    }

    /// Add a shared workflow ID
    pub fn add_workflow(&mut self, workflow_id: impl Into<String>) {
        self.workflow_ids.push(workflow_id.into());
    }

    /// Set a team preference
    pub fn set_preference(&mut self, key: impl Into<String>, value: serde_json::Value) {
        self.preferences.insert(key.into(), value);
    }

    /// Get a team preference
    pub fn get_preference(&self, key: &str) -> Option<&serde_json::Value> {
        self.preferences.get(key)
    }
}

// ============================================================================
// Context Manager
// ============================================================================

/// Manager for context window optimization
pub struct ContextManager {
    /// Maximum tokens for context
    max_tokens: usize,
    /// Compression threshold (0.0-1.0)
    compression_threshold: f32,
    /// Number of recent turns to always keep
    preserve_recent_turns: usize,
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextManager {
    /// Create a new context manager with default settings
    pub fn new() -> Self {
        Self {
            max_tokens: 128_000,
            compression_threshold: 0.8,
            preserve_recent_turns: 10,
        }
    }

    /// Create with custom settings
    pub fn with_settings(
        max_tokens: usize,
        compression_threshold: f32,
        preserve_recent_turns: usize,
    ) -> Self {
        Self {
            max_tokens,
            compression_threshold,
            preserve_recent_turns,
        }
    }

    /// Check if context needs compression
    pub fn needs_compression(&self, context: &ContextMemory) -> bool {
        let current_tokens = context.estimated_tokens();
        let threshold_tokens = (self.max_tokens as f32 * self.compression_threshold) as usize;
        current_tokens >= threshold_tokens
    }

    /// Compress context to fit within limits
    pub fn compress(&self, context: &mut ContextMemory) -> Result<CompressionResult, MemoryError> {
        let before_tokens = context.estimated_tokens();

        if before_tokens <= self.max_tokens {
            return Ok(CompressionResult {
                tokens_before: before_tokens,
                tokens_after: before_tokens,
                messages_removed: 0,
                summary_generated: false,
            });
        }

        // Compact session memory
        let messages_before = context.session.messages.len();
        context.session.compact(self.preserve_recent_turns);
        let messages_removed = messages_before - context.session.messages.len();

        // Clear completed tasks from working memory
        context
            .working
            .task_tree
            .nodes
            .retain(|n| n.status != TaskStatus::Completed && n.status != TaskStatus::Failed);

        let after_tokens = context.estimated_tokens();

        Ok(CompressionResult {
            tokens_before: before_tokens,
            tokens_after: after_tokens,
            messages_removed,
            summary_generated: false, // Would be true if LLM summarization was used
        })
    }

    /// Get remaining token budget
    pub fn remaining_tokens(&self, context: &ContextMemory) -> usize {
        let current = context.estimated_tokens();
        self.max_tokens.saturating_sub(current)
    }
}

/// Result of context compression
#[derive(Debug, Clone)]
pub struct CompressionResult {
    /// Tokens before compression
    pub tokens_before: usize,
    /// Tokens after compression
    pub tokens_after: usize,
    /// Number of messages removed
    pub messages_removed: usize,
    /// Whether a summary was generated
    pub summary_generated: bool,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_working_memory_task_lifecycle() {
        let mut working = WorkingMemory::new();

        // Start a task
        let task_id = working.start_task("Test task");
        assert!(working.task_tree.current_id.is_some());

        // Record a tool call
        working.record_tool_call(&task_id, ToolCallRecord::new("Read", serde_json::json!({})));

        // Complete the task
        working.complete_task(
            &task_id,
            TaskResult::Success(serde_json::json!({"ok": true})),
        );

        let task = working.task_tree.get(&task_id).unwrap();
        assert_eq!(task.status, TaskStatus::Completed);
    }

    #[test]
    fn test_working_memory_variables() {
        let mut working = WorkingMemory::new();

        working.set_variable("file_path", serde_json::json!("/tmp/test.txt"));
        working.set_variable("count", serde_json::json!(42));

        assert_eq!(
            working.get_variable("file_path"),
            Some(&serde_json::json!("/tmp/test.txt"))
        );
        assert_eq!(working.get_variable("count"), Some(&serde_json::json!(42)));
    }

    #[test]
    fn test_session_memory_compaction() {
        let mut session = SessionMemory::new("session-1");

        // Add 20 messages
        for i in 0..20 {
            session.add_user_message(format!("Message {}", i));
        }

        assert_eq!(session.messages.len(), 20);

        // Compact to keep last 5
        session.compact(5);
        assert_eq!(session.messages.len(), 5);
        assert!(session.messages[0].content.contains("15"));
    }

    #[test]
    fn test_context_memory_integration() {
        let context = ContextMemory::new("session-1", "user-1");

        assert!(context.working.task_tree.nodes.is_empty());
        assert!(context.session.messages.is_empty());
        assert!(context.long_term.entries.is_empty());
    }

    #[test]
    fn test_task_tree_progress() {
        let mut tree = TaskTree::new();

        let mut task1 = TaskNode::new("Task 1");
        task1.status = TaskStatus::Completed;
        tree.add_node(task1);

        let task2 = TaskNode::new("Task 2");
        tree.add_node(task2);

        let (completed, total) = tree.progress();
        assert_eq!(completed, 1);
        assert_eq!(total, 2);
    }

    #[test]
    fn test_context_manager_compression() {
        let manager = ContextManager::with_settings(1000, 0.8, 5);
        let mut context = ContextMemory::new("session-1", "user-1");

        // Add messages to exceed threshold
        for i in 0..100 {
            context.session.add_user_message(format!(
                "This is a long message number {} with lots of content",
                i
            ));
        }

        assert!(manager.needs_compression(&context));

        let result = manager.compress(&mut context).unwrap();
        assert!(result.messages_removed > 0);
        assert!(result.tokens_after < result.tokens_before);
    }

    #[test]
    fn test_team_memory() {
        let mut team = TeamMemory::new("team-1");

        team.add_workflow("workflow-1");
        team.set_preference("default_model", serde_json::json!("claude-3"));

        assert_eq!(team.workflow_ids.len(), 1);
        assert_eq!(
            team.get_preference("default_model"),
            Some(&serde_json::json!("claude-3"))
        );
    }

    #[test]
    fn test_verification_lifecycle() {
        let mut working = WorkingMemory::new();

        let verification = Verification::new("Check file was created");
        let id = verification.id.clone();
        working.add_verification(verification);

        assert_eq!(working.pending_verifications.len(), 1);
        assert_eq!(
            working.pending_verifications[0].status,
            VerificationStatus::Pending
        );

        working.complete_verification(&id, true);
        assert_eq!(
            working.pending_verifications[0].status,
            VerificationStatus::Passed
        );
    }

    #[test]
    fn test_tool_state() {
        let mut state = ToolState::new("browser");

        assert!(!state.is_executing());

        state.set_executing("navigate");
        assert!(state.is_executing());

        state.set_idle();
        assert!(!state.is_executing());
    }
}
