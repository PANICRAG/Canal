//! Bearer token authentication middleware for devtools-server.
//!
//! Validates `Authorization: Bearer <api_key>` header against the DEVTOOLS_API_KEY env var.

use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};

/// Validate Bearer token against DEVTOOLS_API_KEY environment variable.
/// Health endpoint should be excluded from this middleware.
pub async fn require_auth(req: Request<Body>, next: Next) -> Result<Response, Response> {
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let api_key = match auth_header {
        Some(key) if !key.is_empty() => key,
        _ => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "Missing or invalid Authorization header. Use: Bearer <api_key>"
                })),
            )
                .into_response());
        }
    };

    let expected_key = std::env::var("DEVTOOLS_API_KEY").unwrap_or_default();
    if expected_key.is_empty() {
        // No key configured — allow all requests in development
        return Ok(next.run(req).await);
    }

    if api_key == expected_key {
        Ok(next.run(req).await)
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Invalid API key"
            })),
        )
            .into_response())
    }
}
