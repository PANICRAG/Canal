//! Log query endpoints — Loki proxy for unified log access.

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

use crate::error::{ApiError, ApiErrorDetail};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Build a full Loki API URL from the base URL and path.
fn loki_url(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

/// Helper to create an internal ApiError from a message.
fn internal_error(msg: impl Into<String>) -> ApiError {
    ApiError {
        error: ApiErrorDetail {
            code: "internal".into(),
            message: msg.into(),
        },
    }
}

/// Helper to create a bad-request ApiError.
fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError {
        error: ApiErrorDetail {
            code: "invalid_input".into(),
            message: msg.into(),
        },
    }
}

/// Dangerous patterns that must not appear in LogQL queries.
const BLOCKED_LOGQL_PATTERNS: &[&str] = &["/loki/api/v1/admin", "delete", "__name__"];

/// Validate a LogQL query string, rejecting admin/dangerous patterns.
fn validate_logql(query: &str) -> Result<(), ApiError> {
    let query_lower = query.to_lowercase();
    for pattern in BLOCKED_LOGQL_PATTERNS {
        if query_lower.contains(pattern) {
            return Err(bad_request(format!(
                "Query contains blocked pattern: {pattern}"
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// POST /v1/logs/query — LogQL query proxy
// ---------------------------------------------------------------------------

/// Request body for LogQL range queries.
#[derive(Debug, Deserialize)]
pub struct LogQueryRequest {
    /// LogQL expression (e.g. `{job="docker"} |= "error"`).
    pub query: String,
    /// Range start (RFC3339 timestamp or relative like `1h`).
    pub start: Option<String>,
    /// Range end (RFC3339 timestamp).
    pub end: Option<String>,
    /// Maximum number of log entries to return (default 100).
    pub limit: Option<u32>,
    /// Sort direction: "forward" (oldest first) or "backward" (newest first).
    pub direction: Option<String>,
}

/// POST /v1/logs/query — proxy a LogQL range query to Loki.
pub async fn query_logs(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LogQueryRequest>,
) -> Result<impl IntoResponse, ApiError> {
    info!(query = %req.query, "LogQL query requested");

    validate_logql(&req.query)?;

    let url = loki_url(&state.loki_url, "/loki/api/v1/query_range");

    let limit = req.limit.unwrap_or(100).min(5000);
    let direction = req.direction.as_deref().unwrap_or("backward");

    let mut query_pairs: Vec<(&str, String)> = vec![
        ("query", req.query),
        ("limit", limit.to_string()),
        ("direction", direction.to_string()),
    ];
    if let Some(start) = req.start {
        query_pairs.push(("start", start));
    }
    if let Some(end) = req.end {
        query_pairs.push(("end", end));
    }

    let resp = state
        .http_client
        .get(&url)
        .query(&query_pairs)
        .send()
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to reach Loki");
            internal_error(format!("Loki unreachable: {e}"))
        })?;

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        error!(error = %e, "Failed to parse Loki response");
        internal_error(format!("Invalid Loki response: {e}"))
    })?;

    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// GET /v1/logs/stream?query=... — SSE log tail proxy
// ---------------------------------------------------------------------------

/// Query parameters for the SSE log stream endpoint.
#[derive(Debug, Deserialize)]
pub struct LogStreamParams {
    /// LogQL expression to tail.
    pub query: String,
    /// Maximum delay in seconds between polls (default 2).
    pub delay_for: Option<u64>,
    /// Maximum number of entries per poll (default 100).
    pub limit: Option<u32>,
}

/// GET /v1/logs/stream — SSE proxy that polls Loki tail endpoint.
///
/// Converts Loki query_range responses into a continuous SSE stream by
/// polling at regular intervals. Each SSE event contains one stream entry.
pub async fn stream_logs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LogStreamParams>,
) -> Result<impl IntoResponse, ApiError> {
    info!(query = %params.query, "Log stream requested");

    validate_logql(&params.query)?;

    let poll_interval = Duration::from_secs(params.delay_for.unwrap_or(2).min(10));
    let limit = params.limit.unwrap_or(100).min(5000);
    let query = params.query.clone();
    let loki_base = state.loki_url.clone();
    let client = state.http_client.clone();

    let stream = async_stream::stream! {
        let mut last_timestamp_ns: u64 = 0;

        // Initial start: 30 seconds ago
        let start = chrono::Utc::now() - chrono::Duration::seconds(30);
        let mut start_ns = start.timestamp_nanos_opt().unwrap_or(0) as u64;

        loop {
            let url = loki_url(&loki_base, "/loki/api/v1/query_range");

            let resp = client
                .get(&url)
                .query(&[
                    ("query", query.as_str()),
                    ("limit", &limit.to_string()),
                    ("direction", "forward"),
                    ("start", &start_ns.to_string()),
                ])
                .send()
                .await;

            match resp {
                Ok(r) => {
                    if let Ok(body) = r.json::<serde_json::Value>().await {
                        if let Some(streams) = body["data"]["result"].as_array() {
                            for s in streams {
                                let labels = &s["stream"];
                                if let Some(values) = s["values"].as_array() {
                                    for v in values {
                                        // Each value is ["timestamp_ns", "log_line"]
                                        let ts_str = v[0].as_str().unwrap_or("0");
                                        let ts: u64 = ts_str.parse().unwrap_or(0);

                                        // Skip already-seen entries
                                        if ts <= last_timestamp_ns {
                                            continue;
                                        }
                                        last_timestamp_ns = ts;

                                        let entry = serde_json::json!({
                                            "stream": labels,
                                            "values": [[v[0], v[1]]]
                                        });

                                        yield Ok::<_, std::convert::Infallible>(Event::default().data(entry.to_string()));
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Loki poll failed, retrying");
                    yield Ok::<_, std::convert::Infallible>(Event::default().event("error").data(
                        serde_json::json!({"error": e.to_string()}).to_string()
                    ));
                }
            }

            // Move start forward for next poll
            if last_timestamp_ns > 0 {
                start_ns = last_timestamp_ns + 1;
            }

            tokio::time::sleep(poll_interval).await;
        }
    };

    Ok(Sse::new(Box::pin(stream)).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

// ---------------------------------------------------------------------------
// GET /v1/logs/labels — Loki label names
// ---------------------------------------------------------------------------

/// GET /v1/logs/labels — list all label names from Loki.
pub async fn list_labels(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    info!("Listing Loki labels");

    let url = loki_url(&state.loki_url, "/loki/api/v1/labels");

    let resp = state.http_client.get(&url).send().await.map_err(|e| {
        error!(error = %e, "Failed to reach Loki labels API");
        internal_error(format!("Loki unreachable: {e}"))
    })?;

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        error!(error = %e, "Failed to parse Loki labels response");
        internal_error(format!("Invalid Loki response: {e}"))
    })?;

    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// GET /v1/logs/label/:name/values — Loki label values
// ---------------------------------------------------------------------------

/// GET /v1/logs/label/:name/values — list values for a specific label.
pub async fn label_values(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    info!(label = %name, "Listing Loki label values");

    let url = loki_url(
        &state.loki_url,
        &format!("/loki/api/v1/label/{name}/values"),
    );

    let resp = state.http_client.get(&url).send().await.map_err(|e| {
        error!(error = %e, label = %name, "Failed to reach Loki label values API");
        internal_error(format!("Loki unreachable: {e}"))
    })?;

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        error!(error = %e, "Failed to parse Loki label values response");
        internal_error(format!("Invalid Loki response: {e}"))
    })?;

    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loki_url_builder() {
        assert_eq!(
            loki_url("http://localhost:3100", "/loki/api/v1/labels"),
            "http://localhost:3100/loki/api/v1/labels"
        );
        assert_eq!(
            loki_url("http://localhost:3100/", "/loki/api/v1/labels"),
            "http://localhost:3100/loki/api/v1/labels"
        );
    }

    #[test]
    fn test_validate_logql_blocks_admin() {
        assert!(validate_logql("{job=\"docker\"}").is_ok());
        assert!(validate_logql("{job=\"docker\"} |= \"error\"").is_ok());
        assert!(validate_logql("/loki/api/v1/admin/delete").is_err());
        assert!(validate_logql("DELETE something").is_err());
        assert!(validate_logql("__name__").is_err());
    }

    #[test]
    fn test_validate_logql_case_insensitive() {
        assert!(validate_logql("DELETE").is_err());
        assert!(validate_logql("Delete").is_err());
        assert!(validate_logql("__NAME__").is_err());
    }

    #[test]
    fn test_log_query_request_defaults() {
        let json = r#"{"query": "{job=\"docker\"}"}"#;
        let req: LogQueryRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.query, "{job=\"docker\"}");
        assert!(req.start.is_none());
        assert!(req.end.is_none());
        assert!(req.limit.is_none());
        assert!(req.direction.is_none());
    }

    #[test]
    fn test_log_query_request_full() {
        let json = r#"{
            "query": "{job=\"docker\"} |= \"error\"",
            "start": "2026-03-01T00:00:00Z",
            "end": "2026-03-07T00:00:00Z",
            "limit": 500,
            "direction": "forward"
        }"#;
        let req: LogQueryRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.limit, Some(500));
        assert_eq!(req.direction.as_deref(), Some("forward"));
    }

    #[test]
    fn test_log_stream_params_deserialize() {
        let json = r#"{"query": "{job=\"docker\"}"}"#;
        let params: LogStreamParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.query, "{job=\"docker\"}");
        assert!(params.delay_for.is_none());
        assert!(params.limit.is_none());
    }
}
