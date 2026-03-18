//! Code execution endpoints
//!
//! Provides API routes for executing code in Docker containers.
//! Supports Python and Bash execution with streaming output.

use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use futures::stream::Stream;
use gateway_core::executor::{ExecutionEvent, ExecutionRequest, ExecutionResult, Language};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::{error::ApiError, state::AppState};

/// Create the code execution routes
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/execute", post(execute_code))
        .route("/execute/stream", post(execute_code_stream))
        .route("/languages", get(list_languages))
        .route("/health", get(executor_health))
}

/// Code execution request from API
#[derive(Debug, Deserialize)]
pub struct ApiExecuteRequest {
    /// Programming language (python, bash)
    pub language: String,
    /// Code to execute
    pub code: String,
    /// Optional timeout in milliseconds
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Whether to stream output (use /execute/stream endpoint instead)
    #[serde(default)]
    #[allow(dead_code)]
    pub stream: bool,
    /// Optional working directory (must be in allowed directories)
    #[serde(default)]
    pub working_dir: Option<String>,
}

/// Code execution response
#[derive(Debug, Serialize)]
pub struct ApiExecuteResponse {
    /// Unique execution ID
    pub execution_id: String,
    /// Language used
    pub language: String,
    /// Combined stdout
    pub stdout: String,
    /// Combined stderr
    pub stderr: String,
    /// Exit code (0 = success)
    pub exit_code: i32,
    /// Execution status
    pub status: String,
    /// Duration in milliseconds
    pub duration_ms: u64,
}

impl From<ExecutionResult> for ApiExecuteResponse {
    fn from(result: ExecutionResult) -> Self {
        Self {
            execution_id: result.execution_id,
            language: result.language.to_string(),
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
            status: format!("{:?}", result.status).to_lowercase(),
            duration_ms: result.duration_ms,
        }
    }
}

/// Supported language info
#[derive(Debug, Serialize)]
pub struct LanguageInfo {
    pub name: String,
    pub enabled: bool,
    pub timeout_ms: u64,
    pub docker_image: String,
}

/// Language list response
#[derive(Debug, Serialize)]
pub struct LanguagesResponse {
    pub languages: Vec<LanguageInfo>,
}

/// Health check response
#[derive(Debug, Serialize)]
pub struct ExecutorHealthResponse {
    pub healthy: bool,
    pub docker_available: bool,
    pub message: String,
}

/// Execute code (non-streaming)
pub async fn execute_code(
    State(state): State<AppState>,
    Json(request): Json<ApiExecuteRequest>,
) -> Result<Json<ApiExecuteResponse>, ApiError> {
    // R4-DM2: Reject oversized code payloads (1MB limit)
    const MAX_CODE_SIZE: usize = 1024 * 1024;
    if request.code.len() > MAX_CODE_SIZE {
        return Err(ApiError::bad_request(format!(
            "Code payload too large: {} bytes (max {})",
            request.code.len(),
            MAX_CODE_SIZE,
        )));
    }

    let executor = state.code_executor.as_ref().ok_or_else(|| {
        ApiError::internal("Code executor not available. Docker may not be running.")
    })?;

    // Parse language
    let language: Language = request
        .language
        .parse()
        .map_err(|e| ApiError::bad_request(format!("Invalid language: {}", e)))?;

    // Check if language is enabled
    if !executor.is_language_enabled(language) {
        return Err(ApiError::bad_request(format!(
            "Language '{}' is not enabled",
            language
        )));
    }

    tracing::info!(
        language = %language,
        code_length = request.code.len(),
        "Executing code"
    );

    let exec_request = ExecutionRequest {
        code: request.code,
        language,
        timeout_ms: request.timeout_ms,
        stream: false,
        working_dir: request.working_dir,
    };

    let result = executor.execute(exec_request).await.map_err(|e| {
        tracing::error!(error = %e, "Code execution failed");
        ApiError::internal(format!("Execution failed: {}", e))
    })?;

    tracing::info!(
        execution_id = %result.execution_id,
        exit_code = result.exit_code,
        duration_ms = result.duration_ms,
        "Code execution completed"
    );

    Ok(Json(result.into()))
}

