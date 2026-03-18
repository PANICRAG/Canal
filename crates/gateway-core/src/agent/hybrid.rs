//! MCP/CodeAct Hybrid Execution Module
//!
//! This module provides a unified interface for executing both MCP tool calls and
//! CodeAct code execution, enabling seamless hybrid workflows where agents can
//! leverage both paradigms.
//!
//! # Architecture
//!
//! ```text
//! HybridRouter
//!        |
//!        |-- ToolTypeDetector --> Identify MCP vs CodeAct requests
//!        |
//!        |-- McpGateway ---------> Route MCP tool calls
//!        |
//!        |-- UnifiedCodeActRouter -> Route CodeAct code execution
//!        |
//!        |-- ResultUnifier ------> Normalize results to unified format
//!        |
//!        |-- ErrorHandler -------> Unified error handling
//! ```
//!
//! # Features
//!
//! - **Tool Type Recognition**: Automatically distinguishes between MCP tools and CodeAct code
//! - **MCP Tool Routing**: Routes MCP tool calls through the McpGateway
//! - **CodeAct Routing**: Routes code execution through the UnifiedCodeActRouter
//! - **Unified Results**: All execution results normalized to a common format
//! - **Unified Errors**: Consistent error handling across execution types
//! - **Parallel Execution**: Support for executing multiple requests concurrently
//!
//! # Example
//!
//! ```rust,ignore
//! use gateway_core::agent::hybrid::{
//!     HybridRouter, ExecutionRequest, ToolType,
//! };
//!
//! let router = HybridRouter::builder()
//!     .mcp_gateway(mcp_gateway)
//!     .codeact_router(codeact_router)
//!     .build();
//!
//! // MCP tool call
//! let mcp_request = ExecutionRequest::mcp_tool("filesystem_read_file", json!({"path": "/tmp/test"}));
//! let result = router.execute(mcp_request).await?;
//!
//! // CodeAct code execution
//! let code_request = ExecutionRequest::code("print('Hello, World!')", "python");
//! let result = router.execute(code_request).await?;
//! ```

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use tracing::{debug, info, instrument};
use uuid::Uuid;

use crate::error::Error;
use crate::executor::result::CodeActResult;
use crate::executor::router::{CodeExecutionRequest, UnifiedCodeActRouter};
use crate::mcp::gateway::McpGateway;
use crate::mcp::protocol::ToolCallResult;

// ============================================================================
// Core Types
// ============================================================================

/// Type of tool/execution being requested
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolType {
    /// MCP protocol tool call
    #[serde(rename = "mcp")]
    Mcp,
    /// CodeAct code execution
    #[serde(rename = "codeact")]
    CodeAct,
    /// Browser automation (special case of CodeAct)
    #[serde(rename = "browser")]
    Browser,
    /// Unknown/undetected type
    #[serde(rename = "unknown")]
    Unknown,
}

impl Default for ToolType {
    fn default() -> Self {
        ToolType::Unknown
    }
}

impl std::fmt::Display for ToolType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolType::Mcp => write!(f, "mcp"),
            ToolType::CodeAct => write!(f, "codeact"),
            ToolType::Browser => write!(f, "browser"),
            ToolType::Unknown => write!(f, "unknown"),
        }
    }
}

/// Execution request that can be either MCP or CodeAct
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRequest {
    /// Unique request identifier
    pub id: String,
    /// Request type (automatically detected if not specified)
    pub request_type: ToolType,
    /// MCP tool name (for MCP requests)
    pub tool_name: Option<String>,
    /// MCP tool arguments (for MCP requests)
    pub arguments: Option<serde_json::Value>,
    /// Code to execute (for CodeAct requests)
    pub code: Option<String>,
    /// Programming language (for CodeAct requests)
    pub language: Option<String>,
    /// Timeout in milliseconds
    pub timeout_ms: u64,
    /// Session ID for stateful execution
    pub session_id: Option<String>,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
    /// Timestamp when request was created
    pub created_at: DateTime<Utc>,
}

