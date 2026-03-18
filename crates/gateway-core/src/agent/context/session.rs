//! Session Context Manager
//!
//! Manages session-level context including conversation history, working files,
//! tool states, and loaded skills. Session context is ephemeral by default but
//! can be persisted on session end.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use super::resolver::{ContextLayer, ContextPriority, LoadedSkill, ResolvedContext};
use crate::agent::types::messages::AgentMessage;

// ============================================================================
// Session Context Types
// ============================================================================

/// Session-level context configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    /// Session unique identifier
    pub session_id: Uuid,

    /// Conversation ID if linked to a conversation
    pub conversation_id: Option<Uuid>,

    /// User ID owning this session
    pub user_id: Option<Uuid>,

    /// Session title/description
    pub title: Option<String>,

    /// Conversation history
    pub messages: Vec<AgentMessage>,

    /// Working files currently being edited
    pub working_files: HashMap<String, FileState>,

    /// Tool execution states
    pub tool_states: HashMap<String, ToolState>,

    /// Skills loaded in this session
    pub loaded_skills: Vec<LoadedSkill>,

    /// Session-specific custom context
    pub custom_context: HashMap<String, serde_json::Value>,
}

/// State of a file being worked on
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    /// File path
    pub path: String,

    /// Whether the file has been modified
    pub modified: bool,

    /// Last read content hash (for change detection)
    pub content_hash: Option<String>,

    /// Pending changes not yet written
    pub pending_changes: Option<String>,
}

/// State of a tool during session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolState {
    /// Tool name
    pub name: String,

    /// Number of times called in session
    pub call_count: u32,

    /// Last execution result summary
    pub last_result: Option<String>,

    /// Custom state data
    pub state_data: HashMap<String, serde_json::Value>,
}

impl Default for SessionContext {
    fn default() -> Self {
        Self {
            session_id: Uuid::new_v4(),
            conversation_id: None,
            user_id: None,
            title: None,
            messages: Vec::new(),
            working_files: HashMap::new(),
            tool_states: HashMap::new(),
            loaded_skills: Vec::new(),
            custom_context: HashMap::new(),
        }
    }
}

impl SessionContext {
    /// Create a new session context with the given ID
    pub fn new(session_id: Uuid) -> Self {
        Self {
            session_id,
            ..Default::default()
        }
    }

    /// Create a session context for a user
    pub fn for_user(session_id: Uuid, user_id: Uuid) -> Self {
        Self {
            session_id,
            user_id: Some(user_id),
            ..Default::default()
        }
    }

    /// Add a message to the session
    pub fn add_message(&mut self, message: AgentMessage) {
        self.messages.push(message);
    }

    /// Get message count
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Mark a file as being worked on
    pub fn track_file(&mut self, path: &str) {
        self.working_files.insert(
            path.to_string(),
            FileState {
                path: path.to_string(),
                modified: false,
                content_hash: None,
                pending_changes: None,
            },
        );
    }

    /// Mark a file as modified
    pub fn mark_file_modified(&mut self, path: &str) {
        if let Some(state) = self.working_files.get_mut(path) {
            state.modified = true;
        }
    }

    /// Record a tool call
    pub fn record_tool_call(&mut self, tool_name: &str, result_summary: Option<String>) {
        let state = self
            .tool_states
            .entry(tool_name.to_string())
            .or_insert(ToolState {
                name: tool_name.to_string(),
                call_count: 0,
                last_result: None,
                state_data: HashMap::new(),
            });
        state.call_count += 1;
        state.last_result = result_summary;
    }

    /// Load a skill into the session
    pub fn load_skill(&mut self, skill: LoadedSkill) {
        // Check if already loaded
        if !self.loaded_skills.iter().any(|s| s.name == skill.name) {
            self.loaded_skills.push(skill);
        }
    }

    /// Check if a skill is loaded
    pub fn is_skill_loaded(&self, name: &str) -> bool {
        self.loaded_skills.iter().any(|s| s.name == name)
    }

