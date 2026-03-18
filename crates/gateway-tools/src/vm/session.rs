//! VM Session Management
//!
//! Provides session management for maintaining execution state across
//! multiple interactions with Firecracker VMs. Supports REPL-like
//! multi-step executions with variable persistence.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use crate::vm::executor::{
    BrowserAction, BrowserResult, ExecutionContext, ExecutionResult, VmExecutor,
};
use crate::vm::manager::VmInstance;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{info, instrument, warn};
use uuid::Uuid;

/// Session configuration
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Maximum session idle time before cleanup (seconds)
    pub max_idle_secs: u64,
    /// Maximum session lifetime (seconds)
    pub max_lifetime_secs: u64,
    /// Maximum number of executions per session
    pub max_executions: usize,
    /// Default execution timeout (milliseconds)
    pub default_timeout_ms: u64,
    /// Whether to persist variables between executions
    pub persist_variables: bool,
    /// Maximum size of persisted variables (bytes)
    pub max_variable_size: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            max_idle_secs: 1800,     // 30 minutes
            max_lifetime_secs: 7200, // 2 hours
            max_executions: 1000,
            default_timeout_ms: 30000, // 30 seconds
            persist_variables: true,
            max_variable_size: 10 * 1024 * 1024, // 10 MB
        }
    }
}

/// Session state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    /// Session is active and can accept executions
    Active,
    /// Session is executing code
    Busy,
    /// Session has been paused
    Paused,
    /// Session is being cleaned up
    Cleaning,
    /// Session has ended
    Ended,
    /// Session encountered an error
    Error,
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionState::Active => write!(f, "active"),
            SessionState::Busy => write!(f, "busy"),
            SessionState::Paused => write!(f, "paused"),
            SessionState::Cleaning => write!(f, "cleaning"),
            SessionState::Ended => write!(f, "ended"),
            SessionState::Error => write!(f, "error"),
        }
    }
}

/// Browser state tracking
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BrowserState {
    /// Current URL
    pub current_url: Option<String>,
    /// Page title
    pub title: Option<String>,
    /// Viewport dimensions
    pub viewport: Option<ViewportDimensions>,
    /// Whether browser is initialized
    pub initialized: bool,
    /// Navigation history length
    pub history_length: usize,
}

/// Viewport dimensions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewportDimensions {
    pub width: u32,
    pub height: u32,
}

/// Execution history entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionEntry {
    /// Execution ID
    pub execution_id: String,
    /// Code that was executed
    pub code: String,
    /// Execution result
    pub result: ExecutionResult,
    /// Timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Session information (serializable subset)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Unique session ID
    pub session_id: String,
    /// Associated VM instance ID
    pub vm_id: String,
    /// Current session state
    pub state: SessionState,
    /// When the session was created
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last activity timestamp
    pub last_activity: chrono::DateTime<chrono::Utc>,
    /// Number of executions performed
    pub execution_count: usize,
    /// Current browser state
    pub browser_state: BrowserState,
    /// Persisted variables (keys only, for privacy)
    pub variable_names: Vec<String>,
}

/// VM Session for maintaining execution state
pub struct VmSession {
    /// Unique session ID
    session_id: String,
    /// Associated VM instance
    vm_instance: VmInstance,
    /// Executor for this session
    executor: VmExecutor,
    /// Session configuration
    config: SessionConfig,
    /// Current session state
    state: Arc<RwLock<SessionState>>,
    /// Creation timestamp
    created_at: Instant,
    /// Last activity timestamp
    last_activity: Arc<RwLock<Instant>>,
    /// Execution count
    execution_count: Arc<RwLock<usize>>,
    /// Persisted variables from Python executions
    variables: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    /// Browser state
    browser_state: Arc<RwLock<BrowserState>>,
    /// Execution history
    history: Arc<RwLock<Vec<ExecutionEntry>>>,
    /// Running processes (execution IDs)
    running_processes: Arc<RwLock<Vec<String>>>,
}

