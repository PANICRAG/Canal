//! Agent Error Classification and Recovery
//!
//! This module provides error classification and recovery strategies for agent execution,
//! implementing the Manus-style error handling pattern with automatic retry and user prompts.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

// ============================================================================
// Agent Error Types
// ============================================================================

/// Classified agent error with recovery information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentError {
    /// Transient error - retry immediately with backoff
    Transient {
        /// Original error message
        message: String,
        /// Error source/category
        source: ErrorSource,
        /// Current retry count
        retry_count: u32,
        /// Maximum allowed retries
        max_retries: u32,
        /// Whether to use backoff
        use_backoff: bool,
    },

    /// Rate limited - retry after delay
    RateLimited {
        /// Original error message
        message: String,
        /// Error source
        source: ErrorSource,
        /// Suggested retry delay in milliseconds
        retry_after_ms: u64,
    },

    /// Ambiguous result - need user input to resolve
    Ambiguous {
        /// Original error message
        message: String,
        /// Error source
        source: ErrorSource,
        /// Recovery options for user to choose from
        options: Vec<RecoveryOption>,
        /// Additional context for decision making
        context: Option<String>,
    },

    /// Fatal error - cannot recover automatically
    Fatal {
        /// Original error message
        message: String,
        /// Error source
        source: ErrorSource,
        /// Additional context about the error
        context: ErrorContext,
        /// Suggested manual recovery steps
        manual_recovery: Option<String>,
    },
}

impl AgentError {
    /// Create a transient error
    pub fn transient(message: impl Into<String>, source: ErrorSource) -> Self {
        Self::Transient {
            message: message.into(),
            source,
            retry_count: 0,
            max_retries: 3,
            use_backoff: true,
        }
    }

    /// Create a rate limited error
    pub fn rate_limited(message: impl Into<String>, retry_after: Duration) -> Self {
        Self::RateLimited {
            message: message.into(),
            source: ErrorSource::External,
            retry_after_ms: retry_after.as_millis() as u64,
        }
    }

    /// Create an ambiguous error requiring user input
    pub fn ambiguous(message: impl Into<String>, options: Vec<RecoveryOption>) -> Self {
        Self::Ambiguous {
            message: message.into(),
            source: ErrorSource::Tool,
            options,
            context: None,
        }
    }

    /// Create a fatal error
    pub fn fatal(message: impl Into<String>, context: ErrorContext) -> Self {
        Self::Fatal {
            message: message.into(),
            source: context.source,
            context,
            manual_recovery: None,
        }
    }

    /// Get the error message
    pub fn message(&self) -> &str {
        match self {
            Self::Transient { message, .. } => message,
            Self::RateLimited { message, .. } => message,
            Self::Ambiguous { message, .. } => message,
            Self::Fatal { message, .. } => message,
        }
    }

    /// Get the error source
    pub fn source(&self) -> ErrorSource {
        match self {
            Self::Transient { source, .. } => *source,
            Self::RateLimited { source, .. } => *source,
            Self::Ambiguous { source, .. } => *source,
            Self::Fatal { source, .. } => *source,
        }
    }

    /// Check if this error is retriable
    pub fn is_retriable(&self) -> bool {
        matches!(self, Self::Transient { .. } | Self::RateLimited { .. })
    }

    /// Check if this error requires user input
    pub fn requires_user_input(&self) -> bool {
        matches!(self, Self::Ambiguous { .. })
    }