    /// Get loaded skill names
    pub fn loaded_skill_names(&self) -> Vec<&str> {
        self.loaded_skills.iter().map(|s| s.name.as_str()).collect()
    }

    /// Set custom context value
    pub fn set_context(&mut self, key: &str, value: serde_json::Value) {
        self.custom_context.insert(key.to_string(), value);
    }

    /// Get custom context value
    pub fn get_context(&self, key: &str) -> Option<&serde_json::Value> {
        self.custom_context.get(key)
    }

    /// Generate a summary of the session for context
    fn generate_summary(&self) -> String {
        let mut summary = String::new();

        // Files being worked on
        if !self.working_files.is_empty() {
            summary.push_str("### Working Files\n\n");
            for (path, state) in &self.working_files {
                let status = if state.modified { "(modified)" } else { "" };
                summary.push_str(&format!("- {} {}\n", path, status));
            }
            summary.push('\n');
        }

        // Loaded skills
        if !self.loaded_skills.is_empty() {
            summary.push_str("### Loaded Skills\n\n");
            for skill in &self.loaded_skills {
                summary.push_str(&format!("- {}\n", skill.name));
            }
            summary.push('\n');
        }

        // Recent tool usage
        let recent_tools: Vec<_> = self
            .tool_states
            .values()
            .filter(|t| t.call_count > 0)
            .take(5)
            .collect();

        if !recent_tools.is_empty() {
            summary.push_str("### Recent Tools\n\n");
            for tool in recent_tools {
                summary.push_str(&format!("- {} ({}x)\n", tool.name, tool.call_count));
            }
        }

        summary
    }
}

// ============================================================================
// ContextLayer Implementation
// ============================================================================

impl ContextLayer for SessionContext {
    fn layer_name(&self) -> &str {
        "session"
    }

    fn priority(&self) -> ContextPriority {
        ContextPriority::Session
    }

    fn apply_to(&self, resolved: &mut ResolvedContext) {
        // Add loaded skills
        for skill in &self.loaded_skills {
            if !resolved.loaded_skills.iter().any(|s| s.name == skill.name) {
                resolved.loaded_skills.push(skill.clone());
            }
        }

        // Generate session summary if there's significant context
        if !self.working_files.is_empty() || !self.loaded_skills.is_empty() {
            let summary = self.generate_summary();
            if !summary.is_empty() {
                // Append to task instructions or create new
                if let Some(existing) = &resolved.task_instructions {
                    resolved.task_instructions =
                        Some(format!("{}\n\n## Session Context\n\n{}", existing, summary));
                } else {
                    resolved.task_instructions = Some(format!("## Session Context\n\n{}", summary));
                }
            }
        }
    }
}

// ============================================================================
// Session Context Loader
// ============================================================================

/// Loader for session context from database or in-memory storage
pub struct SessionContextLoader {
    #[cfg(feature = "database")]
    pool: Option<sqlx::Pool<sqlx::Postgres>>,

    #[cfg(not(feature = "database"))]
    _marker: std::marker::PhantomData<()>,
}

impl SessionContextLoader {
    /// Create a new session context loader
    #[cfg(feature = "database")]
    pub fn new(pool: Option<sqlx::Pool<sqlx::Postgres>>) -> Self {
        Self { pool }
    }

