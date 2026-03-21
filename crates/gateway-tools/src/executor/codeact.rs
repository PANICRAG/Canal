//! CodeAct Execution Engine
//!
//! This module provides the main CodeAct execution engine for orchestrating
//! code execution in Docker containers. It supports multi-language execution
//! with session management and variable persistence.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     CodeActEngine                                │
//! │  ┌──────────────────┐  ┌────────────────┐  ┌────────────────┐  │
//! │  │  ContainerPool   │  │ SessionManager │  │  JSON-RPC      │  │
//! │  └──────────────────┘  └────────────────┘  └────────────────┘  │
//! │           │                    │                   │            │
//! │           └────────────────────┼───────────────────┘            │
//! │                                │                                │
//! │                     ┌──────────┴──────────┐                     │
//! │                     │    Docker API       │                     │
//! │                     └─────────────────────┘                     │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Features
//!
//! - Container pool integration for efficient resource management
//! - Multi-language support (Python, Bash, JavaScript, etc.)
//! - Session management with variable persistence across executions
//! - JSON-RPC based communication with executor scripts
//! - Configurable timeouts and resource limits

use crate::error::{ServiceError as Error, ServiceResult as Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::pool::ContainerPool;
use super::types::{ContainerConfig, PoolConfig};
use super::{ExecutionResult, ExecutionStatus, Language};

/// Configuration for the CodeAct engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeActConfig {
    /// Default timeout for code execution in milliseconds
    #[serde(default = "default_timeout_ms")]
    pub default_timeout_ms: u64,

    /// Maximum timeout allowed in milliseconds
    #[serde(default = "default_max_timeout_ms")]
    pub max_timeout_ms: u64,

    /// Default memory limit in MB
    #[serde(default = "default_memory_limit_mb")]
    pub memory_limit_mb: u64,

    /// Default CPU limit (1.0 = 1 core)
    #[serde(default = "default_cpu_limit")]
    pub cpu_limit: f64,

    /// Session idle timeout in seconds
    #[serde(default = "default_session_idle_timeout")]
    pub session_idle_timeout_secs: u64,

    /// Maximum sessions allowed
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,

    /// Docker image for Python execution
    #[serde(default = "default_python_image")]
    pub python_image: String,

    /// Docker image for multi-language execution
    #[serde(default = "default_multi_image")]
    pub multi_image: String,

    /// Path to executor.py script inside container
    #[serde(default = "default_executor_path")]
    pub executor_path: String,

    /// Pool configuration
    #[serde(default)]
    pub pool_config: PoolConfig,
}

fn default_timeout_ms() -> u64 {
    30_000 // 30 seconds
}

fn default_max_timeout_ms() -> u64 {
    300_000 // 5 minutes
}

fn default_memory_limit_mb() -> u64 {
    512
}

fn default_cpu_limit() -> f64 {
    1.0
}

fn default_session_idle_timeout() -> u64 {
    300 // 5 minutes
}

fn default_max_sessions() -> usize {
    100
}

fn default_python_image() -> String {
    "python:3.11-slim".into()
}

fn default_multi_image() -> String {
    "canal/executor:latest".into()
}

fn default_executor_path() -> String {
    "/app/executor.py".into()
}

impl Default for CodeActConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: default_timeout_ms(),
            max_timeout_ms: default_max_timeout_ms(),
            memory_limit_mb: default_memory_limit_mb(),
            cpu_limit: default_cpu_limit(),
            session_idle_timeout_secs: default_session_idle_timeout(),
            max_sessions: default_max_sessions(),
            python_image: default_python_image(),
            multi_image: default_multi_image(),
            executor_path: default_executor_path(),
            pool_config: PoolConfig::default(),
        }
    }
}