impl ExecutionRequest {
    /// Create a new execution request
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            request_type: ToolType::Unknown,
            tool_name: None,
            arguments: None,
            code: None,
            language: None,
            timeout_ms: 30000,
            session_id: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Create an MCP tool call request
    pub fn mcp_tool(tool_name: impl Into<String>, arguments: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            request_type: ToolType::Mcp,
            tool_name: Some(tool_name.into()),
            arguments: Some(arguments),
            code: None,
            language: None,
            timeout_ms: 30000,
            session_id: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Create a CodeAct code execution request
    pub fn code(code: impl Into<String>, language: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            request_type: ToolType::CodeAct,
            tool_name: None,
            arguments: None,
            code: Some(code.into()),
            language: Some(language.into()),
            timeout_ms: 30000,
            session_id: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Create a browser automation request
    pub fn browser(code: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            request_type: ToolType::Browser,
            tool_name: None,
            arguments: None,
            code: Some(code.into()),
            language: Some("python".to_string()),
            timeout_ms: 60000, // Browser operations often take longer
            session_id: None,
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Set session ID
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Detect the request type based on contents
    pub fn detect_type(&mut self) {
        if self.request_type != ToolType::Unknown {
            return;
        }

        // Check for MCP tool call
        if self.tool_name.is_some() && self.arguments.is_some() {
            self.request_type = ToolType::Mcp;
            return;
        }

        // Check for CodeAct code
        if self.code.is_some() {
            // Check for browser-related patterns
            if let Some(ref code) = self.code {
                let code_lower = code.to_lowercase();
                if code_lower.contains("playwright")
                    || code_lower.contains("selenium")
                    || code_lower.contains("browser")
                    || code_lower.contains("page.goto")
                    || code_lower.contains("page.click")
                {
                    self.request_type = ToolType::Browser;
                    return;
                }
            }
            self.request_type = ToolType::CodeAct;
        }
    }

    /// Validate the request
    pub fn validate(&self) -> std::result::Result<(), HybridError> {
        match self.request_type {
            ToolType::Mcp => {
                if self.tool_name.is_none() {
                    return Err(HybridError::InvalidRequest(
                        "MCP request requires tool_name".to_string(),
                    ));
                }
            }
            ToolType::CodeAct | ToolType::Browser => {
                if self.code.is_none() {
                    return Err(HybridError::InvalidRequest(
                        "CodeAct request requires code".to_string(),
                    ));
                }
            }
            ToolType::Unknown => {
                return Err(HybridError::InvalidRequest(
                    "Request type could not be determined".to_string(),
                ));
            }
        }
        Ok(())
    }
}

impl Default for ExecutionRequest {
    fn default() -> Self {
        Self::new()
    }
}

/// Unified execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Request ID this result corresponds to
    pub request_id: String,
    /// Type of execution performed
    pub execution_type: ToolType,
    /// Whether execution was successful
    pub success: bool,
    /// Text output/result
    pub output: Option<String>,
    /// Structured data output (if any)
    pub data: Option<serde_json::Value>,
    /// Error information (if failed)
    pub error: Option<HybridErrorInfo>,
    /// Artifacts produced (for CodeAct)
    pub artifacts: Vec<ArtifactInfo>,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
    /// Timestamp when execution completed
    pub completed_at: DateTime<Utc>,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

impl ExecutionResult {
    /// Create a successful result
    pub fn success(request_id: impl Into<String>, execution_type: ToolType) -> Self {
        Self {
            request_id: request_id.into(),
            execution_type,
            success: true,
            output: None,
            data: None,
            error: None,
            artifacts: Vec::new(),
            duration_ms: 0,
            completed_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Create a failed result
    pub fn failure(
        request_id: impl Into<String>,
        execution_type: ToolType,
        error: HybridErrorInfo,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            execution_type,
            success: false,
            output: None,
            data: None,
            error: Some(error),
            artifacts: Vec::new(),
            duration_ms: 0,
            completed_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Set output text
    pub fn with_output(mut self, output: impl Into<String>) -> Self {
        self.output = Some(output.into());
        self
    }

    /// Set structured data
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Set duration
    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = duration_ms;
        self
    }

    /// Add artifact
    pub fn with_artifact(mut self, artifact: ArtifactInfo) -> Self {
        self.artifacts.push(artifact);
        self
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Convert from MCP ToolCallResult
    pub fn from_mcp_result(
        request_id: impl Into<String>,
        result: &ToolCallResult,
        duration_ms: u64,
    ) -> Self {
        let request_id = request_id.into();

        if result.is_error {
            Self::failure(
                &request_id,
                ToolType::Mcp,
                HybridErrorInfo {
                    error_type: HybridErrorType::McpError,
                    message: result
                        .text_content()
                        .unwrap_or_else(|| "Unknown error".to_string()),
                    details: None,
                    source: Some("mcp".to_string()),
                },
            )
            .with_duration(duration_ms)
        } else {
            Self::success(&request_id, ToolType::Mcp)
                .with_output(result.text_content().unwrap_or_default())
                .with_duration(duration_ms)
        }
    }

    /// Convert from CodeAct result
    pub fn from_codeact_result(request_id: impl Into<String>, result: &CodeActResult) -> Self {
        let request_id = request_id.into();

        if result.is_success() {
            let mut exec_result = Self::success(&request_id, ToolType::CodeAct)
                .with_output(result.output.clone().unwrap_or_default())
                .with_duration(result.timing.total_ms);

            // Add return value as data if present
            if let Some(ref rv) = result.return_value {
                exec_result = exec_result.with_data(rv.clone());
            }

            // Add artifacts
            for artifact in &result.artifacts {
                exec_result.artifacts.push(ArtifactInfo {
                    name: artifact.name.clone(),
                    artifact_type: artifact.artifact_type.display_name().to_string(),
                    size_bytes: artifact.size(),
                    mime_type: artifact.mime_type().to_string(),
                });
            }

            exec_result
        } else {
            let error_info = if let Some(ref err) = result.error {
                HybridErrorInfo {
                    error_type: HybridErrorType::CodeActError,
                    message: err.message.clone(),
                    details: err.traceback.clone(),
                    source: Some("codeact".to_string()),
                }
            } else {
                HybridErrorInfo {
                    error_type: HybridErrorType::ExecutionFailed,
                    message: "Execution failed".to_string(),
                    details: None,
                    source: Some("codeact".to_string()),
                }
            };

            Self::failure(&request_id, ToolType::CodeAct, error_info)
                .with_duration(result.timing.total_ms)
        }
    }
}

/// Information about an artifact
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactInfo {
    /// Artifact name
    pub name: String,
    /// Artifact type
    pub artifact_type: String,
    /// Size in bytes
    pub size_bytes: usize,
    /// MIME type
    pub mime_type: String,
}

// ============================================================================
// Error Types
// ============================================================================

/// Error types specific to hybrid execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HybridErrorType {
    /// Invalid request format
    InvalidRequest,
    /// MCP tool execution error
    McpError,
    /// CodeAct execution error
    CodeActError,
    /// Tool not found
    ToolNotFound,
    /// Execution failed
    ExecutionFailed,
    /// Timeout
    Timeout,
    /// Permission denied
    PermissionDenied,
    /// Router not available
    RouterUnavailable,
    /// Internal error
    Internal,
}

impl std::fmt::Display for HybridErrorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HybridErrorType::InvalidRequest => write!(f, "invalid_request"),
            HybridErrorType::McpError => write!(f, "mcp_error"),
            HybridErrorType::CodeActError => write!(f, "codeact_error"),
            HybridErrorType::ToolNotFound => write!(f, "tool_not_found"),
            HybridErrorType::ExecutionFailed => write!(f, "execution_failed"),
            HybridErrorType::Timeout => write!(f, "timeout"),
            HybridErrorType::PermissionDenied => write!(f, "permission_denied"),
            HybridErrorType::RouterUnavailable => write!(f, "router_unavailable"),
            HybridErrorType::Internal => write!(f, "internal"),
        }
    }
}

/// Detailed error information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridErrorInfo {
    /// Error type classification
    pub error_type: HybridErrorType,
    /// Human-readable error message
    pub message: String,
    /// Additional details (e.g., traceback)
    pub details: Option<String>,
    /// Error source (mcp, codeact, hybrid)
    pub source: Option<String>,
}

impl HybridErrorInfo {
    /// Create a new error info
    pub fn new(error_type: HybridErrorType, message: impl Into<String>) -> Self {
        Self {
            error_type,
            message: message.into(),
            details: None,
            source: None,
        }
    }

    /// Set details
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Set source
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// Hybrid execution error
#[derive(Error, Debug)]
pub enum HybridError {
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("MCP error: {0}")]
    McpError(String),