/// Execute code with streaming output
pub async fn execute_code_stream(
    State(state): State<AppState>,
    Json(request): Json<ApiExecuteRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    // R4-DM2: Reject oversized code payloads (1MB limit)
    const MAX_CODE_SIZE: usize = 1024 * 1024;
    if request.code.len() > MAX_CODE_SIZE {
        return Err(ApiError::bad_request(format!(
            "Code payload too large: {} bytes (max {})",
            request.code.len(),
            MAX_CODE_SIZE,
        )));
    }

    let executor = state.code_executor.clone().ok_or_else(|| {
        ApiError::internal("Code executor not available. Docker may not be running.")
    })?;

    // Parse language
    let language: Language = request
        .language
        .parse()
        .map_err(|e| ApiError::bad_request(format!("Invalid language: {}", e)))?;

    // Check if language is enabled
    if !executor.is_language_enabled(language) {
        return Err(ApiError::bad_request(format!(
            "Language '{}' is not enabled",
            language
        )));
    }

    tracing::info!(
        language = %language,
        code_length = request.code.len(),
        "Starting streaming code execution"
    );

    let (tx, rx) = mpsc::channel::<ExecutionEvent>(100);

    let exec_request = ExecutionRequest {
        code: request.code,
        language,
        timeout_ms: request.timeout_ms,
        stream: true,
        working_dir: request.working_dir,
    };

    // Spawn task to execute code
    tokio::spawn(async move {
        if let Err(e) = executor.execute_streaming(exec_request, tx).await {
            tracing::error!(error = %e, "Streaming execution failed");
        }
    });

    // Convert mpsc stream to SSE events
    let stream = ReceiverStream::new(rx).map(|event| {
        let json = serde_json::to_string(&event).unwrap_or_default();
        Ok(Event::default().data(json))
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// List supported languages
pub async fn list_languages(
    State(state): State<AppState>,
) -> Result<Json<LanguagesResponse>, ApiError> {
    let languages = match &state.code_executor {
        Some(executor) => {
            vec![
                LanguageInfo {
                    name: "python".to_string(),
                    enabled: executor.is_language_enabled(Language::Python),
                    timeout_ms: 30000,
                    docker_image: "python:3.11-slim".to_string(),
                },
                LanguageInfo {
                    name: "bash".to_string(),
                    enabled: executor.is_language_enabled(Language::Bash),
                    timeout_ms: 10000,
                    docker_image: "ubuntu:22.04".to_string(),
                },
                LanguageInfo {
                    name: "javascript".to_string(),
                    enabled: executor.is_language_enabled(Language::JavaScript),
                    timeout_ms: 30000,
                    docker_image: "node:20-alpine".to_string(),
                },
                LanguageInfo {
                    name: "typescript".to_string(),
                    enabled: executor.is_language_enabled(Language::TypeScript),
                    timeout_ms: 30000,
                    docker_image: "node:20-alpine".to_string(),
                },
                LanguageInfo {
                    name: "go".to_string(),
                    enabled: executor.is_language_enabled(Language::Go),
                    timeout_ms: 60000,
                    docker_image: "golang:1.22-alpine".to_string(),
                },
                LanguageInfo {
                    name: "rust".to_string(),
                    enabled: executor.is_language_enabled(Language::Rust),
                    timeout_ms: 120000,
                    docker_image: "rust:1.75-slim".to_string(),
                },
            ]
        }
        None => {
            vec![
                LanguageInfo {
                    name: "python".to_string(),
                    enabled: false,
                    timeout_ms: 30000,
                    docker_image: "python:3.11-slim".to_string(),
                },
                LanguageInfo {
                    name: "bash".to_string(),
                    enabled: false,
                    timeout_ms: 10000,
                    docker_image: "ubuntu:22.04".to_string(),
                },
                LanguageInfo {
                    name: "javascript".to_string(),
                    enabled: false,
                    timeout_ms: 30000,
                    docker_image: "node:20-alpine".to_string(),
                },
                LanguageInfo {
                    name: "typescript".to_string(),
                    enabled: false,
                    timeout_ms: 30000,
                    docker_image: "node:20-alpine".to_string(),
                },
                LanguageInfo {
                    name: "go".to_string(),
                    enabled: false,
                    timeout_ms: 60000,
                    docker_image: "golang:1.22-alpine".to_string(),
                },
                LanguageInfo {
                    name: "rust".to_string(),
                    enabled: false,
                    timeout_ms: 120000,
                    docker_image: "rust:1.75-slim".to_string(),
                },
            ]
        }
    };

    Ok(Json(LanguagesResponse { languages }))
}

/// Check executor health
pub async fn executor_health(
    State(state): State<AppState>,
) -> Result<Json<ExecutorHealthResponse>, ApiError> {
    match &state.code_executor {
        Some(executor) => {
            let healthy = executor.health_check().await.unwrap_or(false);
            Ok(Json(ExecutorHealthResponse {
                healthy,
                docker_available: healthy,
                message: if healthy {
                    "Code executor is operational".to_string()
                } else {
                    "Docker daemon is not available".to_string()
                },
            }))
        }
        None => Ok(Json(ExecutorHealthResponse {
            healthy: false,
            docker_available: false,
            message: "Code executor not initialized".to_string(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_core::executor::ExecutionStatus;

    #[test]
    fn test_api_execute_response_from_result() {
        let result = ExecutionResult {
            execution_id: "test-123".to_string(),
            language: Language::Python,
            stdout: "Hello, World!".to_string(),
            stderr: "".to_string(),
            exit_code: 0,
            status: ExecutionStatus::Success,
            duration_ms: 150,
        };

        let response: ApiExecuteResponse = result.into();
        assert_eq!(response.execution_id, "test-123");
        assert_eq!(response.language, "python");
        assert_eq!(response.stdout, "Hello, World!");
        assert_eq!(response.exit_code, 0);
        assert_eq!(response.status, "success");
    }
}
