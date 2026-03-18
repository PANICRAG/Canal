//! HTTP error mapping for devtools-server.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use devtools_core::DevtoolsError;
use serde::Serialize;

/// API error response body.
#[derive(Serialize)]
pub struct ApiError {
    pub error: ApiErrorDetail,
}

#[derive(Serialize)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.error.code.as_str() {
            "not_found" => StatusCode::NOT_FOUND,
            "already_exists" => StatusCode::CONFLICT,
            "invalid_input" => StatusCode::UNPROCESSABLE_ENTITY,
            "unauthorized" => StatusCode::UNAUTHORIZED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let body = serde_json::to_string(&self).unwrap_or_else(|_| {
            r#"{"error":{"code":"internal","message":"Failed to serialize error"}}"#.into()
        });

        (status, [("content-type", "application/json")], body).into_response()
    }
}

impl From<DevtoolsError> for ApiError {
    fn from(err: DevtoolsError) -> Self {
        let code = match &err {
            DevtoolsError::TraceNotFound { .. } => "not_found",
            DevtoolsError::ObservationNotFound { .. } => "not_found",
            DevtoolsError::SessionNotFound { .. } => "not_found",
            DevtoolsError::ProjectNotFound { .. } => "not_found",
            DevtoolsError::ProjectAlreadyExists { .. } => "already_exists",
            DevtoolsError::InvalidInput { .. } => "invalid_input",
            DevtoolsError::Unauthorized { .. } => "unauthorized",
            DevtoolsError::Internal(_) => "internal",
        };

        ApiError {
            error: ApiErrorDetail {
                code: code.into(),
                message: err.to_string(),
            },
        }
    }
}