    #[error("CodeAct error: {0}")]
    CodeActError(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Router unavailable: {0}")]
    RouterUnavailable(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl HybridError {
    /// Get the error type
    pub fn error_type(&self) -> HybridErrorType {
        match self {
            HybridError::InvalidRequest(_) => HybridErrorType::InvalidRequest,
            HybridError::McpError(_) => HybridErrorType::McpError,
            HybridError::CodeActError(_) => HybridErrorType::CodeActError,
            HybridError::ToolNotFound(_) => HybridErrorType::ToolNotFound,
            HybridError::ExecutionFailed(_) => HybridErrorType::ExecutionFailed,
            HybridError::Timeout(_) => HybridErrorType::Timeout,
            HybridError::PermissionDenied(_) => HybridErrorType::PermissionDenied,
            HybridError::RouterUnavailable(_) => HybridErrorType::RouterUnavailable,
            HybridError::Internal(_) => HybridErrorType::Internal,
        }
    }

    /// Convert to HybridErrorInfo
    pub fn to_error_info(&self) -> HybridErrorInfo {
        HybridErrorInfo {
            error_type: self.error_type(),
            message: self.to_string(),
            details: None,
            source: Some("hybrid".to_string()),
        }
    }
}

impl From<Error> for HybridError {
    fn from(err: Error) -> Self {
        match err {
            Error::NotFound(msg) => HybridError::ToolNotFound(msg),
            Error::PermissionDenied(msg) => HybridError::PermissionDenied(msg),
            Error::Timeout(msg) => HybridError::Timeout(msg),
            Error::ExecutionFailed(msg) => HybridError::ExecutionFailed(msg),
            Error::Mcp(msg) => HybridError::McpError(msg),
            _ => HybridError::Internal(err.to_string()),
        }
    }
}

// ============================================================================
// Tool Type Detector
// ============================================================================

/// Detects whether a request should be routed to MCP or CodeAct
#[derive(Debug, Clone, Default)]
pub struct ToolTypeDetector {
    /// Known MCP tool prefixes/namespaces
    mcp_namespaces: Vec<String>,
    /// Known CodeAct language identifiers
    codeact_languages: Vec<String>,
}

impl ToolTypeDetector {
    /// Create a new tool type detector
    pub fn new() -> Self {
        Self {
            mcp_namespaces: vec![
                "filesystem".to_string(),
                "executor".to_string(),
                "browser".to_string(),
                "git".to_string(),
                "network".to_string(),
            ],
            codeact_languages: vec![
                "python".to_string(),
                "javascript".to_string(),
                "typescript".to_string(),
                "bash".to_string(),
                "go".to_string(),
                "rust".to_string(),
            ],
        }
    }

    /// Add an MCP namespace
    pub fn add_mcp_namespace(&mut self, namespace: impl Into<String>) {
        self.mcp_namespaces.push(namespace.into());
    }

    /// Add a CodeAct language
    pub fn add_codeact_language(&mut self, language: impl Into<String>) {
        self.codeact_languages.push(language.into());
    }

    /// Detect the type of a request
    pub fn detect(&self, request: &ExecutionRequest) -> ToolType {
        // Already typed
        if request.request_type != ToolType::Unknown {
            return request.request_type;
        }

        // Check for MCP tool call pattern
        if let Some(ref tool_name) = request.tool_name {
            // MCP tools are typically namespace_toolname format
            if tool_name.contains('_') {
                let namespace = tool_name.split('_').next().unwrap_or("");
                if self.mcp_namespaces.iter().any(|ns| ns == namespace) {
                    return ToolType::Mcp;
                }
            }
            // Has tool_name and arguments - likely MCP
            if request.arguments.is_some() {
                return ToolType::Mcp;
            }
        }

        // Check for CodeAct code pattern
        if request.code.is_some() {
            // Check for browser automation
            if let Some(ref code) = request.code {
                let code_lower = code.to_lowercase();
                if code_lower.contains("playwright")
                    || code_lower.contains("selenium")
                    || code_lower.contains("page.goto")
                {
                    return ToolType::Browser;
                }
            }

            // Check language
            if let Some(ref lang) = request.language {
                let lang_lower = lang.to_lowercase();
                if self.codeact_languages.iter().any(|l| l == &lang_lower) {
                    return ToolType::CodeAct;
                }
            }

            // Has code, assume CodeAct
            return ToolType::CodeAct;
        }

        ToolType::Unknown
    }

    /// Check if a tool name is an MCP tool
    pub fn is_mcp_tool(&self, tool_name: &str) -> bool {
        if tool_name.contains('_') {
            let namespace = tool_name.split('_').next().unwrap_or("");
            self.mcp_namespaces.iter().any(|ns| ns == namespace)
        } else {
            false
        }
    }

    /// Check if a language is supported for CodeAct
    pub fn is_codeact_language(&self, language: &str) -> bool {
        let lang_lower = language.to_lowercase();
        self.codeact_languages.iter().any(|l| l == &lang_lower)
    }
}

// ============================================================================
// Hybrid Executor Trait
// ============================================================================

/// Trait for hybrid execution
#[async_trait]
pub trait HybridExecutor: Send + Sync {
    /// Execute a request (MCP or CodeAct)
    async fn execute(
        &self,
        request: ExecutionRequest,
    ) -> std::result::Result<ExecutionResult, HybridError>;

    /// Execute multiple requests in parallel
    async fn execute_batch(
        &self,
        requests: Vec<ExecutionRequest>,
    ) -> Vec<std::result::Result<ExecutionResult, HybridError>>;

    /// Check if the executor is available
    async fn is_available(&self) -> bool;

    /// Get executor status
    async fn status(&self) -> HybridExecutorStatus;
}

/// Status of the hybrid executor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridExecutorStatus {
    /// Whether MCP gateway is available
    pub mcp_available: bool,
    /// Whether CodeAct router is available
    pub codeact_available: bool,
    /// Number of registered MCP tools
    pub mcp_tool_count: usize,
    /// Supported CodeAct languages
    pub codeact_languages: Vec<String>,
    /// Total executions
    pub total_executions: u64,
    /// MCP executions
    pub mcp_executions: u64,
    /// CodeAct executions
    pub codeact_executions: u64,
    /// Failed executions
    pub failed_executions: u64,
}

// ============================================================================
// Hybrid Router Metrics
// ============================================================================

/// Metrics for the hybrid router
#[derive(Debug, Default)]
pub struct HybridMetrics {
    /// Total requests
    total_requests: AtomicU64,
    /// MCP requests
    mcp_requests: AtomicU64,
    /// CodeAct requests
    codeact_requests: AtomicU64,
    /// Browser requests
    browser_requests: AtomicU64,
    /// Failed requests
    failed_requests: AtomicU64,
    /// Average MCP latency (ms)
    mcp_latency_ms: AtomicU64,
    /// Average CodeAct latency (ms)
    codeact_latency_ms: AtomicU64,
}

impl HybridMetrics {
    /// Create new metrics
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an MCP request
    pub fn record_mcp(&self, latency_ms: u64, success: bool) {
        self.total_requests.fetch_add(1, Ordering::SeqCst);
        self.mcp_requests.fetch_add(1, Ordering::SeqCst);
        if !success {
            self.failed_requests.fetch_add(1, Ordering::SeqCst);
        }
        // Update average latency
        let current = self.mcp_latency_ms.load(Ordering::SeqCst);
        let new_latency = if current == 0 {
            latency_ms
        } else {
            (current + latency_ms) / 2
        };
        self.mcp_latency_ms.store(new_latency, Ordering::SeqCst);
    }

    /// Record a CodeAct request
    pub fn record_codeact(&self, latency_ms: u64, success: bool) {
        self.total_requests.fetch_add(1, Ordering::SeqCst);
        self.codeact_requests.fetch_add(1, Ordering::SeqCst);
        if !success {
            self.failed_requests.fetch_add(1, Ordering::SeqCst);
        }
        // Update average latency
        let current = self.codeact_latency_ms.load(Ordering::SeqCst);
        let new_latency = if current == 0 {
            latency_ms
        } else {
            (current + latency_ms) / 2
        };
        self.codeact_latency_ms.store(new_latency, Ordering::SeqCst);
    }

    /// Record a browser request
    pub fn record_browser(&self, latency_ms: u64, success: bool) {
        self.total_requests.fetch_add(1, Ordering::SeqCst);
        self.browser_requests.fetch_add(1, Ordering::SeqCst);
        if !success {
            self.failed_requests.fetch_add(1, Ordering::SeqCst);
        }
        // Browser uses CodeAct latency
        let current = self.codeact_latency_ms.load(Ordering::SeqCst);
        let new_latency = if current == 0 {
            latency_ms
        } else {
            (current + latency_ms) / 2
        };
        self.codeact_latency_ms.store(new_latency, Ordering::SeqCst);
    }

    /// Get total requests
    pub fn total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::SeqCst)
    }

