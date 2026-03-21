//! Tool error types shared between gateway-core and gateway-tools.

use std::fmt;

/// Error type for tool execution
#[derive(Debug, Clone)]
pub enum ToolError {
    /// Invalid input provided
    InvalidInput(String),
    /// Permission denied
    PermissionDenied(String),
    /// Operation needs user permission
    NeedsPermission(String),
    /// Operation timed out
    Timeout(String),
    /// Operation was cancelled
    Cancelled(String),
    /// Internal execution error
    ExecutionError(String),
    /// Resource not found
    NotFound(String),
    /// IO error
    IoError(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
            Self::PermissionDenied(msg) => write!(f, "Permission denied: {}", msg),
            Self::NeedsPermission(msg) => write!(f, "Permission required: {}", msg),
            Self::Timeout(msg) => write!(f, "Timeout: {}", msg),
            Self::Cancelled(msg) => write!(f, "Cancelled: {}", msg),
            Self::ExecutionError(msg) => write!(f, "Execution error: {}", msg),
            Self::NotFound(msg) => write!(f, "Not found: {}", msg),
            Self::IoError(msg) => write!(f, "IO error: {}", msg),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e.to_string())
    }
}

/// Result type for tool execution
pub type ToolResult<T> = Result<T, ToolError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_error_display() {
        let err = ToolError::InvalidInput("missing field".to_string());
        assert_eq!(err.to_string(), "Invalid input: missing field");

        let err = ToolError::PermissionDenied("write not allowed".to_string());
        assert_eq!(err.to_string(), "Permission denied: write not allowed");
    }

    #[test]
    fn test_tool_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let tool_err = ToolError::from(io_err);
        assert!(matches!(tool_err, ToolError::IoError(_)));
    }

    #[test]
    fn test_tool_result_ok() {
        let result: ToolResult<i32> = Ok(42);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn test_tool_result_err() {
        let result: ToolResult<i32> = Err(ToolError::NotFound("nope".into()));
        assert!(result.is_err());
    }
}
