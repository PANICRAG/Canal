//! CodeAct Execution Result Parser
//!
//! This module provides comprehensive result parsing and handling for CodeAct
//! code execution. It includes result type definitions, output parsing,
//! artifact extraction, caching, and agent-friendly formatting.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                      ResultParser                                │
//! │  ┌──────────────────┐  ┌────────────────┐  ┌────────────────┐  │
//! │  │  OutputParser    │  │ ArtifactHandler │  │ ErrorParser   │  │
//! │  └──────────────────┘  └────────────────┘  └────────────────┘  │
//! │           │                    │                   │            │
//! │           └────────────────────┼───────────────────┘            │
//! │                                │                                │
//! │                     ┌──────────┴──────────┐                     │
//! │                     │   CodeActResult     │                     │
//! │                     └─────────────────────┘                     │
//! │                                │                                │
//! │                     ┌──────────┴──────────┐                     │
//! │                     │    ResultCache      │                     │
//! │                     └─────────────────────┘                     │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Features
//!
//! - Comprehensive result type with status, output, artifacts, and timing
//! - Exception/traceback parsing with structured error information
//! - Binary artifact handling (screenshots, files, DataFrames)
//! - Result caching with TTL and size limits
//! - Agent-friendly output formatting (Markdown, code blocks, tables)

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, instrument, warn};

use crate::error::{ServiceError as Error, ServiceResult as Result};

// ============================================================================
// Core Result Types
// ============================================================================

/// Comprehensive result from CodeAct code execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeActResult {
    /// Unique execution identifier
    pub execution_id: String,
    /// Execution status
    pub status: ExecutionStatus,
    /// Standard output (processed)
    pub output: Option<String>,
    /// Error information (if failed)
    pub error: Option<CodeActError>,
    /// Collected artifacts (screenshots, files, etc.)
    pub artifacts: Vec<Artifact>,
    /// Captured variables from execution
    pub variables: HashMap<String, serde_json::Value>,
    /// Execution timing information
    pub timing: ExecutionTiming,
    /// Timestamp when execution started
    pub started_at: DateTime<Utc>,
    /// Timestamp when execution completed
    pub completed_at: Option<DateTime<Utc>>,
    /// Raw stdout (unprocessed)
    pub raw_stdout: String,
    /// Raw stderr (unprocessed)
    pub raw_stderr: String,
    /// Exit code from execution
    pub exit_code: i32,
    /// Return value (if captured)
    pub return_value: Option<serde_json::Value>,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

impl CodeActResult {
    /// Create a new result for a starting execution
    pub fn new(execution_id: impl Into<String>) -> Self {
        Self {
            execution_id: execution_id.into(),
            status: ExecutionStatus::Success,
            output: None,
            error: None,
            artifacts: Vec::new(),
            variables: HashMap::new(),
            timing: ExecutionTiming::default(),
            started_at: Utc::now(),
            completed_at: None,
            raw_stdout: String::new(),
            raw_stderr: String::new(),
            exit_code: 0,
            return_value: None,
            metadata: HashMap::new(),
        }
    }

    /// Create a successful result
    pub fn success(execution_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            execution_id: execution_id.into(),
            status: ExecutionStatus::Success,
            output: Some(output.into()),
            error: None,
            artifacts: Vec::new(),
            variables: HashMap::new(),
            timing: ExecutionTiming::default(),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            raw_stdout: String::new(),
            raw_stderr: String::new(),
            exit_code: 0,
            return_value: None,
            metadata: HashMap::new(),
        }
    }

    /// Create a failed result
    pub fn error(execution_id: impl Into<String>, error: CodeActError) -> Self {
        Self {
            execution_id: execution_id.into(),
            status: ExecutionStatus::Error,
            output: None,
            error: Some(error),
            artifacts: Vec::new(),
            variables: HashMap::new(),
            timing: ExecutionTiming::default(),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            raw_stdout: String::new(),
            raw_stderr: String::new(),
            exit_code: 1,
            return_value: None,
            metadata: HashMap::new(),
        }
    }

    /// Create a timeout result
    pub fn timeout(execution_id: impl Into<String>, timeout_ms: u64) -> Self {
        Self {
            execution_id: execution_id.into(),
            status: ExecutionStatus::Timeout,
            output: None,
            error: Some(CodeActError {
                error_type: ErrorType::Timeout,
                message: format!("Execution timed out after {}ms", timeout_ms),
                details: None,
                traceback: None,
                line_number: None,
                column: None,
                code_snippet: None,
            }),
            artifacts: Vec::new(),
            variables: HashMap::new(),
            timing: ExecutionTiming {
                total_ms: timeout_ms,
                execution_ms: timeout_ms,
                ..Default::default()
            },
            started_at: Utc::now() - chrono::Duration::milliseconds(timeout_ms as i64),
            completed_at: Some(Utc::now()),
            raw_stdout: String::new(),
            raw_stderr: String::new(),
            exit_code: 124,
            return_value: None,
            metadata: HashMap::new(),
        }
    }

    /// Create a cancelled result
    pub fn cancelled(execution_id: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            execution_id: execution_id.into(),
            status: ExecutionStatus::Cancelled,
            output: None,
            error: Some(CodeActError {
                error_type: ErrorType::Cancelled,
                message: "Execution was cancelled".to_string(),
                details: None,
                traceback: None,
                line_number: None,
                column: None,
                code_snippet: None,
            }),
            artifacts: Vec::new(),
            variables: HashMap::new(),
            timing: ExecutionTiming {
                total_ms: duration_ms,
                execution_ms: duration_ms,
                ..Default::default()
            },
            started_at: Utc::now() - chrono::Duration::milliseconds(duration_ms as i64),
            completed_at: Some(Utc::now()),
            raw_stdout: String::new(),
            raw_stderr: String::new(),
            exit_code: 130,
            return_value: None,
            metadata: HashMap::new(),
        }
    }

    /// Check if execution was successful
    pub fn is_success(&self) -> bool {
        matches!(self.status, ExecutionStatus::Success)
    }

    /// Check if execution had an error
    pub fn is_error(&self) -> bool {
        matches!(self.status, ExecutionStatus::Error)
    }

    /// Check if execution timed out
    pub fn is_timeout(&self) -> bool {
        matches!(self.status, ExecutionStatus::Timeout)
    }

    /// Check if execution was cancelled
    pub fn is_cancelled(&self) -> bool {
        matches!(self.status, ExecutionStatus::Cancelled)
    }

    /// Add an artifact
    pub fn add_artifact(&mut self, artifact: Artifact) {
        self.artifacts.push(artifact);
    }

    /// Add a variable
    pub fn add_variable(&mut self, name: impl Into<String>, value: serde_json::Value) {
        self.variables.insert(name.into(), value);
    }

    /// Set the return value
    pub fn set_return_value(&mut self, value: serde_json::Value) {
        self.return_value = Some(value);
    }

    /// Get total artifact size in bytes
    pub fn total_artifact_size(&self) -> usize {
        self.artifacts.iter().map(|a| a.data.len()).sum()
    }
}

