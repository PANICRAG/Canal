//! Request ID middleware for structured logging.
//!
//! Generates a unique correlation ID for each request and adds it to:
//! - The tracing span (for structured log correlation)
//! - The response headers (`X-Request-Id`)
//! - Request extensions (for downstream middleware/handlers)

use axum::{
    extract::Request,
    http::{header::HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use uuid::Uuid;

/// Request ID stored in request extensions for downstream access.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

/// Request ID middleware.
///
/// Generates a UUID v4 for each request and:
/// 1. Stores it in request extensions as `RequestId`
/// 2. Creates a tracing span with `request_id` field
/// 3. Adds `X-Request-Id` response header
///
/// If the client sends an `X-Request-Id` header, that value is used instead.
pub async fn request_id_middleware(mut request: Request, next: Next) -> Response {
    // Use client-provided request ID or generate a new one
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    // Store in extensions for downstream access
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));

    // Create tracing span with request metadata
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let span = tracing::info_span!(
        "request",
        request_id = %request_id,
        method = %method,
        path = %path,
    );

    let _guard = span.enter();

    let mut response = next.run(request).await;

    // Add request ID to response headers
    if let Ok(v) = HeaderValue::from_str(&request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), v);
    }

    response
}
