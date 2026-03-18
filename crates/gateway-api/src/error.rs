//! API error handling

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// API error type
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    /// Create a new API error
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// Create a not found error
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    /// Create a bad request error
    #[allow(dead_code)]
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    /// Create an internal server error
    #[allow(dead_code)]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    /// Create an unauthorized error
    #[allow(dead_code)]
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Unauthorized")
    }

    /// Create a forbidden error
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    /// Create a service unavailable error
    #[allow(dead_code)]
    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({
            "error": {
                "message": self.message,
                "code": self.status.as_u16()
            }
        }));
        (self.status, body).into_response()
    }
}

impl From<gateway_core::Error> for ApiError {
    fn from(err: gateway_core::Error) -> Self {
        use gateway_core::Error;

        let (status, message) = match &err {
            // 404 Not Found
            Error::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),

            // 403 Forbidden
            Error::PermissionDenied(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            Error::PathBlocked(msg) => (StatusCode::FORBIDDEN, format!("Access denied: {}", msg)),
            Error::CommandBlocked(msg) => (
                StatusCode::FORBIDDEN,
                format!("Command not allowed: {}", msg),
            ),

            // 401 Unauthorized
            Error::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized".to_string()),

            // 429 Too Many Requests
            Error::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "Rate limited".to_string()),

            // 400 Bad Request — sanitize config errors to avoid leaking internal paths
            Error::Config(_msg) => (StatusCode::BAD_REQUEST, "Configuration error".to_string()),
            Error::InvalidInput(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            Error::UnsupportedLanguage(lang) => (
                StatusCode::BAD_REQUEST,
                format!("Language not supported: {}", lang),
            ),

            // 413 Payload Too Large
            Error::FileTooLarge { size, limit } => (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "File too large: {} bytes exceeds limit of {} bytes",
                    size, limit
                ),
            ),

            // 408 Request Timeout
            Error::Timeout(msg) => (StatusCode::REQUEST_TIMEOUT, msg.clone()),

            // 500 Internal Server Error (with logging)
            Error::Docker(msg) => {
                tracing::error!(error = %msg, "Docker error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Execution service error".to_string(),
                )
            }
            Error::ExecutionFailed(msg) => {
                tracing::error!(error = %msg, "Execution failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Execution failed".to_string(),
                )
            }
            Error::Database(e) => {
                tracing::error!(error = %e, "Database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                )
            }
            Error::Http(e) => {
                tracing::error!(error = %e, "HTTP error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "External service error".to_string(),
                )
            }
            _ => {
                tracing::error!(error = %err, "Internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal error".to_string(),
                )
            }
        };

        Self { status, message }
    }
}

impl From<gateway_tools::ServiceError> for ApiError {
    fn from(err: gateway_tools::ServiceError) -> Self {
        // Convert through gateway-core's Error for consistent status mapping
        Self::from(gateway_core::Error::from(err))
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        tracing::error!(error = %err, "Database error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Database error".to_string(),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        tracing::error!(error = %err, "Internal error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Internal error".to_string(),
        }
    }
}

impl From<gateway_orchestrator::OrchestratorError> for ApiError {
    fn from(err: gateway_orchestrator::OrchestratorError) -> Self {
        use gateway_orchestrator::OrchestratorError;

        let (status, message) = match &err {
            OrchestratorError::ContainerNotFound(id) => (
                StatusCode::NOT_FOUND,
                format!("Container not found: {}", id),
            ),
            OrchestratorError::InvalidState {
                id,
                state,
                operation,
            } => (
                StatusCode::CONFLICT,
                format!(
                    "Container {} is in state '{}', cannot {}",
                    id, state, operation
                ),
            ),
            OrchestratorError::InvalidRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            OrchestratorError::QuotaExceeded(msg) => (StatusCode::TOO_MANY_REQUESTS, msg.clone()),
            OrchestratorError::Timeout(msg) => (
                StatusCode::GATEWAY_TIMEOUT,
                format!("Container operation timed out: {}", msg),
            ),
            OrchestratorError::GrpcConnection(msg) => {
                tracing::error!(error = %msg, "gRPC connection error");
                (
                    StatusCode::BAD_GATEWAY,
                    "Worker connection failed".to_string(),
                )
            }
            OrchestratorError::Config(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Configuration error: {}", msg),
            ),
            OrchestratorError::Kubernetes(e) => {
                tracing::error!(error = %e, "Kubernetes error");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Container orchestration unavailable".to_string(),
                )
            }
            OrchestratorError::Database(e) => {
                tracing::error!(error = %e, "Database error in orchestrator");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                )
            }
            OrchestratorError::Internal(msg) => {
                tracing::error!(error = %msg, "Internal orchestrator error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal error".to_string(),
                )
            }
        };

        Self { status, message }
    }
}