    /// Check if this error is fatal
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Fatal { .. })
    }

    /// Increment retry count and check if should continue
    pub fn increment_retry(&mut self) -> bool {
        if let Self::Transient {
            retry_count,
            max_retries,
            ..
        } = self
        {
            *retry_count += 1;
            *retry_count <= *max_retries
        } else {
            false
        }
    }

    /// Set additional context
    pub fn with_context(mut self, ctx: impl Into<String>) -> Self {
        match &mut self {
            Self::Ambiguous { context, .. } => *context = Some(ctx.into()),
            Self::Fatal {
                manual_recovery, ..
            } => *manual_recovery = Some(ctx.into()),
            _ => {}
        }
        self
    }
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient {
                message,
                retry_count,
                max_retries,
                ..
            } => {
                write!(
                    f,
                    "Transient error (retry {}/{}): {}",
                    retry_count, max_retries, message
                )
            }
            Self::RateLimited {
                message,
                retry_after_ms,
                ..
            } => {
                write!(
                    f,
                    "Rate limited (retry after {}ms): {}",
                    retry_after_ms, message
                )
            }
            Self::Ambiguous {
                message, options, ..
            } => {
                write!(
                    f,
                    "Ambiguous (needs decision): {} [{} options]",
                    message,
                    options.len()
                )
            }
            Self::Fatal {
                message, context, ..
            } => {
                write!(f, "Fatal error: {} ({})", message, context.operation)
            }
        }
    }
}

impl std::error::Error for AgentError {}

// ============================================================================
// Error Source
// ============================================================================

/// Source/category of the error
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorSource {
    /// Error from network/HTTP operations
    Network,
    /// Error from tool execution
    Tool,
    /// Error from LLM API
    Llm,
    /// Error from external service
    External,
    /// Error from file system
    FileSystem,
    /// Error from permission system
    Permission,
    /// Error from configuration
    Config,
    /// Unknown source
    Unknown,
}

impl ErrorSource {
    /// Check if this source is typically transient
    pub fn is_typically_transient(&self) -> bool {
        matches!(self, Self::Network | Self::External)
    }

    /// Get default max retries for this source
    pub fn default_max_retries(&self) -> u32 {
        match self {
            Self::Network => 3,
            Self::External => 2,
            Self::Llm => 2,
            Self::Tool => 1,
            _ => 0,
        }
    }
}

// ============================================================================
// Error Context
// ============================================================================

/// Context about where and how an error occurred
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorContext {
    /// The operation that failed
    pub operation: String,
    /// Tool name if applicable
    pub tool_name: Option<String>,
    /// Tool parameters if applicable
    pub tool_params: Option<serde_json::Value>,
    /// Error source
    pub source: ErrorSource,
    /// Timestamp when error occurred
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Stack trace or breadcrumbs
    pub breadcrumbs: Vec<String>,
    /// Related checkpoint ID for rollback
    pub checkpoint_id: Option<String>,
    /// Session ID
    pub session_id: Option<String>,
}

impl ErrorContext {
    /// Create a new error context
    pub fn new(operation: impl Into<String>, source: ErrorSource) -> Self {
        Self {
            operation: operation.into(),
            tool_name: None,
            tool_params: None,
            source,
            timestamp: chrono::Utc::now(),
            breadcrumbs: Vec::new(),
            checkpoint_id: None,
            session_id: None,
        }
    }

    /// Set tool information
    pub fn with_tool(mut self, name: impl Into<String>, params: serde_json::Value) -> Self {
        self.tool_name = Some(name.into());
        self.tool_params = Some(params);
        self
    }

    /// Add a breadcrumb
    pub fn with_breadcrumb(mut self, breadcrumb: impl Into<String>) -> Self {
        self.breadcrumbs.push(breadcrumb.into());
        self
    }

    /// Set checkpoint ID for potential rollback
    pub fn with_checkpoint(mut self, checkpoint_id: impl Into<String>) -> Self {
        self.checkpoint_id = Some(checkpoint_id.into());
        self
    }

    /// Set session ID
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
}

// ============================================================================
// Recovery Options
// ============================================================================

/// A recovery option for ambiguous errors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryOption {
    /// Unique identifier for this option
    pub id: String,
    /// Human-readable description
    pub description: String,
    /// Action to take if this option is selected
    pub action: RecoveryAction,
    /// Risk level of this option
    pub risk_level: RiskLevel,
    /// Whether this is the recommended option
    #[serde(default)]
    pub recommended: bool,
}