/// Execution status enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionStatus {
    /// Execution completed successfully
    Success,
    /// Execution failed with error
    Error,
    /// Execution timed out
    Timeout,
    /// Execution was cancelled
    Cancelled,
}

impl ExecutionStatus {
    /// Convert to human-readable string
    pub fn as_str(&self) -> &'static str {
        match self {
            ExecutionStatus::Success => "success",
            ExecutionStatus::Error => "error",
            ExecutionStatus::Timeout => "timeout",
            ExecutionStatus::Cancelled => "cancelled",
        }
    }

    /// Check if this is a terminal failure state
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            ExecutionStatus::Error | ExecutionStatus::Timeout | ExecutionStatus::Cancelled
        )
    }
}

impl std::fmt::Display for ExecutionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// Error Types
// ============================================================================

/// Detailed error information from CodeAct execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeActError {
    /// Type of error
    pub error_type: ErrorType,
    /// Error message
    pub message: String,
    /// Additional error details
    pub details: Option<String>,
    /// Full traceback/stack trace
    pub traceback: Option<String>,
    /// Line number where error occurred
    pub line_number: Option<u32>,
    /// Column number where error occurred
    pub column: Option<u32>,
    /// Code snippet around the error
    pub code_snippet: Option<String>,
}

impl CodeActError {
    /// Create a new error
    pub fn new(error_type: ErrorType, message: impl Into<String>) -> Self {
        Self {
            error_type,
            message: message.into(),
            details: None,
            traceback: None,
            line_number: None,
            column: None,
            code_snippet: None,
        }
    }

    /// Set traceback
    pub fn with_traceback(mut self, traceback: impl Into<String>) -> Self {
        self.traceback = Some(traceback.into());
        self
    }

    /// Set line number
    pub fn with_line_number(mut self, line: u32) -> Self {
        self.line_number = Some(line);
        self
    }

    /// Set code snippet
    pub fn with_code_snippet(mut self, snippet: impl Into<String>) -> Self {
        self.code_snippet = Some(snippet.into());
        self
    }

    /// Format error for display
    pub fn format_display(&self) -> String {
        let mut output = format!("{}: {}", self.error_type, self.message);

        if let Some(line) = self.line_number {
            output.push_str(&format!(" (line {})", line));
        }

        if let Some(ref traceback) = self.traceback {
            output.push_str("\n\nTraceback:\n");
            output.push_str(traceback);
        }

        output
    }
}

impl std::fmt::Display for CodeActError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.format_display())
    }
}

impl std::error::Error for CodeActError {}

/// Error type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorType {
    /// Syntax error in code
    SyntaxError,
    /// Runtime execution error
    RuntimeError,
    /// Import/module error
    ImportError,
    /// Name/reference error
    NameError,
    /// Type error
    TypeError,
    /// Value error
    ValueError,
    /// Index/key error
    IndexError,
    /// Attribute error
    AttributeError,
    /// File/IO error
    IOError,
    /// Timeout error
    Timeout,
    /// Cancelled by user
    Cancelled,
    /// Memory limit exceeded
    MemoryError,
    /// Permission denied
    PermissionError,
    /// Unknown/other error
    Unknown,
}

impl ErrorType {
    /// Parse error type from Python exception name
    pub fn from_python_exception(name: &str) -> Self {
        match name {
            "SyntaxError" => ErrorType::SyntaxError,
            "IndentationError" | "TabError" => ErrorType::SyntaxError,
            "RuntimeError" | "RecursionError" | "SystemError" => ErrorType::RuntimeError,
            "ImportError" | "ModuleNotFoundError" => ErrorType::ImportError,
            "NameError" | "UnboundLocalError" => ErrorType::NameError,
            "TypeError" => ErrorType::TypeError,
            "ValueError" => ErrorType::ValueError,
            "IndexError" | "KeyError" => ErrorType::IndexError,
            "AttributeError" => ErrorType::AttributeError,
            "FileNotFoundError" | "IOError" | "OSError" => ErrorType::IOError,
            "TimeoutError" => ErrorType::Timeout,
            "MemoryError" => ErrorType::MemoryError,
            "PermissionError" => ErrorType::PermissionError,
            "KeyboardInterrupt" => ErrorType::Cancelled,
            _ => ErrorType::Unknown,
        }
    }
}

impl std::fmt::Display for ErrorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorType::SyntaxError => write!(f, "SyntaxError"),
            ErrorType::RuntimeError => write!(f, "RuntimeError"),
            ErrorType::ImportError => write!(f, "ImportError"),
            ErrorType::NameError => write!(f, "NameError"),
            ErrorType::TypeError => write!(f, "TypeError"),
            ErrorType::ValueError => write!(f, "ValueError"),
            ErrorType::IndexError => write!(f, "IndexError"),
            ErrorType::AttributeError => write!(f, "AttributeError"),
            ErrorType::IOError => write!(f, "IOError"),
            ErrorType::Timeout => write!(f, "TimeoutError"),
            ErrorType::Cancelled => write!(f, "Cancelled"),
            ErrorType::MemoryError => write!(f, "MemoryError"),
            ErrorType::PermissionError => write!(f, "PermissionError"),
            ErrorType::Unknown => write!(f, "Error"),
        }
    }
}

// ============================================================================
// Artifact Types
// ============================================================================

/// Artifact from code execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Artifact name/filename
    pub name: String,
    /// Artifact type category
    pub artifact_type: ArtifactType,
    /// Raw artifact data
    #[serde(with = "base64_serde")]
    pub data: Vec<u8>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