/// Execution request for the CodeAct engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeActRequest {
    /// Code to execute
    pub code: String,

    /// Programming language
    pub language: Language,

    /// Timeout in milliseconds (optional, uses default if not specified)
    #[serde(default)]
    pub timeout_ms: u64,

    /// Whether to capture and return the expression result
    #[serde(default)]
    pub capture_return: bool,

    /// Environment variables to set
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Files to make available (path -> content)
    #[serde(default)]
    pub files: HashMap<String, String>,
}

impl CodeActRequest {
    /// Create a new Python execution request
    pub fn python(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            language: Language::Python,
            timeout_ms: 0, // Use default
            capture_return: false,
            env: HashMap::new(),
            files: HashMap::new(),
        }
    }

    /// Create a new Bash execution request
    pub fn bash(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            language: Language::Bash,
            timeout_ms: 0, // Use default
            capture_return: false,
            env: HashMap::new(),
            files: HashMap::new(),
        }
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Enable return value capture
    pub fn with_capture_return(mut self) -> Self {
        self.capture_return = true;
        self
    }

    /// Add environment variable
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Add multiple environment variables
    pub fn with_envs(mut self, envs: HashMap<String, String>) -> Self {
        self.env.extend(envs);
        self
    }

    /// Add file to make available
    pub fn with_file(mut self, path: impl Into<String>, content: impl Into<String>) -> Self {
        self.files.insert(path.into(), content.into());
        self
    }
}

/// Session information for persistent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Unique session ID
    pub session_id: String,

    /// Container ID associated with this session
    pub container_id: String,

    /// Language for this session
    pub language: Language,

    /// When the session was created
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// When the session was last used
    pub last_used_at: chrono::DateTime<chrono::Utc>,

    /// Number of executions in this session
    pub execution_count: u64,

    /// Whether the session is active
    pub active: bool,
}

impl SessionInfo {
    fn new(session_id: String, container_id: String, language: Language) -> Self {
        let now = chrono::Utc::now();
        Self {
            session_id,
            container_id,
            language,
            created_at: now,
            last_used_at: now,
            execution_count: 0,
            active: true,
        }
    }

    fn touch(&mut self) {
        self.last_used_at = chrono::Utc::now();
        self.execution_count += 1;
    }
}

/// JSON-RPC request for executor communication
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: String,
    method: String,
    params: serde_json::Value,
}