impl RecoveryOption {
    /// Create a new recovery option
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
        action: RecoveryAction,
        risk_level: RiskLevel,
    ) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            action,
            risk_level,
            recommended: false,
        }
    }

    /// Mark this option as recommended
    pub fn recommended(mut self) -> Self {
        self.recommended = true;
        self
    }
}

/// Action to take for recovery
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecoveryAction {
    /// Retry the failed operation
    Retry {
        /// Modified parameters for retry
        modified_params: Option<serde_json::Value>,
    },

    /// Skip this operation and continue
    Skip,

    /// Use an alternative approach
    Alternative {
        /// Alternative tool to use
        tool: String,
        /// Parameters for alternative
        params: serde_json::Value,
    },

    /// Rollback to checkpoint
    Rollback {
        /// Checkpoint ID to rollback to
        checkpoint_id: String,
    },

    /// Abort the entire operation
    Abort,

    /// Wait and retry after delay
    WaitAndRetry {
        /// Delay before retry in milliseconds
        delay_ms: u64,
    },

    /// Ask for user input
    RequestInput {
        /// Prompt for user
        prompt: String,
        /// Input type expected
        input_type: InputType,
    },
}

/// Type of input expected from user
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    /// Free text input
    Text,
    /// Yes/no confirmation
    Confirmation,
    /// Select from options
    Selection { options: Vec<String> },
    /// File path input
    FilePath,
}

/// Risk level for recovery options
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// No risk - safe operation
    None,
    /// Low risk - minor side effects possible
    Low,
    /// Medium risk - may affect other operations
    Medium,
    /// High risk - may cause data loss or significant changes
    High,
}

// ============================================================================
// Error Classifier
// ============================================================================

/// Classifies errors into appropriate AgentError types
#[allow(dead_code)]
pub struct ErrorClassifier {
    /// Patterns for transient errors
    transient_patterns: Vec<String>,
    /// Patterns for rate limit errors
    rate_limit_patterns: Vec<String>,
    /// Default max retries
    default_max_retries: u32,
}

impl Default for ErrorClassifier {
    fn default() -> Self {
        Self::new()
    }
}

impl ErrorClassifier {
    /// Create a new error classifier with default patterns
    pub fn new() -> Self {
        Self {
            transient_patterns: vec![
                "timeout".to_string(),
                "connection refused".to_string(),
                "connection reset".to_string(),
                "temporarily unavailable".to_string(),
                "service unavailable".to_string(),
                "network error".to_string(),
                "ETIMEDOUT".to_string(),
                "ECONNRESET".to_string(),
            ],
            rate_limit_patterns: vec![
                "rate limit".to_string(),
                "too many requests".to_string(),
                "429".to_string(),
                "quota exceeded".to_string(),
                "throttled".to_string(),
            ],
            default_max_retries: 3,
        }
    }