    #[cfg(not(feature = "database"))]
    pub fn new() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }

    /// Load session context by session ID
    #[cfg(feature = "database")]
    pub async fn load(&self, session_id: &Uuid) -> crate::error::Result<Option<SessionContext>> {
        let Some(pool) = &self.pool else {
            return Ok(None);
        };

        // Query session from database
        let row = sqlx::query(
            r#"
            SELECT id, conversation_id, user_id, title, context, messages
            FROM sessions
            WHERE id = $1
            "#,
        )
        .bind(session_id)
        .fetch_optional(pool)
        .await?;

        match row {
            Some(r) => {
                use sqlx::Row;
                let messages: Vec<AgentMessage> = r
                    .try_get::<Option<serde_json::Value>, _>("messages")
                    .ok()
                    .flatten()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();

                let custom_context: HashMap<String, serde_json::Value> = r
                    .try_get::<Option<serde_json::Value>, _>("context")
                    .ok()
                    .flatten()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();

                Ok(Some(SessionContext {
                    session_id: r.try_get("id")?,
                    conversation_id: r.try_get("conversation_id")?,
                    user_id: r.try_get("user_id")?,
                    title: r.try_get("title")?,
                    messages,
                    working_files: HashMap::new(),
                    tool_states: HashMap::new(),
                    loaded_skills: Vec::new(),
                    custom_context,
                }))
            }
            None => Ok(None),
        }
    }

    #[cfg(not(feature = "database"))]
    pub async fn load(&self, _session_id: &Uuid) -> crate::error::Result<Option<SessionContext>> {
        Ok(None)
    }

    /// Create a new session in the database
    #[cfg(feature = "database")]
    pub async fn create(
        &self,
        user_id: Option<Uuid>,
        title: Option<String>,
    ) -> crate::error::Result<SessionContext> {
        let session_id = Uuid::new_v4();
        let session = SessionContext {
            session_id,
            user_id,
            title,
            ..Default::default()
        };

        if let Some(pool) = &self.pool {
            sqlx::query(
                r#"
                INSERT INTO sessions (id, user_id, title, context, messages)
                VALUES ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(session.session_id)
            .bind(session.user_id)
            .bind(&session.title)
            .bind(serde_json::json!({}))
            .bind(serde_json::json!([]))
            .execute(pool)
            .await?;
        }

        Ok(session)
    }

    #[cfg(not(feature = "database"))]
    pub async fn create(
        &self,
        user_id: Option<Uuid>,
        title: Option<String>,
    ) -> crate::error::Result<SessionContext> {
        Ok(SessionContext {
            session_id: Uuid::new_v4(),
            user_id,
            title,
            ..Default::default()
        })
    }
}