impl Artifact {
    /// Create a new artifact
    pub fn new(name: impl Into<String>, artifact_type: ArtifactType, data: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            artifact_type,
            data,
            metadata: HashMap::new(),
        }
    }

    /// Create a screenshot artifact
    pub fn screenshot(name: impl Into<String>, data: Vec<u8>) -> Self {
        Self::new(name, ArtifactType::Screenshot, data)
    }

    /// Create a file artifact
    pub fn file(name: impl Into<String>, data: Vec<u8>) -> Self {
        Self::new(name, ArtifactType::File, data)
    }

    /// Create a plot artifact
    pub fn plot(name: impl Into<String>, data: Vec<u8>) -> Self {
        Self::new(name, ArtifactType::Plot, data)
    }

    /// Create a DataFrame artifact
    pub fn dataframe(name: impl Into<String>, data: Vec<u8>) -> Self {
        Self::new(name, ArtifactType::DataFrame, data)
    }

    /// Add metadata
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Get data as base64 string
    pub fn data_base64(&self) -> String {
        BASE64.encode(&self.data)
    }

    /// Get data as UTF-8 string (if valid)
    pub fn data_as_string(&self) -> Option<String> {
        String::from_utf8(self.data.clone()).ok()
    }

    /// Get size in bytes
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// Get MIME type based on artifact type and metadata
    pub fn mime_type(&self) -> &str {
        if let Some(mime) = self.metadata.get("mime_type") {
            return mime;
        }

        match self.artifact_type {
            ArtifactType::Screenshot => "image/png",
            ArtifactType::Plot => "image/png",
            ArtifactType::DataFrame => "application/json",
            ArtifactType::File => {
                // Try to guess from extension
                if let Some(ext) = self.name.split('.').last() {
                    match ext.to_lowercase().as_str() {
                        "png" => "image/png",
                        "jpg" | "jpeg" => "image/jpeg",
                        "gif" => "image/gif",
                        "svg" => "image/svg+xml",
                        "pdf" => "application/pdf",
                        "json" => "application/json",
                        "csv" => "text/csv",
                        "txt" => "text/plain",
                        "html" => "text/html",
                        "xml" => "application/xml",
                        _ => "application/octet-stream",
                    }
                } else {
                    "application/octet-stream"
                }
            }
            ArtifactType::Other(_) => "application/octet-stream",
        }
    }
}

/// Artifact type category
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    /// Browser/screen screenshot
    Screenshot,
    /// Generated or modified file
    File,
    /// Matplotlib/Plotly plot
    Plot,
    /// Pandas DataFrame
    DataFrame,
    /// Other artifact type
    Other(String),
}

impl ArtifactType {
    /// Get display name
    pub fn display_name(&self) -> &str {
        match self {
            ArtifactType::Screenshot => "Screenshot",
            ArtifactType::File => "File",
            ArtifactType::Plot => "Plot",
            ArtifactType::DataFrame => "DataFrame",
            ArtifactType::Other(name) => name,
        }
    }
}

// ============================================================================
// Timing Information
// ============================================================================

/// Execution timing information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionTiming {
    /// Total wall clock time in milliseconds
    pub total_ms: u64,
    /// Time spent in queue (if applicable)
    pub queue_ms: u64,
    /// Time spent executing code
    pub execution_ms: u64,
    /// Time spent parsing output
    pub parsing_ms: u64,
    /// Time spent collecting artifacts
    pub artifact_collection_ms: u64,
}

impl ExecutionTiming {
    /// Create timing info with just total time
    pub fn from_total(total_ms: u64) -> Self {
        Self {
            total_ms,
            execution_ms: total_ms,
            ..Default::default()
        }
    }
}

// ============================================================================
// Result Parser
// ============================================================================

/// Configuration for the result parser
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParserConfig {
    /// Maximum output size in bytes
    #[serde(default = "default_max_output_size")]
    pub max_output_size: usize,
    /// Maximum artifact size in bytes
    #[serde(default = "default_max_artifact_size")]
    pub max_artifact_size: usize,
    /// Whether to parse tracebacks
    #[serde(default = "default_true")]
    pub parse_tracebacks: bool,
    /// Whether to extract variables
    #[serde(default = "default_true")]
    pub extract_variables: bool,
    /// Whether to decode base64 artifacts
    #[serde(default = "default_true")]
    pub decode_base64_artifacts: bool,
    /// Marker for result start
    #[serde(default = "default_result_start_marker")]
    pub result_start_marker: String,
    /// Marker for result end
    #[serde(default = "default_result_end_marker")]
    pub result_end_marker: String,
}

fn default_max_output_size() -> usize {
    10 * 1024 * 1024 // 10MB
}

fn default_max_artifact_size() -> usize {
    50 * 1024 * 1024 // 50MB
}

fn default_true() -> bool {
    true
}

fn default_result_start_marker() -> String {
    "__CANAL_RESULT_START__".to_string()
}

fn default_result_end_marker() -> String {
    "__CANAL_RESULT_END__".to_string()
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            max_output_size: default_max_output_size(),
            max_artifact_size: default_max_artifact_size(),
            parse_tracebacks: true,
            extract_variables: true,
            decode_base64_artifacts: true,
            result_start_marker: default_result_start_marker(),
            result_end_marker: default_result_end_marker(),
        }
    }
}

/// Parsed output from execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedOutput {
    /// Cleaned output text
    pub text: String,
    /// Return value (if any)
    pub return_value: Option<serde_json::Value>,
    /// Extracted variables
    pub variables: HashMap<String, serde_json::Value>,
    /// Whether output was truncated
    pub truncated: bool,
}

/// Execution output for parsing
#[derive(Debug, Clone)]
pub struct ExecutionOutput {
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Exit code
    pub exit_code: i32,
    /// Raw JSON result (if available)
    pub raw_json: Option<serde_json::Value>,
}

/// Result parser for CodeAct execution output
pub struct ResultParser {
    config: ParserConfig,
}

impl ResultParser {
    /// Create a new result parser with default configuration
    pub fn new() -> Self {
        Self {
            config: ParserConfig::default(),
        }
    }

    /// Create a result parser with custom configuration
    pub fn with_config(config: ParserConfig) -> Self {
        Self { config }
    }

    /// Parse raw output into structured parsed output
    #[instrument(skip(self, raw), fields(raw_len = raw.len()))]
    pub fn parse_output(&self, raw: &str) -> Result<ParsedOutput> {
        debug!("Parsing output of {} bytes", raw.len());

        let mut text = raw.to_string();
        let mut return_value = None;
        let mut variables = HashMap::new();

        // Check for result markers
        if let Some(start_idx) = raw.find(&self.config.result_start_marker) {
            if let Some(end_idx) = raw.find(&self.config.result_end_marker) {
                if start_idx < end_idx {
                    let result_str =
                        &raw[start_idx + self.config.result_start_marker.len()..end_idx];
                    let result_str = result_str.trim();

                    // Try to parse as JSON
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(result_str) {
                        // Extract return value
                        if let Some(rv) = json.get("return_value") {
                            return_value = Some(rv.clone());
                        }

                        // Extract variables
                        if self.config.extract_variables {
                            if let Some(vars) = json.get("variables").and_then(|v| v.as_object()) {
                                for (k, v) in vars {
                                    variables.insert(k.clone(), v.clone());
                                }
                            }
                        }
                    }

                    // Remove markers from output
                    text = format!(
                        "{}{}",
                        &raw[..start_idx],
                        &raw[end_idx + self.config.result_end_marker.len()..]
                    )
                    .trim()
                    .to_string();
                }
            }
        }

        // Truncate if needed
        let truncated = text.len() > self.config.max_output_size;
        if truncated {
            text = text[..self.config.max_output_size].to_string();
            text.push_str("\n... (output truncated)");
        }

        Ok(ParsedOutput {
            text,
            return_value,
            variables,
            truncated,
        })
    }