    /// Classify an error message into an AgentError
    pub fn classify(
        &self,
        message: &str,
        source: ErrorSource,
        context: Option<ErrorContext>,
    ) -> AgentError {
        let message_lower = message.to_lowercase();

        // Check for rate limit errors
        if self
            .rate_limit_patterns
            .iter()
            .any(|p| message_lower.contains(p))
        {
            return AgentError::RateLimited {
                message: message.to_string(),
                source,
                retry_after_ms: 60_000, // Default 1 minute
            };
        }

        // Check for transient errors
        if self
            .transient_patterns
            .iter()
            .any(|p| message_lower.contains(p))
            || source.is_typically_transient()
        {
            return AgentError::Transient {
                message: message.to_string(),
                source,
                retry_count: 0,
                max_retries: source.default_max_retries(),
                use_backoff: true,
            };
        }

        // Check for permission errors
        if message_lower.contains("permission")
            || message_lower.contains("access denied")
            || message_lower.contains("forbidden")
        {
            return AgentError::Ambiguous {
                message: message.to_string(),
                source: ErrorSource::Permission,
                options: vec![
                    RecoveryOption::new(
                        "grant",
                        "Grant permission and retry",
                        RecoveryAction::RequestInput {
                            prompt: "Do you want to grant permission for this operation?"
                                .to_string(),
                            input_type: InputType::Confirmation,
                        },
                        RiskLevel::Medium,
                    )
                    .recommended(),
                    RecoveryOption::new(
                        "skip",
                        "Skip this operation",
                        RecoveryAction::Skip,
                        RiskLevel::Low,
                    ),
                    RecoveryOption::new(
                        "abort",
                        "Abort the task",
                        RecoveryAction::Abort,
                        RiskLevel::None,
                    ),
                ],
                context: None,
            };
        }

        // Check for not found errors
        if message_lower.contains("not found")
            || message_lower.contains("does not exist")
            || message_lower.contains("no such file")
        {
            return AgentError::Ambiguous {
                message: message.to_string(),
                source: ErrorSource::FileSystem,
                options: vec![
                    RecoveryOption::new(
                        "create",
                        "Create the missing resource",
                        RecoveryAction::RequestInput {
                            prompt: "Would you like to create the missing resource?".to_string(),
                            input_type: InputType::Confirmation,
                        },
                        RiskLevel::Low,
                    ),
                    RecoveryOption::new(
                        "alternative",
                        "Use an alternative path",
                        RecoveryAction::RequestInput {
                            prompt: "Please provide an alternative path:".to_string(),
                            input_type: InputType::FilePath,
                        },
                        RiskLevel::Low,
                    ),
                    RecoveryOption::new(
                        "skip",
                        "Skip this operation",
                        RecoveryAction::Skip,
                        RiskLevel::None,
                    ),
                ],
                context: None,
            };
        }

        // Check for conflict errors
        if message_lower.contains("conflict")
            || message_lower.contains("already exists")
            || message_lower.contains("duplicate")
        {
            return AgentError::Ambiguous {
                message: message.to_string(),
                source,
                options: vec![
                    RecoveryOption::new(
                        "overwrite",
                        "Overwrite existing content",
                        RecoveryAction::Retry {
                            modified_params: None, // Would be set by caller
                        },
                        RiskLevel::Medium,
                    ),
                    RecoveryOption::new(
                        "rename",
                        "Create with a new name",
                        RecoveryAction::RequestInput {
                            prompt: "Please provide a new name:".to_string(),
                            input_type: InputType::Text,
                        },
                        RiskLevel::Low,
                    )
                    .recommended(),
                    RecoveryOption::new(
                        "skip",
                        "Skip this operation",
                        RecoveryAction::Skip,
                        RiskLevel::None,
                    ),
                ],
                context: None,
            };
        }

        // Default to fatal error
        AgentError::Fatal {
            message: message.to_string(),
            source,
            context: context.unwrap_or_else(|| ErrorContext::new("unknown", source)),
            manual_recovery: None,
        }
    }

    /// Classify from a standard Error
    pub fn classify_std_error(
        &self,
        error: &dyn std::error::Error,
        source: ErrorSource,
    ) -> AgentError {
        self.classify(&error.to_string(), source, None)
    }
}

// ============================================================================
// Error Recovery Handler
// ============================================================================

/// Handles error recovery with retry logic and user prompts
pub struct ErrorRecoveryHandler {
    /// Error classifier
    classifier: ErrorClassifier,
    /// Maximum total retries across all errors
    max_total_retries: u32,
    /// Current total retry count (atomic for safe concurrent access via Arc)
    total_retries: AtomicU32,
}

impl Default for ErrorRecoveryHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ErrorRecoveryHandler {
    /// Create a new error recovery handler
    pub fn new() -> Self {
        Self {
            classifier: ErrorClassifier::new(),
            max_total_retries: 10,
            total_retries: AtomicU32::new(0),
        }
    }

    /// Set maximum total retries
    pub fn with_max_total_retries(mut self, max: u32) -> Self {
        self.max_total_retries = max;
        self
    }

    /// Get the classifier for direct access
    pub fn classifier(&self) -> &ErrorClassifier {
        &self.classifier
    }