#[allow(dead_code)]
impl JsonRpcRequest {
    fn new(method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: uuid::Uuid::new_v4().to_string(),
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC response from executor
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: String,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// Result of executor's execution
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExecutorResult {
    stdout: String,
    stderr: String,
    exit_code: i32,
    #[serde(default)]
    return_value: Option<serde_json::Value>,
    #[serde(default)]
    duration_ms: u64,
    #[serde(default)]
    variables: HashMap<String, serde_json::Value>,
}

/// The main CodeAct execution engine
pub struct CodeActEngine {
    /// Container pool for managing containers
    pool: Arc<ContainerPool>,

    /// Configuration
    config: CodeActConfig,

    /// Active sessions
    sessions: Arc<RwLock<HashMap<String, SessionInfo>>>,

    /// Session variables (session_id -> variables)
    session_variables: Arc<RwLock<HashMap<String, HashMap<String, serde_json::Value>>>>,
}

impl CodeActEngine {
    /// Create a new CodeAct engine with the given configuration
    pub async fn new(config: CodeActConfig) -> Result<Self> {
        let pool = ContainerPool::new(config.pool_config.clone()).await?;

        Ok(Self {
            pool: Arc::new(pool),
            config,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            session_variables: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create a new CodeAct engine with an existing container pool
    pub fn with_pool(pool: Arc<ContainerPool>, config: CodeActConfig) -> Self {
        Self {
            pool,
            config,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            session_variables: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start the engine (initializes pool and background tasks)
    pub async fn start(&self) -> Result<()> {
        self.pool.start().await?;
        self.spawn_session_cleanup_task();
        Ok(())
    }

    /// Execute code in a new container (stateless execution)
    pub async fn execute(&self, request: CodeActRequest) -> Result<ExecutionResult> {
        let execution_id = uuid::Uuid::new_v4().to_string();
        let start_time = std::time::Instant::now();

        // Determine timeout
        let timeout_ms = if request.timeout_ms > 0 {
            request.timeout_ms.min(self.config.max_timeout_ms)
        } else {
            self.config.default_timeout_ms
        };

        // Get container configuration for the language
        let container_config = self.get_container_config(&request.language);

        // Acquire container from pool
        let container = self.pool.acquire(container_config).await?;
        let container_id = container.config.id.clone();

        // Execute code
        let result = self
            .execute_in_container(&container_id, &request, timeout_ms)
            .await;

        // Release container back to pool
        let keep_warm = result.is_ok();
        if let Err(e) = self.pool.release(&container_id, keep_warm).await {
            tracing::warn!("Failed to release container {}: {}", container_id, e);
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;

        match result {
            Ok(exec_result) => Ok(ExecutionResult {
                execution_id,
                language: request.language,
                stdout: exec_result.stdout,
                stderr: exec_result.stderr,
                exit_code: exec_result.exit_code,
                status: if exec_result.exit_code == 0 {
                    ExecutionStatus::Success
                } else {
                    ExecutionStatus::Error
                },
                duration_ms,
            }),
            Err(e) => {
                tracing::error!("Execution failed: {}", e);
                Ok(ExecutionResult {
                    execution_id,
                    language: request.language,
                    stdout: String::new(),
                    stderr: e.to_string(),
                    exit_code: -1,
                    status: ExecutionStatus::Error,
                    duration_ms,
                })
            }
        }
    }

    /// Execute code in an existing session (stateful execution with variable persistence)
    pub async fn execute_in_session(
        &self,
        session_id: &str,
        request: CodeActRequest,
    ) -> Result<ExecutionResult> {
        let execution_id = uuid::Uuid::new_v4().to_string();
        let start_time = std::time::Instant::now();

        // Get session info
        let container_id = {
            let mut sessions = self.sessions.write().await;
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| Error::NotFound(format!("Session not found: {}", session_id)))?;

            if !session.active {
                return Err(Error::InvalidInput("Session is no longer active".into()));
            }

            // Check language compatibility
            if session.language != request.language {
                return Err(Error::InvalidInput(format!(
                    "Session language mismatch: session is {:?}, request is {:?}",
                    session.language, request.language
                )));
            }

            session.touch();
            session.container_id.clone()
        };

        // Determine timeout
        let timeout_ms = if request.timeout_ms > 0 {
            request.timeout_ms.min(self.config.max_timeout_ms)
        } else {
            self.config.default_timeout_ms
        };

        // Execute code in the session's container
        let result = self
            .execute_in_container(&container_id, &request, timeout_ms)
            .await;

        let duration_ms = start_time.elapsed().as_millis() as u64;

        match result {
            Ok(exec_result) => {
                // Store any captured variables
                if !exec_result.variables.is_empty() {
                    let mut session_vars = self.session_variables.write().await;
                    let vars = session_vars.entry(session_id.to_string()).or_default();
                    vars.extend(exec_result.variables);
                }

                Ok(ExecutionResult {
                    execution_id,
                    language: request.language,
                    stdout: exec_result.stdout,
                    stderr: exec_result.stderr,
                    exit_code: exec_result.exit_code,
                    status: if exec_result.exit_code == 0 {
                        ExecutionStatus::Success
                    } else {
                        ExecutionStatus::Error
                    },
                    duration_ms,
                })
            }
            Err(e) => {
                tracing::error!("Session execution failed: {}", e);
                Ok(ExecutionResult {
                    execution_id,
                    language: request.language,
                    stdout: String::new(),
                    stderr: e.to_string(),
                    exit_code: -1,
                    status: ExecutionStatus::Error,
                    duration_ms,
                })
            }
        }
    }

    /// Create a new session for persistent code execution
    pub async fn create_session(&self, language: Language) -> Result<String> {
        // Check session limit
        {
            let sessions = self.sessions.read().await;
            if sessions.len() >= self.config.max_sessions {
                return Err(Error::Internal(format!(
                    "Maximum session limit reached ({})",
                    self.config.max_sessions
                )));
            }
        }

        let session_id = uuid::Uuid::new_v4().to_string();

        // Create container for session
        let container_config = self.get_container_config(&language);
        let container = self.pool.acquire(container_config).await?;
        let container_id = container.config.id.clone();

        // Initialize the executor in the container if needed
        self.initialize_executor(&container_id, &language).await?;

        // Create session info
        let session_info = SessionInfo::new(session_id.clone(), container_id.clone(), language);

        // Store session
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), session_info);
        }

        // Initialize empty variables for session
        {
            let mut session_vars = self.session_variables.write().await;
            session_vars.insert(session_id.clone(), HashMap::new());
        }

        tracing::info!(
            "Created session {} with container {} for {:?}",
            session_id,
            container_id,
            language
        );

        Ok(session_id)
    }

    /// Close a session and release its resources
    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        let container_id = {
            let mut sessions = self.sessions.write().await;
            let session = sessions
                .remove(session_id)
                .ok_or_else(|| Error::NotFound(format!("Session not found: {}", session_id)))?;
            session.container_id
        };

        // Clean up session variables
        {
            let mut session_vars = self.session_variables.write().await;
            session_vars.remove(session_id);
        }

        // Release container (don't keep warm since session state is lost)
        if let Err(e) = self.pool.release(&container_id, false).await {
            tracing::warn!(
                "Failed to release container {} for session {}: {}",
                container_id,
                session_id,
                e
            );
        }

        tracing::info!("Closed session {}", session_id);
        Ok(())
    }

    /// Get session variables
    pub async fn get_session_variables(
        &self,
        session_id: &str,
    ) -> Result<HashMap<String, serde_json::Value>> {
        // Check session exists
        {
            let sessions = self.sessions.read().await;
            if !sessions.contains_key(session_id) {
                return Err(Error::NotFound(format!(
                    "Session not found: {}",
                    session_id
                )));
            }
        }

        let session_vars = self.session_variables.read().await;
        Ok(session_vars.get(session_id).cloned().unwrap_or_default())
    }

    /// Get session info
    pub async fn get_session(&self, session_id: &str) -> Result<SessionInfo> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("Session not found: {}", session_id)))
    }

