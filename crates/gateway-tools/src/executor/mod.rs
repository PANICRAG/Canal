//! Code Executor Module
//!
//! Provides secure code execution capabilities with Docker container isolation.
//! Supports Python, Bash, JavaScript, TypeScript, Go, and Rust execution
//! with resource limits and timeout controls.

mod bash;
mod codeact;
mod config;
mod container;
mod detector;
mod docker;
pub mod error;
#[cfg(feature = "unsafe-executors")]
mod golang;
#[cfg(feature = "unsafe-executors")]
mod nodejs;
mod pool;
mod python;
pub mod result;
pub mod router;
#[cfg(feature = "unsafe-executors")]
mod rust_exec;
pub mod security;
mod types;

pub use bash::BashExecutor;
pub use config::{
    BashConfig, DockerConfig, ExecutorConfig, LanguageConfig, PythonConfig, ResourceLimits,
};
pub use detector::{DetectedLanguage, LanguageDetector};
pub use docker::{ContainerInfo, ContainerStatus, DockerManager};
#[cfg(feature = "unsafe-executors")]
pub use golang::{GoExecutor, GoResult};
#[cfg(feature = "unsafe-executors")]
pub use nodejs::{NodeJsExecutor, NodeJsResult};
pub use python::{
    Artifact, ExecutionContext, ExecutionTiming, ExtendedExecutionResult, ExtendedPythonConfig,
    PythonExecutor, SandboxMode,
};
#[cfg(feature = "unsafe-executors")]
pub use rust_exec::{RustExecutor, RustResult};

// CodeAct execution engine
pub use codeact::{CodeActConfig, CodeActEngine, CodeActRequest, SessionInfo};

// Container lifecycle management
pub use container::ContainerManager;
pub use pool::{ContainerPool, HealthCheckResult, PoolState};
pub use types::{
    ContainerConfig, ContainerRuntime, ContainerState, Mount, MountType, NetworkMode, PoolConfig,
    PoolStats,
};

// Error handling and recovery
pub use error::{
    parse_python_error, ErrorContext, ErrorHandler, ExecutionError, RecoveryAction,
    RecoveryStrategy,
};

// CodeAct result parsing
pub use result::{
    Artifact as CodeActArtifact, ArtifactType as CodeActArtifactType, BinaryResultHandler,
    CacheStats, CachedResult, CodeActError, CodeActResult, ErrorType as CodeActErrorType,
    ExecutionOutput, ExecutionStatus as CodeActExecutionStatus, ExecutionTiming as CodeActTiming,
    ParsedOutput, ParserConfig, ResultCache, ResultParser,
};

// Security validation
pub use security::{
    CodeLocation, DangerousPattern, DangerousPatternConfig, ImportWhitelist, IssueType,
    SecurityConfig, SecurityIssue, SecurityValidator, Severity, SharedSecurityValidator,
    ValidationMetadata, ValidationResult,
};

// Unified CodeAct Router (local Docker / cloud VM routing)
pub use router::{
    AvailableResources, CloudExecutionStrategy, CodeExecutionRequest, ExecutionStrategy,
    ExecutorHealth, FallbackStrategy, LoadBalanceStrategy, LocalExecutionStrategy, QuotaEnforcer,
    QuotaGuard, ResourceQuota, ResourceTracker, ResourceUsage, RouterConfig, RouterMetrics,
    RouterMode, UnifiedCodeActRouter, UnifiedCodeActRouterBuilder, UnifiedRouterStatus,
};

use crate::error::{ServiceError as Error, ServiceResult as Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Execution output event for streaming results
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ExecutionEvent {
    /// Execution started
    #[serde(rename = "started")]
    Started { execution_id: String },

    /// Standard output chunk
    #[serde(rename = "stdout")]
    Stdout { text: String },

    /// Standard error chunk
    #[serde(rename = "stderr")]
    Stderr { text: String },

    /// Execution completed
    #[serde(rename = "completed")]
    Completed { exit_code: i32, duration_ms: u64 },

    /// Execution failed or timed out
    #[serde(rename = "error")]
    Error { message: String },
}

/// Result of code execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Unique execution ID
    pub execution_id: String,
    /// Language of executed code
    pub language: Language,
    /// Combined stdout
    pub stdout: String,
    /// Combined stderr
    pub stderr: String,
    /// Exit code (0 = success)
    pub exit_code: i32,
    /// Execution status
    pub status: ExecutionStatus,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

/// Supported execution languages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Python,
    Bash,
    JavaScript,
    TypeScript,
    Go,
    Rust,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::Python => write!(f, "python"),
            Language::Bash => write!(f, "bash"),
            Language::JavaScript => write!(f, "javascript"),
            Language::TypeScript => write!(f, "typescript"),
            Language::Go => write!(f, "go"),
            Language::Rust => write!(f, "rust"),
        }
    }
}