    /// Parse exception traceback into structured error
    #[instrument(skip(self, traceback))]
    pub fn parse_exception(&self, traceback: &str) -> CodeActError {
        debug!("Parsing exception traceback");

        let mut error_type = ErrorType::Unknown;
        let mut message = String::new();
        let mut line_number = None;

        // Parse Python traceback format
        let lines: Vec<&str> = traceback.lines().collect();

        // Look for the final exception line (usually last non-empty line)
        for line in lines.iter().rev() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Match "ExceptionType: message" pattern
            if let Some(colon_idx) = line.find(':') {
                let exception_name = &line[..colon_idx];
                if exception_name.chars().all(|c| c.is_alphanumeric()) {
                    error_type = ErrorType::from_python_exception(exception_name);
                    message = line[colon_idx + 1..].trim().to_string();
                    break;
                }
            }

            // If no colon, the whole line might be the exception
            if line
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
            {
                error_type = ErrorType::from_python_exception(line);
                message = line.to_string();
                break;
            }
        }

        // Look for line number in traceback
        for line in &lines {
            if line.contains("line ") {
                // Parse "File ..., line N" format
                if let Some(line_idx) = line.find("line ") {
                    let after_line = &line[line_idx + 5..];
                    if let Some(num_str) = after_line.split(|c: char| !c.is_ascii_digit()).next() {
                        if let Ok(num) = num_str.parse::<u32>() {
                            line_number = Some(num);
                            break;
                        }
                    }
                }
            }
        }

        // If no message found, use the whole traceback
        if message.is_empty() {
            message = traceback
                .lines()
                .last()
                .unwrap_or("Unknown error")
                .to_string();
        }

        CodeActError {
            error_type,
            message,
            details: None,
            traceback: Some(traceback.to_string()),
            line_number,
            column: None,
            code_snippet: None,
        }
    }

    /// Extract artifacts from execution output
    #[instrument(skip(self, output))]
    pub fn extract_artifacts(&self, output: &ExecutionOutput) -> Vec<Artifact> {
        debug!("Extracting artifacts from output");
        let mut artifacts = Vec::new();

        // Try to extract from raw JSON if available
        if let Some(ref json) = output.raw_json {
            if let Some(arr) = json.get("artifacts").and_then(|v| v.as_array()) {
                for item in arr {
                    if let Some(artifact) = self.parse_artifact_from_json(item) {
                        if artifact.data.len() <= self.config.max_artifact_size {
                            artifacts.push(artifact);
                        } else {
                            warn!(
                                "Skipping artifact {} - size {} exceeds limit {}",
                                artifact.name,
                                artifact.data.len(),
                                self.config.max_artifact_size
                            );
                        }
                    }
                }
            }
        }

        // Look for base64-encoded artifacts in output
        if self.config.decode_base64_artifacts {
            artifacts.extend(self.extract_base64_artifacts(&output.stdout));
        }

        artifacts
    }

    /// Parse artifact from JSON object
    fn parse_artifact_from_json(&self, json: &serde_json::Value) -> Option<Artifact> {
        let name = json.get("name").and_then(|v| v.as_str())?;
        let artifact_type_str = json.get("type").and_then(|v| v.as_str()).unwrap_or("file");

        let artifact_type = match artifact_type_str {
            "screenshot" => ArtifactType::Screenshot,
            "plot" => ArtifactType::Plot,
            "dataframe" => ArtifactType::DataFrame,
            "file" => ArtifactType::File,
            other => ArtifactType::Other(other.to_string()),
        };

        // Get data (try base64 first, then raw)
        let data = if let Some(b64) = json.get("data_base64").and_then(|v| v.as_str()) {
            BASE64.decode(b64).ok()?
        } else if let Some(raw) = json.get("data").and_then(|v| v.as_str()) {
            raw.as_bytes().to_vec()
        } else {
            return None;
        };

        let mut artifact = Artifact::new(name, artifact_type, data);

        // Add metadata
        if let Some(meta) = json.get("metadata").and_then(|v| v.as_object()) {
            for (k, v) in meta {
                if let Some(s) = v.as_str() {
                    artifact.metadata.insert(k.clone(), s.to_string());
                }
            }
        }

        Some(artifact)
    }

    /// Extract base64-encoded artifacts from output text
    fn extract_base64_artifacts(&self, text: &str) -> Vec<Artifact> {
        let mut artifacts = Vec::new();

        // Look for common patterns like:
        // ARTIFACT:name:base64data
        // or embedded image tags
        for line in text.lines() {
            if let Some(stripped) = line.strip_prefix("ARTIFACT:") {
                if let Some((name, data)) = stripped.split_once(':') {
                    if let Ok(decoded) = BASE64.decode(data.trim()) {
                        let artifact_type = if name.ends_with(".png") || name.ends_with(".jpg") {
                            ArtifactType::Screenshot
                        } else {
                            ArtifactType::File
                        };
                        artifacts.push(Artifact::new(name, artifact_type, decoded));
                    }
                }
            }
        }

        artifacts
    }

    /// Format result for agent consumption
    #[instrument(skip(self, result))]
    pub fn format_for_agent(&self, result: &CodeActResult) -> String {
        debug!("Formatting result for agent");
        let mut output = String::new();

        // Status header
        output.push_str(&format!(
            "## Execution {}\n\n",
            result.status.as_str().to_uppercase()
        ));

        // Timing info
        output.push_str(&format!("**Duration:** {}ms\n\n", result.timing.total_ms));

        // Output
        if let Some(ref out) = result.output {
            if !out.is_empty() {
                output.push_str("### Output\n\n");
                output.push_str("```\n");
                output.push_str(out);
                if !out.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str("```\n\n");
            }
        }

        // Return value
        if let Some(ref rv) = result.return_value {
            output.push_str("### Return Value\n\n");
            if let Ok(pretty) = serde_json::to_string_pretty(rv) {
                output.push_str("```json\n");
                output.push_str(&pretty);
                output.push_str("\n```\n\n");
            }
        }

        // Variables
        if !result.variables.is_empty() {
            output.push_str("### Variables\n\n");
            output.push_str("| Name | Value |\n");
            output.push_str("|------|-------|\n");
            for (name, value) in &result.variables {
                let value_str = format_value_for_table(value);
                output.push_str(&format!("| `{}` | {} |\n", name, value_str));
            }
            output.push('\n');
        }

        // Artifacts
        if !result.artifacts.is_empty() {
            output.push_str("### Artifacts\n\n");
            for artifact in &result.artifacts {
                output.push_str(&format!(
                    "- **{}** ({}, {} bytes)\n",
                    artifact.name,
                    artifact.artifact_type.display_name(),
                    artifact.size()
                ));
            }
            output.push('\n');
        }

        // Error
        if let Some(ref error) = result.error {
            output.push_str("### Error\n\n");
            output.push_str(&format!("**{}:** {}\n\n", error.error_type, error.message));

            if let Some(ref traceback) = error.traceback {
                output.push_str("```\n");
                output.push_str(traceback);
                if !traceback.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str("```\n\n");
            }
        }

        output
    }
}