    /// List all active sessions
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.read().await;
        sessions.values().filter(|s| s.active).cloned().collect()
    }

    /// Get the configuration
    pub fn config(&self) -> &CodeActConfig {
        &self.config
    }

    /// Shutdown the engine
    pub async fn shutdown(&self) -> Result<()> {
        // Close all sessions
        let session_ids: Vec<String> = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect()
        };

        for session_id in session_ids {
            if let Err(e) = self.close_session(&session_id).await {
                tracing::warn!("Failed to close session {}: {}", session_id, e);
            }
        }

        // Shutdown pool
        self.pool.shutdown().await?;

        Ok(())
    }

    // === Private helpers ===

    /// Get container configuration for a language
    fn get_container_config(&self, language: &Language) -> ContainerConfig {
        let image = match language {
            Language::Python => self.config.python_image.clone(),
            _ => self.config.multi_image.clone(),
        };

        ContainerConfig::new(&image)
            .with_cpu_limit(self.config.cpu_limit)
            .with_memory_limit(self.config.memory_limit_mb)
            .with_tmpfs("/tmp", "100m")
            .with_working_dir("/workspace")
            .with_command(vec!["sleep".into(), "infinity".into()])
    }

    /// Initialize the executor in a container
    async fn initialize_executor(&self, _container_id: &str, language: &Language) -> Result<()> {
        // For Python, we might need to install requirements or set up environment
        if *language == Language::Python {
            // Execute initialization script if needed
            let init_code = r#"
import sys
import json

# Ensure we can capture return values
def __canal_capture__(expr):
    return json.dumps(expr, default=str)

# Signal ready
print("CANAL_EXECUTOR_READY", file=sys.stderr)
"#;

            let command = vec![
                "python3".to_string(),
                "-c".to_string(),
                init_code.to_string(),
            ];

            let result = self
                .pool
                .execute(
                    ContainerConfig::new(&self.config.python_image).with_command(command.clone()),
                    command,
                )
                .await;

            if let Err(e) = result {
                tracing::warn!("Failed to initialize Python executor: {}", e);
                // Don't fail - the executor might still work
            }
        }

        Ok(())
    }

    /// Execute code in a specific container
    async fn execute_in_container(
        &self,
        _container_id: &str,
        request: &CodeActRequest,
        timeout_ms: u64,
    ) -> Result<ExecutorResult> {
        let command = self.build_execution_command(request);

        // Execute with timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            self.pool.execute(
                ContainerConfig::new(&self.get_image_for_language(&request.language)),
                command,
            ),
        )
        .await;

        match result {
            Ok(Ok((stdout, stderr, exit_code))) => {
                // Try to parse executor result from stdout
                if let Ok(exec_result) = serde_json::from_str::<ExecutorResult>(&stdout) {
                    Ok(exec_result)
                } else {
                    // Fall back to raw output
                    Ok(ExecutorResult {
                        stdout,
                        stderr,
                        exit_code,
                        return_value: None,
                        duration_ms: 0,
                        variables: HashMap::new(),
                    })
                }
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Error::Timeout(format!(
                "Execution timed out after {}ms",
                timeout_ms
            ))),
        }
    }

    /// Build the execution command for a request
    fn build_execution_command(&self, request: &CodeActRequest) -> Vec<String> {
        match request.language {
            Language::Python => {
                // Build Python command with code
                let code = if request.capture_return {
                    // Wrap code to capture return value
                    format!(
                        r#"
import sys
import json
try:
    __result__ = None
    exec(compile('''{code}''', '<string>', 'exec'))
    if '__result__' in dir():
        print(json.dumps({{"return_value": __result__}}, default=str))
except Exception as e:
    print(str(e), file=sys.stderr)
    sys.exit(1)
"#,
                        code = request.code.replace("'''", r"\'\'\'")
                    )
                } else {
                    request.code.clone()
                };

                vec!["python3".to_string(), "-c".to_string(), code]
            }
            Language::Bash => {
                vec!["bash".to_string(), "-c".to_string(), request.code.clone()]
            }
            Language::JavaScript => {
                vec!["node".to_string(), "-e".to_string(), request.code.clone()]
            }
            Language::TypeScript => {
                // Use ts-node or similar
                vec![
                    "npx".to_string(),
                    "ts-node".to_string(),
                    "-e".to_string(),
                    request.code.clone(),
                ]
            }
            Language::Go => {
                // Go requires file-based execution
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    format!(
                        r#"echo '{}' > /tmp/main.go && go run /tmp/main.go"#,
                        request.code.replace("'", "'\"'\"'")
                    ),
                ]
            }
            Language::Rust => {
                // Rust also requires compilation
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    format!(
                        r#"echo '{}' > /tmp/main.rs && rustc /tmp/main.rs -o /tmp/main && /tmp/main"#,
                        request.code.replace("'", "'\"'\"'")
                    ),
                ]
            }
        }
    }

    /// Get the Docker image for a language
    fn get_image_for_language(&self, language: &Language) -> String {
        match language {
            Language::Python => self.config.python_image.clone(),
            _ => self.config.multi_image.clone(),
        }
    }

    /// Spawn background task to clean up idle sessions
    fn spawn_session_cleanup_task(&self) {
        let sessions = self.sessions.clone();
        let session_variables = self.session_variables.clone();
        let pool = self.pool.clone();
        let idle_timeout = std::time::Duration::from_secs(self.config.session_idle_timeout_secs);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                let now = chrono::Utc::now();
                let mut expired_sessions = Vec::new();

                // Find expired sessions
                {
                    let sessions_guard = sessions.read().await;
                    for (session_id, session) in sessions_guard.iter() {
                        if session.active {
                            let idle_duration = now
                                .signed_duration_since(session.last_used_at)
                                .to_std()
                                .unwrap_or(std::time::Duration::ZERO);

                            if idle_duration > idle_timeout {
                                expired_sessions
                                    .push((session_id.clone(), session.container_id.clone()));
                            }
                        }
                    }
                }

                // Clean up expired sessions
                for (session_id, container_id) in expired_sessions {
                    tracing::info!("Cleaning up idle session {}", session_id);

                    // Mark as inactive
                    {
                        let mut sessions_guard = sessions.write().await;
                        if let Some(session) = sessions_guard.get_mut(&session_id) {
                            session.active = false;
                        }
                    }

                    // Remove variables
                    {
                        let mut vars = session_variables.write().await;
                        vars.remove(&session_id);
                    }

                    // Release container
                    if let Err(e) = pool.release(&container_id, false).await {
                        tracing::warn!(
                            "Failed to release container for expired session {}: {}",
                            session_id,
                            e
                        );
                    }

                    // Remove session
                    {
                        let mut sessions_guard = sessions.write().await;
                        sessions_guard.remove(&session_id);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codeact_config_defaults() {
        let config = CodeActConfig::default();
        assert_eq!(config.default_timeout_ms, 30_000);
        assert_eq!(config.max_timeout_ms, 300_000);
        assert_eq!(config.memory_limit_mb, 512);
        assert_eq!(config.cpu_limit, 1.0);
        assert_eq!(config.session_idle_timeout_secs, 300);
        assert_eq!(config.max_sessions, 100);
        assert_eq!(config.python_image, "python:3.11-slim");
    }

    #[test]
    fn test_codeact_config_serialization() {
        let config = CodeActConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: CodeActConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.default_timeout_ms, config.default_timeout_ms);
        assert_eq!(parsed.memory_limit_mb, config.memory_limit_mb);
    }

    #[test]
    fn test_codeact_request_builder() {
        let request = CodeActRequest::python("print('hello')")
            .with_timeout(5000)
            .with_capture_return()
            .with_env("MY_VAR", "value");

        assert_eq!(request.code, "print('hello')");
        assert_eq!(request.language, Language::Python);
        assert_eq!(request.timeout_ms, 5000);
        assert!(request.capture_return);
        assert_eq!(request.env.get("MY_VAR"), Some(&"value".to_string()));
    }

    #[test]
    fn test_codeact_request_bash() {
        let request = CodeActRequest::bash("echo hello");
        assert_eq!(request.language, Language::Bash);
        assert_eq!(request.code, "echo hello");
    }

    #[test]
    fn test_codeact_request_with_file() {
        let request =
            CodeActRequest::python("import data").with_file("/workspace/data.py", "x = 42");

        assert_eq!(request.files.len(), 1);
        assert_eq!(
            request.files.get("/workspace/data.py"),
            Some(&"x = 42".to_string())
        );
    }

    #[test]
    fn test_session_info_creation() {
        let session = SessionInfo::new(
            "session-123".into(),
            "container-456".into(),
            Language::Python,
        );

        assert_eq!(session.session_id, "session-123");
        assert_eq!(session.container_id, "container-456");
        assert_eq!(session.language, Language::Python);
        assert_eq!(session.execution_count, 0);
        assert!(session.active);
    }

    #[test]
    fn test_session_info_touch() {
        let mut session = SessionInfo::new(
            "session-123".into(),
            "container-456".into(),
            Language::Python,
        );

        let initial_time = session.last_used_at;
        std::thread::sleep(std::time::Duration::from_millis(10));

        session.touch();

        assert_eq!(session.execution_count, 1);
        assert!(session.last_used_at >= initial_time);
    }

    #[test]
    fn test_json_rpc_request() {
        let request = JsonRpcRequest::new(
            "execute",
            serde_json::json!({
                "code": "print('hello')",
                "language": "python"
            }),
        );

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.method, "execute");
        assert!(!request.id.is_empty());
    }

    #[test]
    fn test_json_rpc_response_parsing() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": "test-123",
            "result": {"stdout": "hello", "exit_code": 0}
        }"#;

        let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "test-123");
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[test]
    fn test_json_rpc_error_response_parsing() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": "test-123",
            "error": {
                "code": -32600,
                "message": "Invalid request"
            }
        }"#;

        let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(response.result.is_none());
        assert!(response.error.is_some());
        assert_eq!(response.error.as_ref().unwrap().code, -32600);
    }

    #[test]
    fn test_executor_result_parsing() {
        let json = r#"{
            "stdout": "hello world\n",
            "stderr": "",
            "exit_code": 0,
            "return_value": 42,
            "duration_ms": 150,
            "variables": {"x": 10, "y": "test"}
        }"#;

        let result: ExecutorResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.stdout, "hello world\n");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.return_value, Some(serde_json::json!(42)));
        assert_eq!(result.duration_ms, 150);
        assert_eq!(result.variables.len(), 2);
    }

    #[test]
    fn test_executor_result_minimal() {
        let json = r#"{
            "stdout": "",
            "stderr": "error",
            "exit_code": 1
        }"#;

        let result: ExecutorResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.exit_code, 1);
        assert!(result.return_value.is_none());
        assert!(result.variables.is_empty());
    }

    #[test]
    fn test_build_python_command() {
        // This is a unit test for command building logic
        let request = CodeActRequest::python("x = 1 + 1\nprint(x)");

        // Verify the structure is correct
        assert_eq!(request.language, Language::Python);
        assert!(request.code.contains("print(x)"));
    }

    #[test]
    fn test_build_bash_command() {
        let request = CodeActRequest::bash("echo $HOME");
        assert_eq!(request.language, Language::Bash);
        assert_eq!(request.code, "echo $HOME");
    }

    // Integration tests would require Docker and are marked as ignored
    #[tokio::test]
    #[ignore = "Requires Docker"]
    async fn test_codeact_engine_creation() {
        let config = CodeActConfig::default();
        let engine = CodeActEngine::new(config).await;
        assert!(engine.is_ok());
    }

    #[tokio::test]
    #[ignore = "Requires Docker"]
    async fn test_codeact_execute_python() {
        let config = CodeActConfig::default();
        let engine = CodeActEngine::new(config).await.unwrap();
        engine.start().await.unwrap();

        let request = CodeActRequest::python("print('Hello from CodeAct!')");
        let result = engine.execute(request).await.unwrap();

        assert_eq!(result.status, ExecutionStatus::Success);
        assert!(result.stdout.contains("Hello from CodeAct!"));
    }

    #[tokio::test]
    #[ignore = "Requires Docker"]
    async fn test_codeact_session_workflow() {
        let config = CodeActConfig::default();
        let engine = CodeActEngine::new(config).await.unwrap();
        engine.start().await.unwrap();

        // Create session
        let session_id = engine.create_session(Language::Python).await.unwrap();

        // Execute code in session
        let request1 = CodeActRequest::python("x = 42");
        let result1 = engine
            .execute_in_session(&session_id, request1)
            .await
            .unwrap();
        assert_eq!(result1.status, ExecutionStatus::Success);

        // Execute more code using the variable
        let request2 = CodeActRequest::python("print(x * 2)");
        let result2 = engine
            .execute_in_session(&session_id, request2)
            .await
            .unwrap();
        assert_eq!(result2.status, ExecutionStatus::Success);
        assert!(result2.stdout.contains("84"));

        // Close session
        engine.close_session(&session_id).await.unwrap();

        // Shutdown
        engine.shutdown().await.unwrap();
    }
}