impl std::str::FromStr for Language {
    type Err = Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "python" | "python3" | "py" => Ok(Language::Python),
            "bash" | "sh" | "shell" => Ok(Language::Bash),
            "javascript" | "js" | "node" => Ok(Language::JavaScript),
            "typescript" | "ts" => Ok(Language::TypeScript),
            "go" | "golang" => Ok(Language::Go),
            "rust" | "rs" => Ok(Language::Rust),
            _ => Err(Error::Unsupported(format!("Unsupported language: {}", s))),
        }
    }
}

/// Execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionStatus {
    Pending,
    Running,
    Success,
    Error,
    Timeout,
    Killed,
}

/// Code execution request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRequest {
    /// Code to execute
    pub code: String,
    /// Programming language
    pub language: Language,
    /// Optional timeout in milliseconds (default from config)
    pub timeout_ms: Option<u64>,
    /// Whether to stream output
    pub stream: bool,
    /// Optional working directory (must be in allowed directories)
    pub working_dir: Option<String>,
}

/// Trait for language-specific executors
#[async_trait::async_trait]
pub trait LanguageExecutor: Send + Sync {
    /// Get the language this executor handles
    fn language(&self) -> Language;

    /// Execute code and return the result
    async fn execute(&self, request: &ExecutionRequest) -> Result<ExecutionResult>;

    /// Execute code with streaming output
    async fn execute_streaming(
        &self,
        request: &ExecutionRequest,
        output_tx: mpsc::Sender<ExecutionEvent>,
    ) -> Result<ExecutionResult>;
}

/// Main code executor that manages all language executors
#[allow(dead_code)]
pub struct CodeExecutor {
    docker_manager: Arc<DockerManager>,
    python_executor: Arc<PythonExecutor>,
    bash_executor: Arc<BashExecutor>,
    #[cfg(feature = "unsafe-executors")]
    nodejs_executor: Arc<NodeJsExecutor>,
    #[cfg(feature = "unsafe-executors")]
    go_executor: Arc<GoExecutor>,
    #[cfg(feature = "unsafe-executors")]
    rust_executor: Arc<RustExecutor>,
    config: ExecutorConfig,
    work_dir: String,
}

impl CodeExecutor {
    /// Create a new code executor with the given configuration
    pub async fn new(config: ExecutorConfig) -> Result<Self> {
        let docker_manager = Arc::new(DockerManager::new(config.docker.clone()).await?);

        let python_executor = Arc::new(PythonExecutor::new(
            docker_manager.clone(),
            config.python.clone(),
        ));

        let bash_executor = Arc::new(BashExecutor::new(
            docker_manager.clone(),
            config.bash.clone(),
        ));

        let work_dir = std::env::var("EXECUTOR_WORK_DIR")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string());

        #[cfg(feature = "unsafe-executors")]
        let nodejs_executor = Arc::new(NodeJsExecutor::new(&work_dir));
        #[cfg(feature = "unsafe-executors")]
        let go_executor = Arc::new(GoExecutor::new(&work_dir));
        #[cfg(feature = "unsafe-executors")]
        let rust_executor = Arc::new(RustExecutor::new(&work_dir));

