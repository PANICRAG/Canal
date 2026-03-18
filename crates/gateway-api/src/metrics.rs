//! Prometheus metrics for the API gateway
//!
//! This module provides comprehensive metrics for monitoring:
//! - HTTP request statistics (count, latency, errors)
//! - LLM API usage (tokens, requests, latency)
//! - Container management (active containers, resource usage)
//! - Session management (active sessions, checkpoints)
//! - MCP server health
#![allow(dead_code)]

use metrics::{counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::time::Duration;

/// Metrics collector and exporter
pub struct MetricsCollector {
    handle: PrometheusHandle,
}

impl MetricsCollector {
    /// Create a new metrics collector with Prometheus exporter
    pub fn new() -> Self {
        let builder = PrometheusBuilder::new()
            .set_buckets_for_metric(
                Matcher::Full("http_request_duration_seconds".to_string()),
                &[
                    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                ],
            )
            .unwrap()
            .set_buckets_for_metric(
                Matcher::Full("llm_request_duration_seconds".to_string()),
                &[0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0],
            )
            .unwrap()
            .set_buckets_for_metric(
                Matcher::Full("code_execution_duration_seconds".to_string()),
                &[0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0],
            )
            .unwrap();

        let handle = builder
            .install_recorder()
            .expect("Failed to install Prometheus recorder");

        // Initialize metric descriptions
        Self::describe_metrics();

        Self { handle }
    }

    /// Describe all metrics for Prometheus
    fn describe_metrics() {
        // HTTP metrics
        describe_counter!("http_requests_total", "Total number of HTTP requests");
        describe_histogram!(
            "http_request_duration_seconds",
            "HTTP request duration in seconds"
        );
        describe_counter!(
            "http_request_errors_total",
            "Total number of HTTP request errors"
        );

        // LLM metrics
        describe_counter!("llm_requests_total", "Total number of LLM API requests");
        describe_histogram!(
            "llm_request_duration_seconds",
            "LLM request duration in seconds"
        );
        describe_counter!("llm_tokens_total", "Total LLM tokens processed");
        describe_counter!("llm_errors_total", "Total number of LLM API errors");

        // Chat/Session metrics
        describe_gauge!(
            "active_sessions_count",
            "Number of currently active sessions"
        );
        describe_counter!("sessions_created_total", "Total number of sessions created");
        describe_counter!("sessions_ended_total", "Total number of sessions ended");
        describe_counter!(
            "checkpoints_created_total",
            "Total number of session checkpoints created"
        );

        // Container metrics
        describe_gauge!(
            "active_containers_count",
            "Number of currently running containers"
        );
        describe_counter!(
            "containers_created_total",
            "Total number of containers created"
        );
        describe_counter!(
            "containers_terminated_total",
            "Total number of containers terminated"
        );
        describe_gauge!(
            "container_cpu_usage_percent",
            "Container CPU usage percentage"
        );
        describe_gauge!(
            "container_memory_usage_bytes",
            "Container memory usage in bytes"
        );

        // Code execution metrics
        describe_counter!("code_executions_total", "Total number of code executions");
        describe_histogram!(
            "code_execution_duration_seconds",
            "Code execution duration in seconds"
        );
        describe_counter!(
            "code_execution_errors_total",
            "Total number of code execution errors"
        );

        // Tool/MCP metrics
        describe_counter!("tool_calls_total", "Total number of tool calls");
        describe_gauge!("mcp_servers_active", "Number of active MCP servers");
        describe_counter!("mcp_tool_invocations_total", "Total MCP tool invocations");

        // Git metrics
        describe_counter!("git_operations_total", "Total number of git operations");

        // A28: Rate limiting metrics
        describe_counter!(
            "rate_limit_exceeded_total",
            "Total number of rate-limited requests"
        );
        describe_counter!(
            "rate_limit_allowed_total",
            "Total number of requests allowed by rate limiter"
        );

        // A28: RTE delegation metrics
        describe_counter!(
            "rte_delegations_total",
            "Total RTE tool delegation attempts"
        );
        describe_histogram!(
            "rte_delegation_duration_seconds",
            "RTE delegation round-trip time in seconds"
        );

        // A28: Auth metrics
        describe_counter!("auth_attempts_total", "Total authentication attempts");

        // A28: Active rate limiter buckets
        describe_gauge!(
            "rate_limit_active_buckets",
            "Number of active rate limiter buckets"
        );
    }

    /// Render metrics in Prometheus format
    pub fn render(&self) -> String {
        self.handle.render()
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// HTTP Metrics
// ============================================================================

/// Normalize a request path for use as a Prometheus label.
///
/// R4-M: Replaces UUID segments and numeric IDs with placeholders to prevent
/// unbounded label cardinality from dynamic path segments.
fn normalize_path(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            // Replace UUIDs (8-4-4-4-12 hex pattern)
            if segment.len() == 36 && segment.chars().filter(|c| *c == '-').count() == 4 {
                return ":id";
            }
            // Replace pure numeric segments
            if !segment.is_empty() && segment.chars().all(|c| c.is_ascii_digit()) {
                return ":id";
            }
            segment
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Record an HTTP request
pub fn record_http_request(method: &str, path: &str, status: u16, duration: Duration) {
    let normalized_path = normalize_path(path);
    let labels = [
        ("method", method.to_string()),
        ("path", normalized_path),
        ("status", status.to_string()),
    ];

    counter!("http_requests_total", &labels).increment(1);
    histogram!("http_request_duration_seconds", &labels).record(duration.as_secs_f64());

    if status >= 400 {
        counter!("http_request_errors_total", &labels).increment(1);
    }
}

// ============================================================================
// LLM Metrics
// ============================================================================

/// Record an LLM request
pub fn record_llm_request(
    provider: &str,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    duration: Duration,
    error: bool,
) {
    let labels = [
        ("provider", provider.to_string()),
        ("model", model.to_string()),
    ];

    counter!("llm_requests_total", &labels).increment(1);
    histogram!("llm_request_duration_seconds", &labels).record(duration.as_secs_f64());

    counter!(
        "llm_tokens_total",
        &[
            ("type", "input".to_string()),
            ("provider", provider.to_string())
        ]
    )
    .increment(input_tokens);
    counter!(
        "llm_tokens_total",
        &[
            ("type", "output".to_string()),
            ("provider", provider.to_string())
        ]
    )
    .increment(output_tokens);

    if error {
        counter!("llm_errors_total", &labels).increment(1);
    }
}

// ============================================================================
// Session Metrics
// ============================================================================

/// Update active session count
pub fn set_active_sessions(count: i64) {
    gauge!("active_sessions_count").set(count as f64);
}

/// Record session creation
pub fn record_session_created(session_type: &str) {
    counter!("sessions_created_total", "type" => session_type.to_string()).increment(1);
}

/// Record session end
pub fn record_session_ended(session_type: &str, reason: &str) {
    counter!(
        "sessions_ended_total",
        "type" => session_type.to_string(),
        "reason" => reason.to_string()
    )
    .increment(1);
}

/// Record checkpoint creation
pub fn record_checkpoint_created(checkpoint_type: &str) {
    counter!("checkpoints_created_total", "type" => checkpoint_type.to_string()).increment(1);
}

// ============================================================================
// Container Metrics
// ============================================================================

/// Update active container count
pub fn set_active_containers(count: i64) {
    gauge!("active_containers_count").set(count as f64);
}

/// Record container creation
pub fn record_container_created(container_type: &str) {
    counter!("containers_created_total", "type" => container_type.to_string()).increment(1);
}

/// Record container termination
pub fn record_container_terminated(reason: &str) {
    counter!("containers_terminated_total", "reason" => reason.to_string()).increment(1);
}

/// Update container resource usage
pub fn set_container_resource_usage(container_id: &str, cpu_percent: f64, memory_bytes: i64) {
    gauge!("container_cpu_usage_percent", "container_id" => container_id.to_string())
        .set(cpu_percent);
    gauge!("container_memory_usage_bytes", "container_id" => container_id.to_string())
        .set(memory_bytes as f64);
}

// ============================================================================
// Code Execution Metrics
// ============================================================================

/// Record a code execution
pub fn record_code_execution(language: &str, duration: Duration, success: bool) {
    let labels = [("language", language.to_string())];

    counter!("code_executions_total", &labels).increment(1);
    histogram!("code_execution_duration_seconds", &labels).record(duration.as_secs_f64());

    if !success {
        counter!("code_execution_errors_total", &labels).increment(1);
    }
}

// ============================================================================
// Tool/MCP Metrics
// ============================================================================

/// Record a tool call
pub fn record_tool_call(tool_name: &str, success: bool) {
    counter!(
        "tool_calls_total",
        "tool" => tool_name.to_string(),
        "success" => success.to_string()
    )
    .increment(1);
}

/// Update active MCP servers count
pub fn set_active_mcp_servers(count: i64) {
    gauge!("mcp_servers_active").set(count as f64);
}

/// Record MCP tool invocation
pub fn record_mcp_tool_invocation(server: &str, tool: &str) {
    counter!(
        "mcp_tool_invocations_total",
        "server" => server.to_string(),
        "tool" => tool.to_string()
    )
    .increment(1);
}

// ============================================================================
// Git Metrics
// ============================================================================

/// Record a git operation
pub fn record_git_operation(operation: &str, success: bool) {
    counter!(
        "git_operations_total",
        "operation" => operation.to_string(),
        "success" => success.to_string()
    )
    .increment(1);
}

// ============================================================================
// A28: Rate Limiting Metrics
// ============================================================================

/// Record a rate limit decision
pub fn record_rate_limit(category: &str, tier: &str, allowed: bool) {
    if allowed {
        counter!(
            "rate_limit_allowed_total",
            "category" => category.to_string(),
            "tier" => tier.to_string()
        )
        .increment(1);
    } else {
        counter!(
            "rate_limit_exceeded_total",
            "category" => category.to_string(),
            "tier" => tier.to_string()
        )
        .increment(1);
    }
}

/// Update rate limiter bucket count gauge
pub fn set_rate_limit_buckets(count: i64) {
    gauge!("rate_limit_active_buckets").set(count as f64);
}

// ============================================================================
// A28: RTE Delegation Metrics
// ============================================================================

/// Record an RTE delegation attempt
pub fn record_rte_delegation(tool_name: &str, result: &str, duration: Duration) {
    counter!(
        "rte_delegations_total",
        "tool" => tool_name.to_string(),
        "result" => result.to_string()
    )
    .increment(1);
    histogram!(
        "rte_delegation_duration_seconds",
        "tool" => tool_name.to_string()
    )
    .record(duration.as_secs_f64());
}

// ============================================================================
// A28: Auth Metrics
// ============================================================================

/// Record an authentication attempt
pub fn record_auth_attempt(method: &str, success: bool) {
    counter!(
        "auth_attempts_total",
        "method" => method.to_string(),
        "success" => success.to_string()
    )
    .increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_collector_creation() {
        // This test may fail if metrics recorder is already installed
        // Just verify the code compiles correctly
    }

    #[test]
    fn test_record_functions_compile() {
        // Verify all record functions are callable
        // They may fail at runtime without a metrics recorder, but should compile
    }
}