    /// Get MCP requests
    pub fn mcp_requests(&self) -> u64 {
        self.mcp_requests.load(Ordering::SeqCst)
    }

    /// Get CodeAct requests
    pub fn codeact_requests(&self) -> u64 {
        self.codeact_requests.load(Ordering::SeqCst)
    }

    /// Get browser requests
    pub fn browser_requests(&self) -> u64 {
        self.browser_requests.load(Ordering::SeqCst)
    }

    /// Get failed requests
    pub fn failed_requests(&self) -> u64 {
        self.failed_requests.load(Ordering::SeqCst)
    }

    /// Get MCP average latency
    pub fn mcp_latency_ms(&self) -> u64 {
        self.mcp_latency_ms.load(Ordering::SeqCst)
    }

    /// Get CodeAct average latency
    pub fn codeact_latency_ms(&self) -> u64 {
        self.codeact_latency_ms.load(Ordering::SeqCst)
    }
}

// ============================================================================
// Hybrid Router
// ============================================================================

/// Configuration for the hybrid router
#[derive(Debug, Clone)]
pub struct HybridRouterConfig {
    /// Default timeout in milliseconds
    pub default_timeout_ms: u64,
    /// Maximum parallel executions
    pub max_parallel_executions: usize,
    /// Whether to auto-detect request types
    pub auto_detect_type: bool,
    /// Whether to enable MCP routing
    pub enable_mcp: bool,
    /// Whether to enable CodeAct routing
    pub enable_codeact: bool,
}

impl Default for HybridRouterConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 30000,
            max_parallel_executions: 10,
            auto_detect_type: true,
            enable_mcp: true,
            enable_codeact: true,
        }
    }
}

/// The main hybrid router that routes requests to MCP or CodeAct
pub struct HybridRouter {
    /// MCP gateway
    mcp_gateway: Option<Arc<McpGateway>>,
    /// CodeAct router
    codeact_router: Option<Arc<UnifiedCodeActRouter>>,
    /// Tool type detector
    detector: Arc<ToolTypeDetector>,
    /// Configuration
    config: HybridRouterConfig,
    /// Metrics
    metrics: Arc<HybridMetrics>,
}

impl HybridRouter {
    /// Create a new hybrid router
    pub fn new(
        mcp_gateway: Option<Arc<McpGateway>>,
        codeact_router: Option<Arc<UnifiedCodeActRouter>>,
        config: HybridRouterConfig,
    ) -> Self {
        Self {
            mcp_gateway,
            codeact_router,
            detector: Arc::new(ToolTypeDetector::new()),
            config,
            metrics: Arc::new(HybridMetrics::new()),
        }
    }

    /// Create a builder
    pub fn builder() -> HybridRouterBuilder {
        HybridRouterBuilder::new()
    }

    /// Get reference to MCP gateway
    pub fn mcp_gateway(&self) -> Option<&Arc<McpGateway>> {
        self.mcp_gateway.as_ref()
    }

    /// Get reference to CodeAct router
    pub fn codeact_router(&self) -> Option<&Arc<UnifiedCodeActRouter>> {
        self.codeact_router.as_ref()
    }

    /// Get reference to metrics
    pub fn metrics(&self) -> &Arc<HybridMetrics> {
        &self.metrics
    }

    /// Get reference to detector
    pub fn detector(&self) -> &Arc<ToolTypeDetector> {
        &self.detector
    }

    /// Execute an MCP tool call
    #[instrument(skip(self, tool_name, arguments))]
    async fn execute_mcp(
        &self,
        request_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> std::result::Result<ExecutionResult, HybridError> {
        let gateway = self.mcp_gateway.as_ref().ok_or_else(|| {
            HybridError::RouterUnavailable("MCP gateway not configured".to_string())
        })?;

        if !self.config.enable_mcp {
            return Err(HybridError::RouterUnavailable(
                "MCP routing is disabled".to_string(),
            ));
        }

        debug!(tool_name = %tool_name, "Executing MCP tool call");

        let start = Instant::now();
        let result = gateway.execute_llm_tool_call(tool_name, arguments).await;
        let latency_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(tool_result) => {
                let success = !tool_result.is_error;
                self.metrics.record_mcp(latency_ms, success);

                Ok(ExecutionResult::from_mcp_result(
                    request_id,
                    &tool_result,
                    latency_ms,
                ))
            }
            Err(e) => {
                self.metrics.record_mcp(latency_ms, false);

                // Check for permission requests
                if let Some(_perm_request) = McpGateway::parse_permission_request(&e) {
                    return Err(HybridError::PermissionDenied(e.to_string()));
                }

                Err(HybridError::from(e))
            }
        }
    }