        Ok(Self {
            docker_manager,
            python_executor,
            bash_executor,
            #[cfg(feature = "unsafe-executors")]
            nodejs_executor,
            #[cfg(feature = "unsafe-executors")]
            go_executor,
            #[cfg(feature = "unsafe-executors")]
            rust_executor,
            config,
            work_dir,
        })
    }

    /// Check if a language is enabled
    pub fn is_language_enabled(&self, language: Language) -> bool {
        match language {
            Language::Python => self.config.python.enabled,
            Language::Bash => self.config.bash.enabled,
            // R5-C1/C2/C3: Non-Docker executors require `unsafe-executors` feature
            #[cfg(feature = "unsafe-executors")]
            Language::JavaScript | Language::TypeScript | Language::Go | Language::Rust => true,
            #[cfg(not(feature = "unsafe-executors"))]
            Language::JavaScript | Language::TypeScript | Language::Go | Language::Rust => false,
        }
    }

    /// Get the executor for a specific language
    fn get_executor(&self, language: Language) -> Result<&dyn LanguageExecutor> {
        match language {
            Language::Python => {
                if !self.config.python.enabled {
                    return Err(Error::Internal("Python execution is disabled".into()));
                }
                Ok(self.python_executor.as_ref())
            }
            Language::Bash => {
                if !self.config.bash.enabled {
                    return Err(Error::Internal("Bash execution is disabled".into()));
                }
                Ok(self.bash_executor.as_ref())
            }
            _ => Err(Error::Unsupported(format!(
                "{:?} uses standalone executor, not LanguageExecutor trait",
                language
            ))),
        }
    }

    /// Execute code
    pub async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
        let _execution_id = uuid::Uuid::new_v4().to_string();
        #[cfg(feature = "unsafe-executors")]
        let execution_id = _execution_id;

        match request.language {
            Language::Python | Language::Bash => {
                let executor = self.get_executor(request.language)?;
                executor.execute(&request).await
            }
            // R5-C1/C2/C3: Non-Docker executors gated behind `unsafe-executors` feature
            #[cfg(feature = "unsafe-executors")]
            Language::JavaScript => {
                let result = self
                    .nodejs_executor
                    .execute_javascript(&request.code, &[])
                    .await?;
                Ok(ExecutionResult {
                    execution_id,
                    language: Language::JavaScript,
                    stdout: result.stdout,
                    stderr: result.stderr,
                    exit_code: result.exit_code,
                    status: if result.exit_code == 0 {
                        ExecutionStatus::Success
                    } else {
                        ExecutionStatus::Error
                    },
                    duration_ms: result.execution_time_ms,
                })
            }
            #[cfg(feature = "unsafe-executors")]
            Language::TypeScript => {
                let result = self
                    .nodejs_executor
                    .execute_typescript(&request.code, &[])
                    .await?;
                Ok(ExecutionResult {
                    execution_id,
                    language: Language::TypeScript,
                    stdout: result.stdout,
                    stderr: result.stderr,
                    exit_code: result.exit_code,
                    status: if result.exit_code == 0 {
                        ExecutionStatus::Success
                    } else {
                        ExecutionStatus::Error
                    },
                    duration_ms: result.execution_time_ms,
                })
            }
            #[cfg(feature = "unsafe-executors")]
            Language::Go => {
                let result = self.go_executor.execute(&request.code).await?;
                Ok(ExecutionResult {
                    execution_id,
                    language: Language::Go,
                    stdout: result.stdout,
                    stderr: result.stderr,
                    exit_code: result.exit_code,
                    status: if result.exit_code == 0 {
                        ExecutionStatus::Success
                    } else {
                        ExecutionStatus::Error
                    },
                    duration_ms: result.execution_time_ms,
                })
            }
            #[cfg(feature = "unsafe-executors")]
            Language::Rust => {
                let result = self.rust_executor.execute(&request.code).await?;
                Ok(ExecutionResult {
                    execution_id,
                    language: Language::Rust,
                    stdout: result.stdout,
                    stderr: result.stderr,
                    exit_code: result.exit_code,
                    status: if result.exit_code == 0 {
                        ExecutionStatus::Success
                    } else {
                        ExecutionStatus::Error
                    },
                    duration_ms: result.execution_time_ms,
                })
            }
            #[cfg(not(feature = "unsafe-executors"))]
            Language::JavaScript | Language::TypeScript | Language::Go | Language::Rust => {
                Err(Error::Unsupported(format!(
                    "{} execution disabled — unsandboxed executors require the `unsafe-executors` feature",
                    request.language
                )))
            }
        }
    }

    /// Execute code with streaming output
    pub async fn execute_streaming(
        &self,
        request: ExecutionRequest,
        output_tx: mpsc::Sender<ExecutionEvent>,
    ) -> Result<ExecutionResult> {
        match request.language {
            Language::Python | Language::Bash => {
                let executor = self.get_executor(request.language)?;
                executor.execute_streaming(&request, output_tx).await
            }
            _ => {
                let _ = output_tx
                    .send(ExecutionEvent::Started {
                        execution_id: "streaming".to_string(),
                    })
                    .await;

                let result = self.execute(request).await?;

                if !result.stdout.is_empty() {
                    let _ = output_tx
                        .send(ExecutionEvent::Stdout {
                            text: result.stdout.clone(),
                        })
                        .await;
                }
                if !result.stderr.is_empty() {
                    let _ = output_tx
                        .send(ExecutionEvent::Stderr {
                            text: result.stderr.clone(),
                        })
                        .await;
                }
                let _ = output_tx
                    .send(ExecutionEvent::Completed {
                        exit_code: result.exit_code,
                        duration_ms: result.duration_ms,
                    })
                    .await;

                Ok(result)
            }
        }
    }

    /// Check Docker connectivity and health
    pub async fn health_check(&self) -> Result<bool> {
        self.docker_manager.health_check().await
    }

    /// Clean up any orphaned containers
    pub async fn cleanup(&self) -> Result<()> {
        self.docker_manager.cleanup_orphaned_containers().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_from_str() {
        assert_eq!("python".parse::<Language>().unwrap(), Language::Python);
        assert_eq!("Python".parse::<Language>().unwrap(), Language::Python);
        assert_eq!("py".parse::<Language>().unwrap(), Language::Python);
        assert_eq!("bash".parse::<Language>().unwrap(), Language::Bash);
        assert_eq!("sh".parse::<Language>().unwrap(), Language::Bash);
        assert!("ruby".parse::<Language>().is_err());
    }

    #[test]
    fn test_language_display() {
        assert_eq!(Language::Python.to_string(), "python");
        assert_eq!(Language::Bash.to_string(), "bash");
    }

    #[test]
    fn test_execution_status_serialize() {
        let status = ExecutionStatus::Success;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"success\"");
    }
}