impl VmSession {
    /// Create a new session for a VM instance
    pub fn new(vm_instance: VmInstance, config: SessionConfig) -> Self {
        let session_id = Uuid::new_v4().to_string();
        let timeout = Duration::from_millis(config.default_timeout_ms);
        let executor = VmExecutor::new(&vm_instance, timeout);

        info!(
            session_id = %session_id,
            vm_id = %vm_instance.id,
            "Creating new VM session"
        );

        Self {
            session_id,
            vm_instance,
            executor,
            config,
            state: Arc::new(RwLock::new(SessionState::Active)),
            created_at: Instant::now(),
            last_activity: Arc::new(RwLock::new(Instant::now())),
            execution_count: Arc::new(RwLock::new(0)),
            variables: Arc::new(RwLock::new(HashMap::new())),
            browser_state: Arc::new(RwLock::new(BrowserState::default())),
            history: Arc::new(RwLock::new(Vec::new())),
            running_processes: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Get session ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Get VM instance
    pub fn vm_instance(&self) -> &VmInstance {
        &self.vm_instance
    }

    /// Get the executor
    pub fn executor(&self) -> &VmExecutor {
        &self.executor
    }

    /// Get current session state
    pub async fn state(&self) -> SessionState {
        self.state.read().await.clone()
    }

    /// Get session info (serializable)
    pub async fn info(&self) -> SessionInfo {
        let state = self.state.read().await.clone();
        let last_activity = *self.last_activity.read().await;
        let execution_count = *self.execution_count.read().await;
        let browser_state = self.browser_state.read().await.clone();
        let variables = self.variables.read().await;

        SessionInfo {
            session_id: self.session_id.clone(),
            vm_id: self.vm_instance.id.clone(),
            state,
            created_at: chrono::Utc::now()
                - chrono::Duration::from_std(self.created_at.elapsed()).unwrap_or_default(),
            last_activity: chrono::Utc::now()
                - chrono::Duration::from_std(last_activity.elapsed()).unwrap_or_default(),
            execution_count,
            browser_state,
            variable_names: variables.keys().cloned().collect(),
        }
    }

    /// Execute Python code in this session
    #[instrument(skip(self, code), fields(session_id = %self.session_id))]
    pub async fn execute_python(&self, code: &str) -> Result<ExecutionResult> {
        // Check session state
        {
            let state = self.state.read().await;
            match *state {
                SessionState::Active => {}
                SessionState::Busy => {
                    return Err(Error::ExecutionFailed(
                        "Session is busy with another execution".to_string(),
                    ));
                }
                SessionState::Ended | SessionState::Cleaning => {
                    return Err(Error::ExecutionFailed("Session has ended".to_string()));
                }
                SessionState::Error => {
                    return Err(Error::ExecutionFailed(
                        "Session is in error state".to_string(),
                    ));
                }
                SessionState::Paused => {
                    return Err(Error::ExecutionFailed("Session is paused".to_string()));
                }
            }
        }

        // Check execution limit
        {
            let count = *self.execution_count.read().await;
            if count >= self.config.max_executions {
                return Err(Error::ExecutionFailed(format!(
                    "Session execution limit ({}) reached",
                    self.config.max_executions
                )));
            }
        }

        // Set state to busy
        {
            let mut state = self.state.write().await;
            *state = SessionState::Busy;
        }

        // Build execution context with persisted variables
        let mut context = ExecutionContext {
            timeout_ms: self.config.default_timeout_ms,
            capture_output: true,
            ..Default::default()
        };

        if self.config.persist_variables {
            let vars = self.variables.read().await;
            context.session_vars = vars.clone();
        }

        // Execute
        let result = self.executor.execute_python(code, context).await;

        // Update state and activity
        {
            let mut state = self.state.write().await;
            *state = if result.is_ok() {
                SessionState::Active
            } else {
                SessionState::Active // Stay active even on execution error
            };
        }

        {
            let mut last_activity = self.last_activity.write().await;
            *last_activity = Instant::now();
        }

        // Update execution count
        {
            let mut count = self.execution_count.write().await;
            *count += 1;
        }

        // If successful, persist captured variables
        if let Ok(ref exec_result) = result {
            if self.config.persist_variables && exec_result.success {
                let mut vars = self.variables.write().await;
                for (key, value) in &exec_result.captured_vars {
                    // Check variable size
                    let size = serde_json::to_string(value).map(|s| s.len()).unwrap_or(0);
                    if size <= self.config.max_variable_size {
                        vars.insert(key.clone(), value.clone());
                    } else {
                        warn!(
                            session_id = %self.session_id,
                            variable = %key,
                            size = size,
                            max = self.config.max_variable_size,
                            "Variable too large to persist"
                        );
                    }
                }
            }

            // Add to history
            let mut history = self.history.write().await;
            history.push(ExecutionEntry {
                execution_id: exec_result.execution_id.clone(),
                code: code.to_string(),
                result: exec_result.clone(),
                timestamp: chrono::Utc::now(),
            });

            // Limit history size
            if history.len() > 100 {
                history.drain(0..50);
            }
        }

        result
    }

    /// Execute a browser action in this session
    #[instrument(skip(self, action), fields(session_id = %self.session_id))]
    pub async fn execute_browser(&self, action: BrowserAction) -> Result<BrowserResult> {
        // Check session state
        {
            let state = self.state.read().await;
            if !matches!(*state, SessionState::Active | SessionState::Busy) {
                return Err(Error::ExecutionFailed(format!(
                    "Session is not active: {}",
                    *state
                )));
            }
        }

        // Execute browser action
        let result = self.executor.execute_browser(action.clone()).await;

        // Update browser state based on action
        if let Ok(ref browser_result) = result {
            if browser_result.success {
                let mut browser_state = self.browser_state.write().await;

                match &action {
                    BrowserAction::Navigate { url, .. } => {
                        browser_state.current_url = Some(url.clone());
                        browser_state.initialized = true;
                        if let Some(data) = &browser_result.data {
                            if let Some(title) = data.get("title").and_then(|v| v.as_str()) {
                                browser_state.title = Some(title.to_string());
                            }
                        }
                    }
                    BrowserAction::SetViewport { width, height } => {
                        browser_state.viewport = Some(ViewportDimensions {
                            width: *width,
                            height: *height,
                        });
                    }
                    BrowserAction::Back | BrowserAction::Forward => {
                        if let Some(data) = &browser_result.data {
                            if let Some(url) = data.get("url").and_then(|v| v.as_str()) {
                                browser_state.current_url = Some(url.to_string());
                            }
                            if let Some(title) = data.get("title").and_then(|v| v.as_str()) {
                                browser_state.title = Some(title.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Update activity timestamp
        {
            let mut last_activity = self.last_activity.write().await;
            *last_activity = Instant::now();
        }

        result
    }

    /// Get a persisted variable
    pub async fn get_variable(&self, name: &str) -> Option<serde_json::Value> {
        let vars = self.variables.read().await;
        vars.get(name).cloned()
    }

    /// Set a persisted variable
    pub async fn set_variable(&self, name: &str, value: serde_json::Value) -> Result<()> {
        let size = serde_json::to_string(&value).map(|s| s.len()).unwrap_or(0);

        if size > self.config.max_variable_size {
            return Err(Error::InvalidInput(format!(
                "Variable too large: {} bytes (max: {})",
                size, self.config.max_variable_size
            )));
        }

        let mut vars = self.variables.write().await;
        vars.insert(name.to_string(), value);
        Ok(())
    }

    /// Delete a persisted variable
    pub async fn delete_variable(&self, name: &str) -> bool {
        let mut vars = self.variables.write().await;
        vars.remove(name).is_some()
    }

    /// Clear all persisted variables
    pub async fn clear_variables(&self) {
        let mut vars = self.variables.write().await;
        vars.clear();
    }

    /// Get all variable names
    pub async fn variable_names(&self) -> Vec<String> {
        let vars = self.variables.read().await;
        vars.keys().cloned().collect()
    }

    /// Get browser state
    pub async fn browser_state(&self) -> BrowserState {
        self.browser_state.read().await.clone()
    }

    /// Get execution history
    pub async fn history(&self, limit: Option<usize>) -> Vec<ExecutionEntry> {
        let history = self.history.read().await;
        let limit = limit.unwrap_or(history.len());
        history.iter().rev().take(limit).cloned().collect()
    }

    /// Clear execution history
    pub async fn clear_history(&self) {
        let mut history = self.history.write().await;
        history.clear();
    }

    /// Check if session is expired
    pub async fn is_expired(&self) -> bool {
        // Check lifetime
        if self.created_at.elapsed().as_secs() >= self.config.max_lifetime_secs {
            return true;
        }

        // Check idle time
        let last_activity = *self.last_activity.read().await;
        if last_activity.elapsed().as_secs() >= self.config.max_idle_secs {
            return true;
        }

        false
    }

    /// Get session age in seconds
    pub fn age_secs(&self) -> u64 {
        self.created_at.elapsed().as_secs()
    }

    /// Get idle time in seconds
    pub async fn idle_secs(&self) -> u64 {
        let last_activity = *self.last_activity.read().await;
        last_activity.elapsed().as_secs()
    }

    /// Pause the session
    pub async fn pause(&self) -> Result<()> {
        let mut state = self.state.write().await;
        match *state {
            SessionState::Active => {
                *state = SessionState::Paused;
                Ok(())
            }
            SessionState::Busy => Err(Error::ExecutionFailed(
                "Cannot pause while execution is running".to_string(),
            )),
            _ => Err(Error::ExecutionFailed(format!(
                "Cannot pause session in state: {}",
                *state
            ))),
        }
    }

    /// Resume a paused session
    pub async fn resume(&self) -> Result<()> {
        let mut state = self.state.write().await;
        if *state == SessionState::Paused {
            *state = SessionState::Active;
            Ok(())
        } else {
            Err(Error::ExecutionFailed(format!(
                "Cannot resume session in state: {}",
                *state
            )))
        }
    }

    /// End the session
    #[instrument(skip(self), fields(session_id = %self.session_id))]
    pub async fn end(&self) -> Result<()> {
        info!("Ending session");

        // Cancel any running executions
        {
            let processes = self.running_processes.read().await;
            for exec_id in processes.iter() {
                let _ = self.executor.cancel(exec_id).await;
            }
        }

        // Set state to cleaning, then ended
        {
            let mut state = self.state.write().await;
            *state = SessionState::Cleaning;
        }

        // Clear variables and history
        self.clear_variables().await;
        self.clear_history().await;

        // Set final state
        {
            let mut state = self.state.write().await;
            *state = SessionState::Ended;
        }

        Ok(())
    }

    /// Touch the session (update last activity)
    pub async fn touch(&self) {
        let mut last_activity = self.last_activity.write().await;
        *last_activity = Instant::now();
    }
}

/// Session manager for handling multiple sessions
pub struct SessionManager {
    /// Active sessions by session ID
    sessions: Arc<RwLock<HashMap<String, Arc<VmSession>>>>,
    /// Default session configuration
    config: SessionConfig,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new(config: SessionConfig) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Create a new session for a VM instance
    pub async fn create_session(&self, vm_instance: VmInstance) -> Arc<VmSession> {
        let session = Arc::new(VmSession::new(vm_instance, self.config.clone()));
        let session_id = session.session_id().to_string();

        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id, Arc::clone(&session));

        session
    }

    /// Get a session by ID
    pub async fn get_session(&self, session_id: &str) -> Option<Arc<VmSession>> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }

    /// Remove a session
    pub async fn remove_session(&self, session_id: &str) -> Option<Arc<VmSession>> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id)
    }

    /// Get all active session IDs
    pub async fn active_session_ids(&self) -> Vec<String> {
        let sessions = self.sessions.read().await;
        sessions.keys().cloned().collect()
    }

    /// Get session count
    pub async fn session_count(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }

    /// Clean up expired sessions
    #[instrument(skip(self))]
    pub async fn cleanup_expired_sessions(&self) -> usize {
        let mut to_remove = Vec::new();

        {
            let sessions = self.sessions.read().await;
            for (id, session) in sessions.iter() {
                if session.is_expired().await {
                    to_remove.push(id.clone());
                }
            }
        }

        let count = to_remove.len();
        if count > 0 {
            info!(count = count, "Cleaning up expired sessions");

            let mut sessions = self.sessions.write().await;
            for id in to_remove {
                if let Some(session) = sessions.remove(&id) {
                    let _ = session.end().await;
                }
            }
        }

        count
    }

    /// Start background cleanup task
    pub fn start_cleanup_task(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let manager = Arc::clone(self);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));

            loop {
                interval.tick().await;
                manager.cleanup_expired_sessions().await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn create_test_instance() -> VmInstance {
        VmInstance {
            id: "test-vm".to_string(),
            ip: Ipv4Addr::new(172, 16, 0, 2),
            port: 8080,
            vnc_port: 5900,
            status: crate::vm::manager::VmStatus::Running,
            created_at: Instant::now(),
            index: 0,
        }
    }

    #[test]
    fn test_session_config_defaults() {
        let config = SessionConfig::default();

        assert_eq!(config.max_idle_secs, 1800);
        assert_eq!(config.max_lifetime_secs, 7200);
        assert_eq!(config.max_executions, 1000);
        assert_eq!(config.default_timeout_ms, 30000);
        assert!(config.persist_variables);
    }

    #[test]
    fn test_session_state_display() {
        assert_eq!(SessionState::Active.to_string(), "active");
        assert_eq!(SessionState::Busy.to_string(), "busy");
        assert_eq!(SessionState::Ended.to_string(), "ended");
    }

    #[tokio::test]
    async fn test_session_creation() {
        let instance = create_test_instance();
        let session = VmSession::new(instance.clone(), SessionConfig::default());

        assert!(!session.session_id().is_empty());
        assert_eq!(session.vm_instance().id, "test-vm");
        assert_eq!(session.state().await, SessionState::Active);
    }

    #[tokio::test]
    async fn test_session_info() {
        let instance = create_test_instance();
        let session = VmSession::new(instance, SessionConfig::default());

        let info = session.info().await;

        assert_eq!(info.session_id, session.session_id());
        assert_eq!(info.vm_id, "test-vm");
        assert_eq!(info.state, SessionState::Active);
        assert_eq!(info.execution_count, 0);
    }

    #[tokio::test]
    async fn test_session_variables() {
        let instance = create_test_instance();
        let session = VmSession::new(instance, SessionConfig::default());

        // Set variable
        session
            .set_variable("test", serde_json::json!(42))
            .await
            .unwrap();

        // Get variable
        let value = session.get_variable("test").await;
        assert_eq!(value, Some(serde_json::json!(42)));

        // Delete variable
        assert!(session.delete_variable("test").await);
        assert!(session.get_variable("test").await.is_none());
    }

    #[tokio::test]
    async fn test_session_variable_size_limit() {
        let instance = create_test_instance();
        let config = SessionConfig {
            max_variable_size: 10, // Very small limit for testing
            ..Default::default()
        };
        let session = VmSession::new(instance, config);

        // Try to set a large variable
        let large_value = serde_json::json!("this is a very long string");
        let result = session.set_variable("large", large_value).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_pause_resume() {
        let instance = create_test_instance();
        let session = VmSession::new(instance, SessionConfig::default());

        // Pause
        session.pause().await.unwrap();
        assert_eq!(session.state().await, SessionState::Paused);

        // Resume
        session.resume().await.unwrap();
        assert_eq!(session.state().await, SessionState::Active);
    }

    #[tokio::test]
    async fn test_session_end() {
        let instance = create_test_instance();
        let session = VmSession::new(instance, SessionConfig::default());

        // Add a variable
        session
            .set_variable("test", serde_json::json!(1))
            .await
            .unwrap();

        // End session
        session.end().await.unwrap();

        assert_eq!(session.state().await, SessionState::Ended);
        assert!(session.variable_names().await.is_empty());
    }

    #[tokio::test]
    async fn test_session_manager() {
        let manager = SessionManager::new(SessionConfig::default());

        // Create session
        let instance = create_test_instance();
        let session = manager.create_session(instance).await;
        let session_id = session.session_id().to_string();

        // Get session
        let retrieved = manager.get_session(&session_id).await;
        assert!(retrieved.is_some());

        // Count
        assert_eq!(manager.session_count().await, 1);

        // Remove session
        let removed = manager.remove_session(&session_id).await;
        assert!(removed.is_some());
        assert_eq!(manager.session_count().await, 0);
    }

    #[tokio::test]
    async fn test_session_expiry() {
        let instance = create_test_instance();
        let config = SessionConfig {
            max_idle_secs: 0,     // 0 seconds means already expired
            max_lifetime_secs: 0, // 0 seconds means already expired
            ..Default::default()
        };
        let session = VmSession::new(instance, config);

        // With max_idle_secs and max_lifetime_secs both at 0,
        // the session should be expired immediately (>= 0 is always true)
        assert!(session.is_expired().await);
    }

    #[tokio::test]
    async fn test_session_touch() {
        let instance = create_test_instance();
        let config = SessionConfig {
            max_idle_secs: 1,
            ..Default::default()
        };
        let session = VmSession::new(instance, config);

        // Touch should reset idle time
        tokio::time::sleep(Duration::from_millis(100)).await;
        session.touch().await;

        assert!(!session.is_expired().await);
    }

    #[tokio::test]
    async fn test_browser_state() {
        let instance = create_test_instance();
        let session = VmSession::new(instance, SessionConfig::default());

        let state = session.browser_state().await;
        assert!(!state.initialized);
        assert!(state.current_url.is_none());
    }

    #[tokio::test]
    async fn test_clear_variables() {
        let instance = create_test_instance();
        let session = VmSession::new(instance, SessionConfig::default());

        session
            .set_variable("a", serde_json::json!(1))
            .await
            .unwrap();
        session
            .set_variable("b", serde_json::json!(2))
            .await
            .unwrap();

        assert_eq!(session.variable_names().await.len(), 2);

        session.clear_variables().await;

        assert!(session.variable_names().await.is_empty());
    }

    #[test]
    fn test_session_info_serialization() {
        let info = SessionInfo {
            session_id: "test-session".to_string(),
            vm_id: "test-vm".to_string(),
            state: SessionState::Active,
            created_at: chrono::Utc::now(),
            last_activity: chrono::Utc::now(),
            execution_count: 5,
            browser_state: BrowserState::default(),
            variable_names: vec!["x".to_string(), "y".to_string()],
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("test-session"));
        assert!(json.contains("active"));
    }
}