    /// Handle an error and return the appropriate action
    pub fn handle(&self, error: &mut AgentError) -> RecoveryDecision {
        // Check total retry limit
        if self.total_retries.load(Ordering::Relaxed) >= self.max_total_retries {
            return RecoveryDecision::Abort {
                reason: "Maximum total retries exceeded".to_string(),
            };
        }

        match error {
            AgentError::Transient { .. } => {
                if error.increment_retry() {
                    let new_count = self.total_retries.fetch_add(1, Ordering::Relaxed) + 1;
                    RecoveryDecision::Retry {
                        delay_ms: self.calculate_backoff_ms(new_count),
                    }
                } else {
                    RecoveryDecision::Abort {
                        reason: format!("Max retries exceeded: {}", error.message()),
                    }
                }
            }

            AgentError::RateLimited { retry_after_ms, .. } => {
                self.total_retries.fetch_add(1, Ordering::Relaxed);
                RecoveryDecision::Retry {
                    delay_ms: Some(*retry_after_ms),
                }
            }

            AgentError::Ambiguous { options, .. } => RecoveryDecision::AskUser {
                options: options.clone(),
            },

            AgentError::Fatal { .. } => RecoveryDecision::Abort {
                reason: error.message().to_string(),
            },
        }
    }

    /// Calculate exponential backoff delay in milliseconds
    fn calculate_backoff_ms(&self, attempt: u32) -> Option<u64> {
        let base_ms: u64 = 1_000; // 1 second
        let max_ms: u64 = 30_000; // 30 seconds
        let delay_ms = (base_ms as f64) * 2.0_f64.powi(attempt as i32);
        Some((delay_ms as u64).min(max_ms))
    }

    /// Apply a recovery action chosen by the user
    pub fn apply_action(&self, action: &RecoveryAction) -> RecoveryDecision {
        match action {
            RecoveryAction::Retry { .. } => {
                self.total_retries.fetch_add(1, Ordering::Relaxed);
                RecoveryDecision::Retry { delay_ms: None }
            }
            RecoveryAction::Skip => RecoveryDecision::Skip,
            RecoveryAction::Alternative { tool, params } => RecoveryDecision::UseAlternative {
                tool: tool.clone(),
                params: params.clone(),
            },
            RecoveryAction::Rollback { checkpoint_id } => RecoveryDecision::Rollback {
                checkpoint_id: checkpoint_id.clone(),
            },
            RecoveryAction::Abort => RecoveryDecision::Abort {
                reason: "User requested abort".to_string(),
            },
            RecoveryAction::WaitAndRetry { delay_ms } => RecoveryDecision::Retry {
                delay_ms: Some(*delay_ms),
            },
            RecoveryAction::RequestInput { .. } => {
                // This should be handled by the caller
                RecoveryDecision::AskUser { options: vec![] }
            }
        }
    }

    /// Reset the retry counter
    pub fn reset(&self) {
        self.total_retries.store(0, Ordering::Relaxed);
    }
}