#[cfg(not(feature = "database"))]
impl Default for SessionContextLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SessionContextLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionContextLoader").finish()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_context_default() {
        let ctx = SessionContext::default();
        assert!(!ctx.session_id.is_nil());
        assert!(ctx.messages.is_empty());
        assert!(ctx.working_files.is_empty());
        assert!(ctx.loaded_skills.is_empty());
    }

    #[test]
    fn test_session_context_new() {
        let id = Uuid::new_v4();
        let ctx = SessionContext::new(id);
        assert_eq!(ctx.session_id, id);
    }

    #[test]
    fn test_session_context_for_user() {
        let session_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let ctx = SessionContext::for_user(session_id, user_id);

        assert_eq!(ctx.session_id, session_id);
        assert_eq!(ctx.user_id, Some(user_id));
    }

    #[test]
    fn test_track_file() {
        let mut ctx = SessionContext::default();
        ctx.track_file("/path/to/file.rs");

        assert!(ctx.working_files.contains_key("/path/to/file.rs"));
        assert!(!ctx.working_files.get("/path/to/file.rs").unwrap().modified);
    }

    #[test]
    fn test_mark_file_modified() {
        let mut ctx = SessionContext::default();
        ctx.track_file("/path/to/file.rs");
        ctx.mark_file_modified("/path/to/file.rs");

        assert!(ctx.working_files.get("/path/to/file.rs").unwrap().modified);
    }

    #[test]
    fn test_record_tool_call() {
        let mut ctx = SessionContext::default();
        ctx.record_tool_call("read", Some("success".to_string()));
        ctx.record_tool_call("read", None);

        let state = ctx.tool_states.get("read").unwrap();
        assert_eq!(state.call_count, 2);
        assert!(state.last_result.is_none());
    }

    #[test]
    fn test_load_skill() {
        let mut ctx = SessionContext::default();
        let skill = LoadedSkill {
            name: "test-skill".to_string(),
            content: "Test content".to_string(),
            requires_browser: false,
            automation_tab: false,
        };

        ctx.load_skill(skill.clone());
        assert!(ctx.is_skill_loaded("test-skill"));
        assert!(!ctx.is_skill_loaded("other-skill"));

        // Duplicate load should not add
        ctx.load_skill(skill);
        assert_eq!(ctx.loaded_skills.len(), 1);
    }

    #[test]
    fn test_custom_context() {
        let mut ctx = SessionContext::default();
        ctx.set_context("key1", serde_json::json!("value1"));

        assert_eq!(ctx.get_context("key1"), Some(&serde_json::json!("value1")));
        assert!(ctx.get_context("key2").is_none());
    }

    #[test]
    fn test_context_layer_metadata() {
        let ctx = SessionContext::default();
        assert_eq!(ctx.layer_name(), "session");
        assert_eq!(ctx.priority(), ContextPriority::Session);
    }

    #[test]
    fn test_apply_to() {
        let mut ctx = SessionContext::default();
        ctx.track_file("/path/to/file.rs");
        ctx.load_skill(LoadedSkill {
            name: "test-skill".to_string(),
            content: "content".to_string(),
            requires_browser: false,
            automation_tab: false,
        });

        let mut resolved = ResolvedContext::default();
        ctx.apply_to(&mut resolved);

        assert_eq!(resolved.loaded_skills.len(), 1);
        assert!(resolved.task_instructions.is_some());
        let instructions = resolved.task_instructions.unwrap();
        assert!(instructions.contains("Working Files"));
        assert!(instructions.contains("/path/to/file.rs"));
        assert!(instructions.contains("test-skill"));
    }

    #[test]
    fn test_generate_summary_empty() {
        let ctx = SessionContext::default();
        let summary = ctx.generate_summary();
        assert!(summary.is_empty());
    }

    #[test]
    fn test_generate_summary_with_files() {
        let mut ctx = SessionContext::default();
        ctx.track_file("/path/to/file.rs");
        ctx.mark_file_modified("/path/to/file.rs");

        let summary = ctx.generate_summary();
        assert!(summary.contains("Working Files"));
        assert!(summary.contains("(modified)"));
    }

    #[test]
    fn test_loaded_skill_names() {
        let mut ctx = SessionContext::default();
        ctx.load_skill(LoadedSkill {
            name: "skill1".to_string(),
            content: "".to_string(),
            requires_browser: false,
            automation_tab: false,
        });
        ctx.load_skill(LoadedSkill {
            name: "skill2".to_string(),
            content: "".to_string(),
            requires_browser: false,
            automation_tab: false,
        });

        let names = ctx.loaded_skill_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"skill1"));
        assert!(names.contains(&"skill2"));
    }

    #[test]
    fn test_serde_round_trip() {
        let mut ctx = SessionContext::default();
        ctx.title = Some("Test Session".to_string());
        ctx.track_file("/test.rs");
        ctx.set_context("key", serde_json::json!({"nested": "value"}));

        let json = serde_json::to_string(&ctx).expect("serialize");
        let restored: SessionContext = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(ctx.session_id, restored.session_id);
        assert_eq!(ctx.title, restored.title);
        assert!(restored.working_files.contains_key("/test.rs"));
    }

    #[cfg(not(feature = "database"))]
    #[tokio::test]
    async fn test_loader_without_database() {
        let loader = SessionContextLoader::new();
        let result = loader.load(&Uuid::new_v4()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[cfg(not(feature = "database"))]
    #[tokio::test]
    async fn test_create_session() {
        let loader = SessionContextLoader::new();
        let session = loader
            .create(Some(Uuid::new_v4()), Some("Test".to_string()))
            .await;
        assert!(session.is_ok());
        let session = session.unwrap();
        assert_eq!(session.title, Some("Test".to_string()));
    }
}
