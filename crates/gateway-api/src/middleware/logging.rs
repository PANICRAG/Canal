//! Request/response logging middleware

// Wired in main.rs router (Phase 3)

use axum::{extract::Request, middleware::Next, response::Response};
use std::time::Instant;
use tracing::{info, warn};

/// Request logging middleware
pub async fn logging_middleware(request: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = request.method().clone();
    let uri = request.uri().clone();
    let version = request.version();

    // Extract request ID if present
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Log incoming request
    info!(
        request_id = %request_id,
        method = %method,
        uri = %uri,
        version = ?version,
        "Incoming request"
    );

    // Process request
    let response = next.run(request).await;

    // Calculate duration
    let duration = start.elapsed();
    let status = response.status();

    // Log response
    if status.is_success() {
        info!(
            request_id = %request_id,
            method = %method,
            uri = %uri,
            status = %status.as_u16(),
            duration_ms = %duration.as_millis(),
            "Request completed"
        );
    } else if status.is_client_error() {
        warn!(
            request_id = %request_id,
            method = %method,
            uri = %uri,
            status = %status.as_u16(),
            duration_ms = %duration.as_millis(),
            "Client error"
        );
    } else {
        warn!(
            request_id = %request_id,
            method = %method,
            uri = %uri,
            status = %status.as_u16(),
            duration_ms = %duration.as_millis(),
            "Server error"
        );
    }

    response
}

/// Record Prometheus metrics for a completed request.
pub fn record_request_metrics(method: &str, path: &str, status: u16, duration_ms: u64) {
    let status_str = status.to_string();
    metrics::counter!("http_requests_total", "method" => method.to_string(), "path" => path.to_string(), "status" => status_str.clone()).increment(1);
    metrics::histogram!("http_request_duration_seconds", "method" => method.to_string(), "path" => path.to_string()).record(duration_ms as f64 / 1000.0);
    if status >= 400 {
        metrics::counter!("http_request_errors_total", "method" => method.to_string(), "status" => status_str).increment(1);
    }
}