/// Decision on how to proceed after error handling
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecoveryDecision {
    /// Retry the operation
    Retry {
        /// Optional delay before retry in milliseconds
        #[serde(default)]
        delay_ms: Option<u64>,
    },
    /// Skip this operation and continue
    Skip,
    /// Use an alternative approach
    UseAlternative {
        /// Tool to use
        tool: String,
        /// Parameters
        params: serde_json::Value,
    },
    /// Rollback to a checkpoint
    Rollback {
        /// Checkpoint ID
        checkpoint_id: String,
    },
    /// Abort the operation
    Abort {
        /// Reason for abort
        reason: String,
    },
    /// Ask user for decision
    AskUser {
        /// Options to present
        options: Vec<RecoveryOption>,
    },
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_classification_transient() {
        let classifier = ErrorClassifier::new();
        let error = classifier.classify("Connection timeout", ErrorSource::Network, None);

        assert!(matches!(error, AgentError::Transient { .. }));
        assert!(error.is_retriable());
    }

    #[test]
    fn test_error_classification_rate_limit() {
        let classifier = ErrorClassifier::new();
        let error = classifier.classify("Rate limit exceeded", ErrorSource::External, None);

        assert!(matches!(error, AgentError::RateLimited { .. }));
        assert!(error.is_retriable());
    }

    #[test]
    fn test_error_classification_permission() {
        let classifier = ErrorClassifier::new();
        let error = classifier.classify("Permission denied", ErrorSource::Tool, None);

        assert!(matches!(error, AgentError::Ambiguous { .. }));
        assert!(error.requires_user_input());
    }

    #[test]
    fn test_error_classification_not_found() {
        let classifier = ErrorClassifier::new();
        let error = classifier.classify(
            "File not found: /tmp/test.txt",
            ErrorSource::FileSystem,
            None,
        );

        assert!(matches!(error, AgentError::Ambiguous { .. }));
        if let AgentError::Ambiguous { options, .. } = error {
            assert!(options.len() >= 2);
        }
    }

    #[test]
    fn test_error_classification_fatal() {
        let classifier = ErrorClassifier::new();
        let error = classifier.classify("Unknown critical error", ErrorSource::Unknown, None);

        assert!(matches!(error, AgentError::Fatal { .. }));
        assert!(error.is_fatal());
    }

    #[test]
    fn test_transient_retry_increment() {
        let mut error = AgentError::transient("test", ErrorSource::Network);

        // Should allow 3 retries
        assert!(error.increment_retry()); // 1
        assert!(error.increment_retry()); // 2
        assert!(error.increment_retry()); // 3
        assert!(!error.increment_retry()); // 4 - exceeds max
    }

    #[test]
    fn test_recovery_handler() {
        let handler = ErrorRecoveryHandler::new();
        let mut error = AgentError::transient("test", ErrorSource::Network);

        let decision = handler.handle(&mut error);
        assert!(matches!(decision, RecoveryDecision::Retry { .. }));
    }

    #[test]
    fn test_recovery_handler_max_total_retries() {
        let handler = ErrorRecoveryHandler::new().with_max_total_retries(2);

        let mut error1 = AgentError::transient("test1", ErrorSource::Network);
        assert!(matches!(
            handler.handle(&mut error1),
            RecoveryDecision::Retry { .. }
        ));

        let mut error2 = AgentError::transient("test2", ErrorSource::Network);
        assert!(matches!(
            handler.handle(&mut error2),
            RecoveryDecision::Retry { .. }
        ));

        let mut error3 = AgentError::transient("test3", ErrorSource::Network);
        assert!(matches!(
            handler.handle(&mut error3),
            RecoveryDecision::Abort { .. }
        ));
    }

    #[test]
    fn test_recovery_option() {
        let option = RecoveryOption::new(
            "retry",
            "Retry the operation",
            RecoveryAction::Retry {
                modified_params: None,
            },
            RiskLevel::Low,
        )
        .recommended();

        assert_eq!(option.id, "retry");
        assert!(option.recommended);
        assert_eq!(option.risk_level, RiskLevel::Low);
    }

    #[test]
    fn test_error_context() {
        let context = ErrorContext::new("file_write", ErrorSource::FileSystem)
            .with_tool("Write", serde_json::json!({"path": "/tmp/test.txt"}))
            .with_breadcrumb("Started write operation")
            .with_checkpoint("chk_123");

        assert_eq!(context.operation, "file_write");
        assert_eq!(context.tool_name, Some("Write".to_string()));
        assert_eq!(context.breadcrumbs.len(), 1);
        assert_eq!(context.checkpoint_id, Some("chk_123".to_string()));
    }

    #[test]
    fn test_risk_level_ordering() {
        assert!(RiskLevel::None < RiskLevel::Low);
        assert!(RiskLevel::Low < RiskLevel::Medium);
        assert!(RiskLevel::Medium < RiskLevel::High);
    }
}