    /// Execute CodeAct code
    #[instrument(skip(self, code))]
    async fn execute_codeact(
        &self,
        request_id: &str,
        code: &str,
        language: &str,
        timeout_ms: u64,
        session_id: Option<String>,
    ) -> std::result::Result<ExecutionResult, HybridError> {
        let router = self.codeact_router.as_ref().ok_or_else(|| {
            HybridError::RouterUnavailable("CodeAct router not configured".to_string())
        })?;

        if !self.config.enable_codeact {
            return Err(HybridError::RouterUnavailable(
                "CodeAct routing is disabled".to_string(),
            ));
        }

        debug!(language = %language, code_len = code.len(), "Executing CodeAct code");

        let code_request = CodeExecutionRequest::new(code, language).with_timeout(timeout_ms);

        let code_request = if let Some(sid) = session_id {
            code_request.with_session(sid)
        } else {
            code_request
        };

        let start = Instant::now();
        let result = router.execute(code_request).await;
        let latency_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(codeact_result) => {
                let success = codeact_result.is_success();
                self.metrics.record_codeact(latency_ms, success);

                Ok(ExecutionResult::from_codeact_result(
                    request_id,
                    &codeact_result,
                ))
            }
            Err(e) => {
                self.metrics.record_codeact(latency_ms, false);
                Err(HybridError::from(crate::error::Error::from(e)))
            }
        }
    }

    /// Execute browser automation code
    #[instrument(skip(self, code))]
    async fn execute_browser(
        &self,
        request_id: &str,
        code: &str,
        timeout_ms: u64,
        session_id: Option<String>,
    ) -> std::result::Result<ExecutionResult, HybridError> {
        // Browser automation uses CodeAct with Python + browser libraries
        let router = self.codeact_router.as_ref().ok_or_else(|| {
            HybridError::RouterUnavailable("CodeAct router not configured for browser".to_string())
        })?;

        debug!(code_len = code.len(), "Executing browser automation");

        let code_request = CodeExecutionRequest::new(code, "python").with_timeout(timeout_ms);

        let code_request = if let Some(sid) = session_id {
            code_request.with_session(sid)
        } else {
            code_request
        };

        let start = Instant::now();
        let result = router.execute(code_request).await;
        let latency_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(codeact_result) => {
                let success = codeact_result.is_success();
                self.metrics.record_browser(latency_ms, success);

                let mut exec_result =
                    ExecutionResult::from_codeact_result(request_id, &codeact_result);
                exec_result.execution_type = ToolType::Browser;
                Ok(exec_result)
            }
            Err(e) => {
                self.metrics.record_browser(latency_ms, false);
                Err(HybridError::from(crate::error::Error::from(e)))
            }
        }
    }
}

#[async_trait]
impl HybridExecutor for HybridRouter {
    #[instrument(skip(self, request), fields(request_id = %request.id))]
    async fn execute(
        &self,
        mut request: ExecutionRequest,
    ) -> std::result::Result<ExecutionResult, HybridError> {
        // Auto-detect type if needed
        if self.config.auto_detect_type && request.request_type == ToolType::Unknown {
            request.request_type = self.detector.detect(&request);
        }

        // Validate
        request.validate()?;

        info!(
            request_id = %request.id,
            request_type = %request.request_type,
            "Executing hybrid request"
        );

        match request.request_type {
            ToolType::Mcp => {
                let tool_name = request.tool_name.as_ref().unwrap();
                let arguments = request.arguments.clone().unwrap_or(serde_json::json!({}));
                self.execute_mcp(&request.id, tool_name, arguments).await
            }
            ToolType::CodeAct => {
                let code = request.code.as_ref().unwrap();
                let language = request.language.as_deref().unwrap_or("python");
                self.execute_codeact(
                    &request.id,
                    code,
                    language,
                    request.timeout_ms,
                    request.session_id.clone(),
                )
                .await
            }
            ToolType::Browser => {
                let code = request.code.as_ref().unwrap();
                self.execute_browser(
                    &request.id,
                    code,
                    request.timeout_ms,
                    request.session_id.clone(),
                )
                .await
            }
            ToolType::Unknown => Err(HybridError::InvalidRequest(
                "Could not determine request type".to_string(),
            )),
        }
    }

    async fn execute_batch(
        &self,
        requests: Vec<ExecutionRequest>,
    ) -> Vec<std::result::Result<ExecutionResult, HybridError>> {
        if requests.is_empty() {
            return Vec::new();
        }

        let max_parallel = self
            .config
            .max_parallel_executions
            .min(requests.len())
            .max(1);
        let mut results = Vec::with_capacity(requests.len());

        // Process in batches
        for chunk in requests.chunks(max_parallel) {
            let futures: Vec<_> = chunk.iter().map(|req| self.execute(req.clone())).collect();

            let batch_results = futures::future::join_all(futures).await;
            results.extend(batch_results);
        }

        results
    }

    async fn is_available(&self) -> bool {
        let mcp_ok = self.mcp_gateway.is_some() && self.config.enable_mcp;
        let codeact_ok = self.codeact_router.is_some() && self.config.enable_codeact;
        mcp_ok || codeact_ok
    }

    async fn status(&self) -> HybridExecutorStatus {
        let mcp_available = self.mcp_gateway.is_some() && self.config.enable_mcp;
        let codeact_available = self.codeact_router.is_some() && self.config.enable_codeact;

        let mcp_tool_count = if let Some(ref gw) = self.mcp_gateway {
            gw.get_tools().await.len()
        } else {
            0
        };

        HybridExecutorStatus {
            mcp_available,
            codeact_available,
            mcp_tool_count,
            codeact_languages: vec![
                "python".to_string(),
                "javascript".to_string(),
                "typescript".to_string(),
                "bash".to_string(),
                "go".to_string(),
                "rust".to_string(),
            ],
            total_executions: self.metrics.total_requests(),
            mcp_executions: self.metrics.mcp_requests(),
            codeact_executions: self.metrics.codeact_requests(),
            failed_executions: self.metrics.failed_requests(),
        }
    }
}

// ============================================================================
// Builder
// ============================================================================

/// Builder for HybridRouter
pub struct HybridRouterBuilder {
    mcp_gateway: Option<Arc<McpGateway>>,
    codeact_router: Option<Arc<UnifiedCodeActRouter>>,
    config: HybridRouterConfig,
    detector: Option<ToolTypeDetector>,
}

