//! DevTools error types

/// Unified error type for devtools-core operations.
#[derive(Debug, thiserror::Error)]
pub enum DevtoolsError {
    #[error("trace not found: {id}")]
    TraceNotFound { id: String },

    #[error("observation not found: {id}")]
    ObservationNotFound { id: String },

    #[error("session not found: {id}")]
    SessionNotFound { id: String },

    #[error("project not found: {id}")]
    ProjectNotFound { id: String },

    #[error("project already exists: {id}")]
    ProjectAlreadyExists { id: String },

    #[error("invalid input: {message}")]
    InvalidInput { message: String },

    #[error("unauthorized: {message}")]
    Unauthorized { message: String },

    #[error("internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = DevtoolsError::TraceNotFound {
            id: "tr-123".into(),
        };
        assert_eq!(err.to_string(), "trace not found: tr-123");
    }

    #[test]
    fn test_project_not_found_msg() {
        let err = DevtoolsError::ProjectNotFound {
            id: "proj-1".into(),
        };
        assert_eq!(err.to_string(), "project not found: proj-1");
    }

    #[test]
    fn test_unauthorized_msg() {
        let err = DevtoolsError::Unauthorized {
            message: "invalid API key".into(),
        };
        assert_eq!(err.to_string(), "unauthorized: invalid API key");
    }
}