impl Default for ResultParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Format a JSON value for display in a table cell
fn format_value_for_table(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "`null`".to_string(),
        serde_json::Value::Bool(b) => format!("`{}`", b),
        serde_json::Value::Number(n) => format!("`{}`", n),
        serde_json::Value::String(s) => {
            if s.len() > 50 {
                format!("`\"{}...\"` ({} chars)", &s[..47], s.len())
            } else {
                format!("`\"{}\"`", s)
            }
        }
        serde_json::Value::Array(arr) => {
            format!("`[...]` ({} items)", arr.len())
        }
        serde_json::Value::Object(obj) => {
            format!("`{{...}}` ({} keys)", obj.len())
        }
    }
}

// ============================================================================
// Binary Result Handling
// ============================================================================

/// Handler for binary data in results (screenshots, files, etc.)
pub struct BinaryResultHandler;

impl BinaryResultHandler {
    /// Decode base64-encoded screenshot/image data
    pub fn decode_screenshot(base64_data: &str) -> Result<Vec<u8>> {
        BASE64
            .decode(base64_data)
            .map_err(|e| Error::InvalidInput(format!("Invalid base64 data: {}", e)))
    }

    /// Decode base64-encoded file content
    pub fn decode_file(base64_data: &str) -> Result<Vec<u8>> {
        BASE64
            .decode(base64_data)
            .map_err(|e| Error::InvalidInput(format!("Invalid base64 data: {}", e)))
    }

    /// Serialize DataFrame to JSON
    pub fn serialize_dataframe(data: &serde_json::Value) -> Result<Vec<u8>> {
        serde_json::to_vec_pretty(data).map_err(|e| Error::Serialization(e.to_string()))
    }

    /// Deserialize DataFrame from JSON bytes
    pub fn deserialize_dataframe(data: &[u8]) -> Result<serde_json::Value> {
        serde_json::from_slice(data).map_err(|e| Error::Serialization(e.to_string()))
    }

    /// Read file content and encode as base64
    pub async fn read_file_as_base64(path: &std::path::Path) -> Result<String> {
        let data = tokio::fs::read(path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read file: {}", e)))?;
        Ok(BASE64.encode(&data))
    }

    /// Write base64-encoded data to file
    pub async fn write_base64_to_file(path: &std::path::Path, base64_data: &str) -> Result<()> {
        let data = Self::decode_file(base64_data)?;
        tokio::fs::write(path, &data)
            .await
            .map_err(|e| Error::Internal(format!("Failed to write file: {}", e)))
    }
}

// ============================================================================
// Result Cache
// ============================================================================

/// Cached result entry
#[derive(Debug, Clone)]
pub struct CachedResult {
    /// The cached result
    pub result: CodeActResult,
    /// When the result was cached
    pub cached_at: DateTime<Utc>,
    /// Size in bytes (approximate)
    pub size_bytes: usize,
}

impl CachedResult {
    fn new(result: CodeActResult) -> Self {
        let size_bytes = Self::estimate_size(&result);
        Self {
            result,
            cached_at: Utc::now(),
            size_bytes,
        }
    }