impl HybridRouterBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            mcp_gateway: None,
            codeact_router: None,
            config: HybridRouterConfig::default(),
            detector: None,
        }
    }

    /// Set the MCP gateway
    pub fn mcp_gateway(mut self, gateway: Arc<McpGateway>) -> Self {
        self.mcp_gateway = Some(gateway);
        self
    }

    /// Set the CodeAct router
    pub fn codeact_router(mut self, router: Arc<UnifiedCodeActRouter>) -> Self {
        self.codeact_router = Some(router);
        self
    }

    /// Set the configuration
    pub fn config(mut self, config: HybridRouterConfig) -> Self {
        self.config = config;
        self
    }

    /// Set default timeout
    pub fn default_timeout(mut self, timeout_ms: u64) -> Self {
        self.config.default_timeout_ms = timeout_ms;
        self
    }

    /// Set max parallel executions
    pub fn max_parallel(mut self, max: usize) -> Self {
        self.config.max_parallel_executions = max;
        self
    }

    /// Enable/disable auto type detection
    pub fn auto_detect(mut self, enabled: bool) -> Self {
        self.config.auto_detect_type = enabled;
        self
    }

    /// Enable/disable MCP
    pub fn enable_mcp(mut self, enabled: bool) -> Self {
        self.config.enable_mcp = enabled;
        self
    }

    /// Enable/disable CodeAct
    pub fn enable_codeact(mut self, enabled: bool) -> Self {
        self.config.enable_codeact = enabled;
        self
    }

    /// Set custom detector
    pub fn detector(mut self, detector: ToolTypeDetector) -> Self {
        self.detector = Some(detector);
        self
    }

    /// Build the router
    pub fn build(self) -> HybridRouter {
        let mut router = HybridRouter::new(self.mcp_gateway, self.codeact_router, self.config);
        if let Some(detector) = self.detector {
            router.detector = Arc::new(detector);
        }
        router
    }
}

impl Default for HybridRouterBuilder {
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

    // ========== ToolType Tests ==========

    #[test]
    fn test_tool_type_display() {
        assert_eq!(ToolType::Mcp.to_string(), "mcp");
        assert_eq!(ToolType::CodeAct.to_string(), "codeact");
        assert_eq!(ToolType::Browser.to_string(), "browser");
        assert_eq!(ToolType::Unknown.to_string(), "unknown");
    }

    #[test]
    fn test_tool_type_default() {
        assert_eq!(ToolType::default(), ToolType::Unknown);
    }

    #[test]
    fn test_tool_type_serialization() {
        let json = serde_json::to_string(&ToolType::Mcp).unwrap();
        assert_eq!(json, "\"mcp\"");

        let parsed: ToolType = serde_json::from_str("\"codeact\"").unwrap();
        assert_eq!(parsed, ToolType::CodeAct);
    }

    // ========== ExecutionRequest Tests ==========

    #[test]
    fn test_execution_request_new() {
        let request = ExecutionRequest::new();
        assert_eq!(request.request_type, ToolType::Unknown);
        assert!(request.tool_name.is_none());
        assert!(request.code.is_none());
    }

    #[test]
    fn test_execution_request_mcp_tool() {
        let request = ExecutionRequest::mcp_tool(
            "filesystem_read_file",
            serde_json::json!({"path": "/tmp/test"}),
        );
        assert_eq!(request.request_type, ToolType::Mcp);
        assert_eq!(request.tool_name, Some("filesystem_read_file".to_string()));
        assert!(request.arguments.is_some());
    }

    #[test]
    fn test_execution_request_code() {
        let request = ExecutionRequest::code("print('hello')", "python");
        assert_eq!(request.request_type, ToolType::CodeAct);
        assert_eq!(request.code, Some("print('hello')".to_string()));
        assert_eq!(request.language, Some("python".to_string()));
    }

    #[test]
    fn test_execution_request_browser() {
        let request = ExecutionRequest::browser("page.goto('http://example.com')");
        assert_eq!(request.request_type, ToolType::Browser);
        assert_eq!(request.timeout_ms, 60000); // Browser has longer timeout
    }

    #[test]
    fn test_execution_request_with_timeout() {
        let request = ExecutionRequest::code("test", "python").with_timeout(5000);
        assert_eq!(request.timeout_ms, 5000);
    }

    #[test]
    fn test_execution_request_with_session() {
        let request = ExecutionRequest::code("test", "python").with_session("session-123");
        assert_eq!(request.session_id, Some("session-123".to_string()));
    }

    #[test]
    fn test_execution_request_with_metadata() {
        let request =
            ExecutionRequest::code("test", "python").with_metadata("key", serde_json::json!(42));
        assert_eq!(request.metadata.get("key"), Some(&serde_json::json!(42)));
    }

    #[test]
    fn test_execution_request_detect_type_mcp() {
        let mut request = ExecutionRequest::new();
        request.tool_name = Some("filesystem_read".to_string());
        request.arguments = Some(serde_json::json!({}));
        request.detect_type();
        assert_eq!(request.request_type, ToolType::Mcp);
    }

    #[test]
    fn test_execution_request_detect_type_codeact() {
        let mut request = ExecutionRequest::new();
        request.code = Some("print('hello')".to_string());
        request.detect_type();
        assert_eq!(request.request_type, ToolType::CodeAct);
    }

    #[test]
    fn test_execution_request_detect_type_browser() {
        let mut request = ExecutionRequest::new();
        request.code = Some("playwright.chromium.launch()".to_string());
        request.detect_type();
        assert_eq!(request.request_type, ToolType::Browser);
    }

    #[test]
    fn test_execution_request_validate_mcp() {
        let request = ExecutionRequest::mcp_tool("test_tool", serde_json::json!({}));
        assert!(request.validate().is_ok());

        let mut bad_request = ExecutionRequest::new();
        bad_request.request_type = ToolType::Mcp;
        assert!(bad_request.validate().is_err());
    }

    #[test]
    fn test_execution_request_validate_codeact() {
        let request = ExecutionRequest::code("test", "python");
        assert!(request.validate().is_ok());

        let mut bad_request = ExecutionRequest::new();
        bad_request.request_type = ToolType::CodeAct;
        assert!(bad_request.validate().is_err());
    }

    #[test]
    fn test_execution_request_validate_unknown() {
        let request = ExecutionRequest::new();
        assert!(request.validate().is_err());
    }

    // ========== ExecutionResult Tests ==========

    #[test]
    fn test_execution_result_success() {
        let result = ExecutionResult::success("req-1", ToolType::Mcp);
        assert!(result.success);
        assert_eq!(result.request_id, "req-1");
        assert_eq!(result.execution_type, ToolType::Mcp);
    }

    #[test]
    fn test_execution_result_failure() {
        let error = HybridErrorInfo::new(HybridErrorType::McpError, "Test error");
        let result = ExecutionResult::failure("req-1", ToolType::Mcp, error);
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_execution_result_with_output() {
        let result = ExecutionResult::success("req-1", ToolType::CodeAct).with_output("Hello!");
        assert_eq!(result.output, Some("Hello!".to_string()));
    }

    #[test]
    fn test_execution_result_with_data() {
        let result =
            ExecutionResult::success("req-1", ToolType::CodeAct).with_data(serde_json::json!(42));
        assert_eq!(result.data, Some(serde_json::json!(42)));
    }

    #[test]
    fn test_execution_result_with_artifact() {
        let artifact = ArtifactInfo {
            name: "test.png".to_string(),
            artifact_type: "screenshot".to_string(),
            size_bytes: 100,
            mime_type: "image/png".to_string(),
        };
        let result = ExecutionResult::success("req-1", ToolType::CodeAct).with_artifact(artifact);
        assert_eq!(result.artifacts.len(), 1);
    }

    #[test]
    fn test_execution_result_from_mcp_success() {
        let tool_result = ToolCallResult::text("Success output");
        let result = ExecutionResult::from_mcp_result("req-1", &tool_result, 100);
        assert!(result.success);
        assert_eq!(result.output, Some("Success output".to_string()));
        assert_eq!(result.duration_ms, 100);
    }

    #[test]
    fn test_execution_result_from_mcp_error() {
        let tool_result = ToolCallResult::error("Error message");
        let result = ExecutionResult::from_mcp_result("req-1", &tool_result, 50);
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    // ========== HybridErrorInfo Tests ==========

    #[test]
    fn test_hybrid_error_info_new() {
        let info = HybridErrorInfo::new(HybridErrorType::McpError, "Test error");
        assert_eq!(info.error_type, HybridErrorType::McpError);
        assert_eq!(info.message, "Test error");
    }

    #[test]
    fn test_hybrid_error_info_with_details() {
        let info =
            HybridErrorInfo::new(HybridErrorType::CodeActError, "Error").with_details("Traceback");
        assert_eq!(info.details, Some("Traceback".to_string()));
    }

    #[test]
    fn test_hybrid_error_info_with_source() {
        let info = HybridErrorInfo::new(HybridErrorType::Internal, "Error").with_source("hybrid");
        assert_eq!(info.source, Some("hybrid".to_string()));
    }

    // ========== HybridError Tests ==========

    #[test]
    fn test_hybrid_error_types() {
        let errors = vec![
            HybridError::InvalidRequest("test".to_string()),
            HybridError::McpError("test".to_string()),
            HybridError::CodeActError("test".to_string()),
            HybridError::ToolNotFound("test".to_string()),
            HybridError::Timeout("test".to_string()),
            HybridError::PermissionDenied("test".to_string()),
        ];

        let expected_types = vec![
            HybridErrorType::InvalidRequest,
            HybridErrorType::McpError,
            HybridErrorType::CodeActError,
            HybridErrorType::ToolNotFound,
            HybridErrorType::Timeout,
            HybridErrorType::PermissionDenied,
        ];

        for (error, expected) in errors.iter().zip(expected_types.iter()) {
            assert_eq!(error.error_type(), *expected);
        }
    }

    #[test]
    fn test_hybrid_error_to_error_info() {
        let error = HybridError::McpError("MCP failed".to_string());
        let info = error.to_error_info();
        assert_eq!(info.error_type, HybridErrorType::McpError);
        assert!(info.message.contains("MCP failed"));
    }

    #[test]
    fn test_hybrid_error_from_core_error() {
        let core_error = Error::NotFound("Not found".to_string());
        let hybrid_error: HybridError = core_error.into();
        assert!(matches!(hybrid_error, HybridError::ToolNotFound(_)));

        let core_error = Error::PermissionDenied("Denied".to_string());
        let hybrid_error: HybridError = core_error.into();
        assert!(matches!(hybrid_error, HybridError::PermissionDenied(_)));
    }

    // ========== ToolTypeDetector Tests ==========

    #[test]
    fn test_detector_new() {
        let detector = ToolTypeDetector::new();
        assert!(!detector.mcp_namespaces.is_empty());
        assert!(!detector.codeact_languages.is_empty());
    }

    #[test]
    fn test_detector_add_namespace() {
        let mut detector = ToolTypeDetector::new();
        detector.add_mcp_namespace("custom");
        assert!(detector.mcp_namespaces.contains(&"custom".to_string()));
    }

    #[test]
    fn test_detector_add_language() {
        let mut detector = ToolTypeDetector::new();
        detector.add_codeact_language("ruby");
        assert!(detector.codeact_languages.contains(&"ruby".to_string()));
    }

    #[test]
    fn test_detector_detect_mcp() {
        let detector = ToolTypeDetector::new();
        let request = ExecutionRequest::mcp_tool("filesystem_read", serde_json::json!({}));
        assert_eq!(detector.detect(&request), ToolType::Mcp);
    }

    #[test]
    fn test_detector_detect_codeact() {
        let detector = ToolTypeDetector::new();
        let request = ExecutionRequest::code("print('hello')", "python");
        assert_eq!(detector.detect(&request), ToolType::CodeAct);
    }

    #[test]
    fn test_detector_detect_browser() {
        let detector = ToolTypeDetector::new();

        let mut request = ExecutionRequest::new();
        request.code = Some("from playwright.sync_api import sync_playwright".to_string());
        assert_eq!(detector.detect(&request), ToolType::Browser);
    }

    #[test]
    fn test_detector_is_mcp_tool() {
        let detector = ToolTypeDetector::new();
        assert!(detector.is_mcp_tool("filesystem_read"));
        assert!(detector.is_mcp_tool("executor_bash"));
        assert!(!detector.is_mcp_tool("unknown_tool"));
        assert!(!detector.is_mcp_tool("notool"));
    }

    #[test]
    fn test_detector_is_codeact_language() {
        let detector = ToolTypeDetector::new();
        assert!(detector.is_codeact_language("python"));
        assert!(detector.is_codeact_language("Python"));
        assert!(detector.is_codeact_language("javascript"));
        assert!(!detector.is_codeact_language("cobol"));
    }

    // ========== HybridMetrics Tests ==========

    #[test]
    fn test_metrics_new() {
        let metrics = HybridMetrics::new();
        assert_eq!(metrics.total_requests(), 0);
        assert_eq!(metrics.mcp_requests(), 0);
        assert_eq!(metrics.codeact_requests(), 0);
    }

    #[test]
    fn test_metrics_record_mcp() {
        let metrics = HybridMetrics::new();
        metrics.record_mcp(100, true);
        assert_eq!(metrics.total_requests(), 1);
        assert_eq!(metrics.mcp_requests(), 1);
        assert_eq!(metrics.failed_requests(), 0);
        assert_eq!(metrics.mcp_latency_ms(), 100);
    }

    #[test]
    fn test_metrics_record_mcp_failure() {
        let metrics = HybridMetrics::new();
        metrics.record_mcp(100, false);
        assert_eq!(metrics.total_requests(), 1);
        assert_eq!(metrics.failed_requests(), 1);
    }

    #[test]
    fn test_metrics_record_codeact() {
        let metrics = HybridMetrics::new();
        metrics.record_codeact(200, true);
        assert_eq!(metrics.total_requests(), 1);
        assert_eq!(metrics.codeact_requests(), 1);
        assert_eq!(metrics.codeact_latency_ms(), 200);
    }

    #[test]
    fn test_metrics_record_browser() {
        let metrics = HybridMetrics::new();
        metrics.record_browser(500, true);
        assert_eq!(metrics.total_requests(), 1);
        assert_eq!(metrics.browser_requests(), 1);
    }

    #[test]
    fn test_metrics_latency_averaging() {
        let metrics = HybridMetrics::new();
        metrics.record_mcp(100, true);
        metrics.record_mcp(200, true);
        // Average should be (100 + 200) / 2 = 150
        assert_eq!(metrics.mcp_latency_ms(), 150);
    }

    // ========== HybridRouterConfig Tests ==========

    #[test]
    fn test_config_default() {
        let config = HybridRouterConfig::default();
        assert_eq!(config.default_timeout_ms, 30000);
        assert_eq!(config.max_parallel_executions, 10);
        assert!(config.auto_detect_type);
        assert!(config.enable_mcp);
        assert!(config.enable_codeact);
    }

    // ========== HybridRouterBuilder Tests ==========

    #[test]
    fn test_builder_new() {
        let builder = HybridRouterBuilder::new();
        assert!(builder.mcp_gateway.is_none());
        assert!(builder.codeact_router.is_none());
    }

    #[test]
    fn test_builder_config_methods() {
        let builder = HybridRouterBuilder::new()
            .default_timeout(5000)
            .max_parallel(20)
            .auto_detect(false)
            .enable_mcp(false)
            .enable_codeact(true);

        assert_eq!(builder.config.default_timeout_ms, 5000);
        assert_eq!(builder.config.max_parallel_executions, 20);
        assert!(!builder.config.auto_detect_type);
        assert!(!builder.config.enable_mcp);
        assert!(builder.config.enable_codeact);
    }

    #[test]
    fn test_builder_build_without_gateways() {
        let router = HybridRouterBuilder::new().build();
        assert!(router.mcp_gateway.is_none());
        assert!(router.codeact_router.is_none());
    }

    #[test]
    fn test_builder_with_detector() {
        let mut detector = ToolTypeDetector::new();
        detector.add_mcp_namespace("custom");

        let router = HybridRouterBuilder::new().detector(detector).build();

        assert!(router
            .detector
            .mcp_namespaces
            .contains(&"custom".to_string()));
    }

    // ========== HybridRouter Tests (without actual gateways) ==========

    #[tokio::test]
    async fn test_router_is_available_empty() {
        let router = HybridRouterBuilder::new().build();
        assert!(!router.is_available().await);
    }

    #[tokio::test]
    async fn test_router_status_empty() {
        let router = HybridRouterBuilder::new().build();
        let status = router.status().await;
        assert!(!status.mcp_available);
        assert!(!status.codeact_available);
        assert_eq!(status.mcp_tool_count, 0);
    }

    #[tokio::test]
    async fn test_router_execute_no_mcp() {
        let router = HybridRouterBuilder::new().build();
        let request = ExecutionRequest::mcp_tool("test_tool", serde_json::json!({}));
        let result = router.execute(request).await;
        assert!(result.is_err());
        assert!(matches!(result, Err(HybridError::RouterUnavailable(_))));
    }

    #[tokio::test]
    async fn test_router_execute_no_codeact() {
        let router = HybridRouterBuilder::new().build();
        let request = ExecutionRequest::code("print('hello')", "python");
        let result = router.execute(request).await;
        assert!(result.is_err());
        assert!(matches!(result, Err(HybridError::RouterUnavailable(_))));
    }

    #[tokio::test]
    async fn test_router_execute_mcp_disabled() {
        let router = HybridRouterBuilder::new().enable_mcp(false).build();
        let request = ExecutionRequest::mcp_tool("test_tool", serde_json::json!({}));
        let result = router.execute(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_router_execute_codeact_disabled() {
        let router = HybridRouterBuilder::new().enable_codeact(false).build();
        let request = ExecutionRequest::code("test", "python");
        let result = router.execute(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_router_execute_unknown_type() {
        let router = HybridRouterBuilder::new().auto_detect(false).build();
        let request = ExecutionRequest::new(); // Unknown type
        let result = router.execute(request).await;
        assert!(result.is_err());
        assert!(matches!(result, Err(HybridError::InvalidRequest(_))));
    }

    #[tokio::test]
    async fn test_router_batch_empty() {
        let router = HybridRouterBuilder::new().build();
        let results = router.execute_batch(vec![]).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_router_batch_all_fail() {
        let router = HybridRouterBuilder::new().build();
        let requests = vec![
            ExecutionRequest::mcp_tool("tool1", serde_json::json!({})),
            ExecutionRequest::code("test", "python"),
        ];
        let results = router.execute_batch(requests).await;
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.is_err()));
    }

    // ========== Integration-like Tests ==========

    #[test]
    fn test_full_request_flow() {
        // Test complete request creation and validation
        let mcp_request = ExecutionRequest::mcp_tool(
            "filesystem_read_file",
            serde_json::json!({"path": "/tmp/test"}),
        )
        .with_timeout(5000)
        .with_session("sess-1")
        .with_metadata("user", serde_json::json!("test"));

        assert!(mcp_request.validate().is_ok());
        assert_eq!(mcp_request.request_type, ToolType::Mcp);
        assert_eq!(mcp_request.timeout_ms, 5000);
        assert_eq!(mcp_request.session_id, Some("sess-1".to_string()));

        let code_request = ExecutionRequest::code("print('hello')", "python").with_timeout(10000);

        assert!(code_request.validate().is_ok());
        assert_eq!(code_request.request_type, ToolType::CodeAct);
    }

    #[test]
    fn test_result_creation_flow() {
        // Test complete result creation
        let artifact = ArtifactInfo {
            name: "screenshot.png".to_string(),
            artifact_type: "screenshot".to_string(),
            size_bytes: 1024,
            mime_type: "image/png".to_string(),
        };

        let result = ExecutionResult::success("req-123", ToolType::CodeAct)
            .with_output("Hello, World!")
            .with_data(serde_json::json!({"result": 42}))
            .with_duration(150)
            .with_artifact(artifact)
            .with_metadata("executor", serde_json::json!("local"));

        assert!(result.success);
        assert_eq!(result.output, Some("Hello, World!".to_string()));
        assert_eq!(result.data, Some(serde_json::json!({"result": 42})));
        assert_eq!(result.duration_ms, 150);
        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(result.artifacts[0].name, "screenshot.png");
    }

    #[test]
    fn test_error_conversion_chain() {
        // Test error conversion from core error to hybrid error to error info
        let core_error = Error::Timeout("Execution timed out after 30s".to_string());
        let hybrid_error: HybridError = core_error.into();
        let error_info = hybrid_error.to_error_info();

        assert_eq!(error_info.error_type, HybridErrorType::Timeout);
        assert!(error_info.message.contains("Execution timed out"));
    }
}