    fn estimate_size(result: &CodeActResult) -> usize {
        let mut size = result.raw_stdout.len() + result.raw_stderr.len();
        if let Some(ref output) = result.output {
            size += output.len();
        }
        for artifact in &result.artifacts {
            size += artifact.data.len();
        }
        size
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        let elapsed = Utc::now()
            .signed_duration_since(self.cached_at)
            .to_std()
            .unwrap_or(Duration::ZERO);
        elapsed > ttl
    }
}

/// Result cache with TTL and size limits
pub struct ResultCache {
    cache: Arc<RwLock<HashMap<String, CachedResult>>>,
    max_size: usize,
    ttl: Duration,
    current_size: Arc<RwLock<usize>>,
}

impl ResultCache {
    /// Create a new result cache
    pub fn new(max_size: usize, ttl: Duration) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            max_size,
            ttl,
            current_size: Arc::new(RwLock::new(0)),
        }
    }

    /// Create with default settings (100MB cache, 5 minute TTL)
    pub fn with_defaults() -> Self {
        Self::new(100 * 1024 * 1024, Duration::from_secs(300))
    }

    /// Get a result from cache
    #[instrument(skip(self))]
    pub async fn get(&self, execution_id: &str) -> Option<CodeActResult> {
        let cache = self.cache.read().await;
        if let Some(cached) = cache.get(execution_id) {
            if !cached.is_expired(self.ttl) {
                debug!("Cache hit for {}", execution_id);
                return Some(cached.result.clone());
            }
        }
        debug!("Cache miss for {}", execution_id);
        None
    }

    /// Store a result in cache
    #[instrument(skip(self, result))]
    pub async fn set(&self, execution_id: &str, result: CodeActResult) {
        let cached = CachedResult::new(result);
        let entry_size = cached.size_bytes;

        // Check if we need to evict entries
        self.maybe_evict(entry_size).await;

        let mut cache = self.cache.write().await;
        let mut current_size = self.current_size.write().await;

        // Remove old entry if exists
        if let Some(old) = cache.remove(execution_id) {
            *current_size = current_size.saturating_sub(old.size_bytes);
        }

        // Add new entry
        cache.insert(execution_id.to_string(), cached);
        *current_size += entry_size;

        debug!(
            "Cached result {} ({} bytes, total cache size: {} bytes)",
            execution_id, entry_size, *current_size
        );
    }

    /// Remove a result from cache
    pub async fn remove(&self, execution_id: &str) -> Option<CodeActResult> {
        let mut cache = self.cache.write().await;
        let mut current_size = self.current_size.write().await;

        if let Some(cached) = cache.remove(execution_id) {
            *current_size = current_size.saturating_sub(cached.size_bytes);
            Some(cached.result)
        } else {
            None
        }
    }

    /// Clear expired entries
    #[instrument(skip(self))]
    pub async fn cleanup_expired(&self) {
        let mut cache = self.cache.write().await;
        let mut current_size = self.current_size.write().await;

        let expired: Vec<String> = cache
            .iter()
            .filter(|(_, v)| v.is_expired(self.ttl))
            .map(|(k, _)| k.clone())
            .collect();

        for key in expired {
            if let Some(cached) = cache.remove(&key) {
                *current_size = current_size.saturating_sub(cached.size_bytes);
                debug!("Evicted expired entry {}", key);
            }
        }
    }

    /// Clear all entries
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        let mut current_size = self.current_size.write().await;
        cache.clear();
        *current_size = 0;
    }

    /// Get cache statistics
    pub async fn stats(&self) -> CacheStats {
        let cache = self.cache.read().await;
        let current_size = *self.current_size.read().await;

        CacheStats {
            entry_count: cache.len(),
            total_size_bytes: current_size,
            max_size_bytes: self.max_size,
            ttl_seconds: self.ttl.as_secs(),
        }
    }

    /// Evict entries if needed to make room
    async fn maybe_evict(&self, needed_bytes: usize) {
        let current = *self.current_size.read().await;

        if current + needed_bytes <= self.max_size {
            return;
        }

        // First, clean up expired entries
        self.cleanup_expired().await;

        // If still over limit, evict oldest entries
        let mut cache = self.cache.write().await;
        let mut current_size = self.current_size.write().await;

        if *current_size + needed_bytes > self.max_size {
            // Sort by age and evict oldest - collect keys and sizes first
            let mut entries: Vec<_> = cache
                .iter()
                .map(|(k, v)| (k.clone(), v.cached_at, v.size_bytes))
                .collect();
            entries.sort_by(|a, b| a.1.cmp(&b.1));

            for (key, _, size_bytes) in entries {
                if *current_size + needed_bytes <= self.max_size {
                    break;
                }
                *current_size = current_size.saturating_sub(size_bytes);
                cache.remove(&key);
                debug!("Evicted entry {} to make room", key);
            }
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    /// Number of entries in cache
    pub entry_count: usize,
    /// Total size of cached data in bytes
    pub total_size_bytes: usize,
    /// Maximum cache size in bytes
    pub max_size_bytes: usize,
    /// TTL in seconds
    pub ttl_seconds: u64,
}

// ============================================================================
// Helper Modules
// ============================================================================

/// Serde helper for base64 encoding of `Vec<u8>`
mod base64_serde {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&BASE64.encode(data))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        BASE64.decode(&s).map_err(serde::de::Error::custom)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========== CodeActResult Tests ==========

    #[test]
    fn test_codeact_result_new() {
        let result = CodeActResult::new("exec-123");
        assert_eq!(result.execution_id, "exec-123");
        assert_eq!(result.status, ExecutionStatus::Success);
        assert!(result.output.is_none());
        assert!(result.error.is_none());
        assert!(result.artifacts.is_empty());
    }

    #[test]
    fn test_codeact_result_success() {
        let result = CodeActResult::success("exec-456", "Hello, World!");
        assert_eq!(result.status, ExecutionStatus::Success);
        assert_eq!(result.output, Some("Hello, World!".to_string()));
        assert!(result.is_success());
        assert!(!result.is_error());
    }

    #[test]
    fn test_codeact_result_error() {
        let error = CodeActError::new(ErrorType::RuntimeError, "Division by zero");
        let result = CodeActResult::error("exec-789", error);
        assert_eq!(result.status, ExecutionStatus::Error);
        assert!(result.is_error());
        assert!(result.error.is_some());
    }

    #[test]
    fn test_codeact_result_timeout() {
        let result = CodeActResult::timeout("exec-timeout", 30000);
        assert_eq!(result.status, ExecutionStatus::Timeout);
        assert!(result.is_timeout());
        assert_eq!(result.exit_code, 124);
        assert_eq!(result.timing.total_ms, 30000);
    }

    #[test]
    fn test_codeact_result_cancelled() {
        let result = CodeActResult::cancelled("exec-cancel", 5000);
        assert_eq!(result.status, ExecutionStatus::Cancelled);
        assert!(result.is_cancelled());
        assert_eq!(result.exit_code, 130);
    }

    #[test]
    fn test_codeact_result_add_artifact() {
        let mut result = CodeActResult::new("exec-art");
        let artifact = Artifact::screenshot("test.png", vec![1, 2, 3]);
        result.add_artifact(artifact);
        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(result.total_artifact_size(), 3);
    }

    #[test]
    fn test_codeact_result_add_variable() {
        let mut result = CodeActResult::new("exec-var");
        result.add_variable("x", serde_json::json!(42));
        assert_eq!(result.variables.get("x"), Some(&serde_json::json!(42)));
    }

    #[test]
    fn test_codeact_result_serialization() {
        let result = CodeActResult::success("exec-serial", "output");
        let json = serde_json::to_string(&result).unwrap();
        let parsed: CodeActResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.execution_id, "exec-serial");
        assert_eq!(parsed.status, ExecutionStatus::Success);
    }

    // ========== ExecutionStatus Tests ==========

    #[test]
    fn test_execution_status_display() {
        assert_eq!(ExecutionStatus::Success.to_string(), "success");
        assert_eq!(ExecutionStatus::Error.to_string(), "error");
        assert_eq!(ExecutionStatus::Timeout.to_string(), "timeout");
        assert_eq!(ExecutionStatus::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn test_execution_status_is_failure() {
        assert!(!ExecutionStatus::Success.is_failure());
        assert!(ExecutionStatus::Error.is_failure());
        assert!(ExecutionStatus::Timeout.is_failure());
        assert!(ExecutionStatus::Cancelled.is_failure());
    }

    #[test]
    fn test_execution_status_serialization() {
        let json = serde_json::to_string(&ExecutionStatus::Success).unwrap();
        assert_eq!(json, "\"success\"");
        let parsed: ExecutionStatus = serde_json::from_str("\"error\"").unwrap();
        assert_eq!(parsed, ExecutionStatus::Error);
    }

    // ========== CodeActError Tests ==========

    #[test]
    fn test_codeact_error_new() {
        let error = CodeActError::new(ErrorType::TypeError, "expected int, got string");
        assert_eq!(error.error_type, ErrorType::TypeError);
        assert_eq!(error.message, "expected int, got string");
    }

    #[test]
    fn test_codeact_error_with_traceback() {
        let error = CodeActError::new(ErrorType::RuntimeError, "test error")
            .with_traceback("File test.py, line 1\n  x = 1/0")
            .with_line_number(1);
        assert!(error.traceback.is_some());
        assert_eq!(error.line_number, Some(1));
    }

    #[test]
    fn test_codeact_error_format_display() {
        let error =
            CodeActError::new(ErrorType::NameError, "undefined variable 'x'").with_line_number(42);
        let display = error.format_display();
        assert!(display.contains("NameError"));
        assert!(display.contains("undefined variable 'x'"));
        assert!(display.contains("line 42"));
    }

    // ========== ErrorType Tests ==========

    #[test]
    fn test_error_type_from_python_exception() {
        assert_eq!(
            ErrorType::from_python_exception("SyntaxError"),
            ErrorType::SyntaxError
        );
        assert_eq!(
            ErrorType::from_python_exception("TypeError"),
            ErrorType::TypeError
        );
        assert_eq!(
            ErrorType::from_python_exception("ModuleNotFoundError"),
            ErrorType::ImportError
        );
        assert_eq!(
            ErrorType::from_python_exception("UnknownError"),
            ErrorType::Unknown
        );
    }

    #[test]
    fn test_error_type_display() {
        assert_eq!(ErrorType::SyntaxError.to_string(), "SyntaxError");
        assert_eq!(ErrorType::RuntimeError.to_string(), "RuntimeError");
    }

    // ========== Artifact Tests ==========

    #[test]
    fn test_artifact_new() {
        let artifact = Artifact::new("test.txt", ArtifactType::File, vec![1, 2, 3, 4]);
        assert_eq!(artifact.name, "test.txt");
        assert_eq!(artifact.artifact_type, ArtifactType::File);
        assert_eq!(artifact.size(), 4);
    }

    #[test]
    fn test_artifact_screenshot() {
        let artifact = Artifact::screenshot("screen.png", vec![0x89, 0x50, 0x4E, 0x47]);
        assert_eq!(artifact.artifact_type, ArtifactType::Screenshot);
        assert_eq!(artifact.mime_type(), "image/png");
    }

    #[test]
    fn test_artifact_dataframe() {
        let artifact = Artifact::dataframe("data.json", b"{}".to_vec());
        assert_eq!(artifact.artifact_type, ArtifactType::DataFrame);
        assert_eq!(artifact.mime_type(), "application/json");
    }

    #[test]
    fn test_artifact_with_metadata() {
        let artifact = Artifact::file("data.csv", vec![])
            .with_metadata("encoding", "utf-8")
            .with_metadata("rows", "100");
        assert_eq!(
            artifact.metadata.get("encoding"),
            Some(&"utf-8".to_string())
        );
        assert_eq!(artifact.metadata.get("rows"), Some(&"100".to_string()));
    }

    #[test]
    fn test_artifact_data_base64() {
        let artifact = Artifact::file("test", b"hello".to_vec());
        assert_eq!(artifact.data_base64(), "aGVsbG8=");
    }

    #[test]
    fn test_artifact_data_as_string() {
        let artifact = Artifact::file("test.txt", b"hello world".to_vec());
        assert_eq!(artifact.data_as_string(), Some("hello world".to_string()));

        let binary = Artifact::file("test.bin", vec![0xFF, 0xFE]);
        assert!(binary.data_as_string().is_none());
    }

    #[test]
    fn test_artifact_mime_type_from_extension() {
        let png = Artifact::file("image.png", vec![]);
        assert_eq!(png.mime_type(), "image/png");

        let csv = Artifact::file("data.csv", vec![]);
        assert_eq!(csv.mime_type(), "text/csv");

        let pdf = Artifact::file("doc.pdf", vec![]);
        assert_eq!(pdf.mime_type(), "application/pdf");
    }

    #[test]
    fn test_artifact_serialization() {
        let artifact = Artifact::file("test", b"data".to_vec());
        let json = serde_json::to_string(&artifact).unwrap();
        let parsed: Artifact = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.data, b"data");
    }

    // ========== ArtifactType Tests ==========

    #[test]
    fn test_artifact_type_display_name() {
        assert_eq!(ArtifactType::Screenshot.display_name(), "Screenshot");
        assert_eq!(ArtifactType::DataFrame.display_name(), "DataFrame");
        assert_eq!(
            ArtifactType::Other("Custom".to_string()).display_name(),
            "Custom"
        );
    }

    // ========== ExecutionTiming Tests ==========

    #[test]
    fn test_execution_timing_default() {
        let timing = ExecutionTiming::default();
        assert_eq!(timing.total_ms, 0);
        assert_eq!(timing.execution_ms, 0);
    }

    #[test]
    fn test_execution_timing_from_total() {
        let timing = ExecutionTiming::from_total(1000);
        assert_eq!(timing.total_ms, 1000);
        assert_eq!(timing.execution_ms, 1000);
    }

    // ========== ResultParser Tests ==========

    #[test]
    fn test_parser_config_default() {
        let config = ParserConfig::default();
        assert_eq!(config.max_output_size, 10 * 1024 * 1024);
        assert!(config.parse_tracebacks);
        assert!(config.extract_variables);
    }

    #[test]
    fn test_parser_parse_output_simple() {
        let parser = ResultParser::new();
        let result = parser.parse_output("Hello, World!").unwrap();
        assert_eq!(result.text, "Hello, World!");
        assert!(result.return_value.is_none());
        assert!(!result.truncated);
    }

    #[test]
    fn test_parser_parse_output_with_markers() {
        let parser = ResultParser::new();
        let output = "some output\n__CANAL_RESULT_START__\n{\"return_value\": 42}\n__CANAL_RESULT_END__\n";
        let result = parser.parse_output(output).unwrap();
        assert!(result.text.contains("some output"));
        assert!(!result.text.contains("CANAL"));
        assert_eq!(result.return_value, Some(serde_json::json!(42)));
    }

    #[test]
    fn test_parser_parse_output_with_variables() {
        let parser = ResultParser::new();
        let output = "__CANAL_RESULT_START__\n{\"variables\": {\"x\": 1, \"y\": \"test\"}}\n__CANAL_RESULT_END__";
        let result = parser.parse_output(output).unwrap();
        assert_eq!(result.variables.get("x"), Some(&serde_json::json!(1)));
        assert_eq!(result.variables.get("y"), Some(&serde_json::json!("test")));
    }

    #[test]
    fn test_parser_parse_exception_python() {
        let parser = ResultParser::new();
        let traceback = r#"Traceback (most recent call last):
  File "test.py", line 10, in <module>
    result = 1 / 0
ZeroDivisionError: division by zero"#;

        let error = parser.parse_exception(traceback);
        assert_eq!(error.error_type, ErrorType::Unknown); // ZeroDivisionError maps to Unknown
        assert!(error.message.contains("division by zero"));
        assert_eq!(error.line_number, Some(10));
    }

    #[test]
    fn test_parser_parse_exception_name_error() {
        let parser = ResultParser::new();
        let traceback = r#"Traceback (most recent call last):
  File "test.py", line 5
NameError: name 'undefined_var' is not defined"#;

        let error = parser.parse_exception(traceback);
        assert_eq!(error.error_type, ErrorType::NameError);
        assert!(error.message.contains("undefined_var"));
    }

    #[test]
    fn test_parser_extract_artifacts_from_json() {
        let parser = ResultParser::new();
        let output = ExecutionOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            raw_json: Some(serde_json::json!({
                "artifacts": [
                    {
                        "name": "test.png",
                        "type": "screenshot",
                        "data_base64": "aGVsbG8="
                    }
                ]
            })),
        };

        let artifacts = parser.extract_artifacts(&output);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].name, "test.png");
        assert_eq!(artifacts[0].artifact_type, ArtifactType::Screenshot);
        assert_eq!(artifacts[0].data, b"hello");
    }

    #[test]
    fn test_parser_format_for_agent_success() {
        let parser = ResultParser::new();
        let mut result = CodeActResult::success("exec-1", "Hello, World!");
        result.timing = ExecutionTiming::from_total(100);

        let formatted = parser.format_for_agent(&result);
        assert!(formatted.contains("## Execution SUCCESS"));
        assert!(formatted.contains("**Duration:** 100ms"));
        assert!(formatted.contains("Hello, World!"));
    }

    #[test]
    fn test_parser_format_for_agent_with_variables() {
        let parser = ResultParser::new();
        let mut result = CodeActResult::success("exec-1", "");
        result.add_variable("count", serde_json::json!(42));
        result.add_variable("name", serde_json::json!("test"));

        let formatted = parser.format_for_agent(&result);
        assert!(formatted.contains("### Variables"));
        assert!(formatted.contains("`count`"));
        assert!(formatted.contains("`42`"));
    }

    #[test]
    fn test_parser_format_for_agent_with_error() {
        let parser = ResultParser::new();
        let error =
            CodeActError::new(ErrorType::TypeError, "test error").with_traceback("Traceback here");
        let result = CodeActResult::error("exec-1", error);

        let formatted = parser.format_for_agent(&result);
        assert!(formatted.contains("## Execution ERROR"));
        assert!(formatted.contains("### Error"));
        assert!(formatted.contains("TypeError"));
        assert!(formatted.contains("test error"));
    }

    // ========== BinaryResultHandler Tests ==========

    #[test]
    fn test_binary_decode_screenshot() {
        let encoded = "aGVsbG8gd29ybGQ="; // "hello world"
        let decoded = BinaryResultHandler::decode_screenshot(encoded).unwrap();
        assert_eq!(decoded, b"hello world");
    }

    #[test]
    fn test_binary_decode_invalid_base64() {
        let result = BinaryResultHandler::decode_screenshot("not valid base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_binary_serialize_dataframe() {
        let df = serde_json::json!({
            "columns": ["a", "b"],
            "data": [[1, 2], [3, 4]]
        });
        let bytes = BinaryResultHandler::serialize_dataframe(&df).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed, df);
    }

    #[test]
    fn test_binary_deserialize_dataframe() {
        let bytes = br#"{"columns": ["x"], "data": [[1]]}"#;
        let df = BinaryResultHandler::deserialize_dataframe(bytes).unwrap();
        assert_eq!(df["columns"][0], "x");
    }

    // ========== ResultCache Tests ==========

    #[tokio::test]
    async fn test_cache_set_and_get() {
        let cache = ResultCache::with_defaults();
        let result = CodeActResult::success("exec-1", "test output");

        cache.set("exec-1", result).await;

        let cached = cache.get("exec-1").await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().execution_id, "exec-1");
    }

    #[tokio::test]
    async fn test_cache_miss() {
        let cache = ResultCache::with_defaults();
        let cached = cache.get("nonexistent").await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_cache_remove() {
        let cache = ResultCache::with_defaults();
        let result = CodeActResult::success("exec-1", "test");

        cache.set("exec-1", result).await;
        let removed = cache.remove("exec-1").await;
        assert!(removed.is_some());

        let cached = cache.get("exec-1").await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let cache = ResultCache::with_defaults();
        cache
            .set("exec-1", CodeActResult::success("exec-1", ""))
            .await;
        cache
            .set("exec-2", CodeActResult::success("exec-2", ""))
            .await;

        cache.clear().await;

        let stats = cache.stats().await;
        assert_eq!(stats.entry_count, 0);
        assert_eq!(stats.total_size_bytes, 0);
    }

    #[tokio::test]
    async fn test_cache_stats() {
        let cache = ResultCache::new(1024, Duration::from_secs(60));
        let result = CodeActResult::success("exec-1", "test output");

        cache.set("exec-1", result).await;

        let stats = cache.stats().await;
        assert_eq!(stats.entry_count, 1);
        assert!(stats.total_size_bytes > 0);
        assert_eq!(stats.max_size_bytes, 1024);
        assert_eq!(stats.ttl_seconds, 60);
    }

    #[tokio::test]
    async fn test_cache_ttl_expiration() {
        let cache = ResultCache::new(1024, Duration::from_millis(1)); // 1ms TTL
        let result = CodeActResult::success("exec-1", "test");

        cache.set("exec-1", result).await;

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(10)).await;

        let cached = cache.get("exec-1").await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_cache_cleanup_expired() {
        let cache = ResultCache::new(10240, Duration::from_millis(1));

        cache
            .set("exec-1", CodeActResult::success("exec-1", "a"))
            .await;
        cache
            .set("exec-2", CodeActResult::success("exec-2", "b"))
            .await;

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(10)).await;

        cache.cleanup_expired().await;

        let stats = cache.stats().await;
        assert_eq!(stats.entry_count, 0);
    }

    // ========== Integration Tests ==========

    #[test]
    fn test_full_result_parsing_flow() {
        let parser = ResultParser::new();

        // Simulate execution output
        let stdout = r#"Processing data...
Result: 42
__CANAL_RESULT_START__
{"return_value": 42, "variables": {"result": 42}}
__CANAL_RESULT_END__
"#;

        let parsed = parser.parse_output(stdout).unwrap();
        assert!(parsed.text.contains("Processing data..."));
        assert!(parsed.text.contains("Result: 42"));
        assert_eq!(parsed.return_value, Some(serde_json::json!(42)));
        assert_eq!(parsed.variables.get("result"), Some(&serde_json::json!(42)));
    }

    #[test]
    fn test_format_value_for_table() {
        assert_eq!(format_value_for_table(&serde_json::json!(null)), "`null`");
        assert_eq!(format_value_for_table(&serde_json::json!(true)), "`true`");
        assert_eq!(format_value_for_table(&serde_json::json!(42)), "`42`");
        assert_eq!(
            format_value_for_table(&serde_json::json!("hello")),
            "`\"hello\"`"
        );
        assert_eq!(
            format_value_for_table(&serde_json::json!([1, 2, 3])),
            "`[...]` (3 items)"
        );
        assert_eq!(
            format_value_for_table(&serde_json::json!({"a": 1})),
            "`{...}` (1 keys)"
        );
    }

    #[test]
    fn test_cached_result_size_estimation() {
        let mut result = CodeActResult::success("exec-1", "output");
        result.raw_stdout = "stdout content".to_string();
        result.raw_stderr = "stderr content".to_string();
        result.add_artifact(Artifact::file("test", vec![0u8; 100]));

        let cached = CachedResult::new(result);
        // Size should include stdout + stderr + output + artifact data
        assert!(cached.size_bytes > 100);
    }
}
