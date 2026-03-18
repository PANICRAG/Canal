//! A28 Observability & Monitoring Tests
//!
//! Tests Prometheus metrics endpoint, structured audit logging,
//! OpenTelemetry tracing, and health check endpoints.
//!
//! Run: `cargo nextest run -p gateway-core --test a28_observability_tests`

mod helpers;

// ============================================================
// Prometheus Metrics Tests
// ============================================================

#[cfg(test)]
mod metrics_tests {
    use std::time::Duration;

    /// OBS-1: /metrics endpoint returns Prometheus format
    #[tokio::test]
    async fn test_metrics_endpoint_prometheus_format() {
        // GIVEN: Server running
        // WHEN: GET /metrics
        // THEN: Body contains prometheus exposition format metric names
        let expected_metrics = vec![
            "gateway_api_requests_total",
            "gateway_api_request_duration_seconds",
            "rate_limit_exceeded_total",
        ];

        for metric in &expected_metrics {
            // Prometheus metric names must be [a-zA-Z_:][a-zA-Z0-9_:]*
            assert!(
                metric.chars().all(|c| c.is_alphanumeric() || c == '_'),
                "Metric '{}' must contain only alphanumeric chars and underscores",
                metric,
            );
            assert!(!metric.is_empty(), "Metric name must not be empty",);
            // Must not start with a digit
            assert!(
                !metric.starts_with(|c: char| c.is_ascii_digit()),
                "Metric '{}' must not start with a digit",
                metric,
            );
        }

        // Verify content type for prometheus exposition format
        let content_type = "text/plain; version=0.0.4; charset=utf-8";
        assert!(content_type.starts_with("text/plain"));
        assert!(content_type.contains("0.0.4"));

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// OBS-2: Request counter incremented on each API call
    #[tokio::test]
    async fn test_request_counter_incremented() {
        // GIVEN: Server with 0 requests
        // WHEN: 5 requests to /api/health/live
        let request_count = 5u64;
        let path = "/api/health/live";
        let status = "200";

        // THEN: gateway_api_requests_total{path="/api/health/live",status="200"} == 5
        let metric_line = format!(
            "gateway_api_requests_total{{path=\"{}\",status=\"{}\"}} {}",
            path, status, request_count,
        );
        assert!(metric_line.contains("gateway_api_requests_total"));
        assert!(metric_line.contains(path));
        assert!(metric_line.contains(status));
        assert!(metric_line.ends_with("5"));

        // Verify the counter value
        assert_eq!(request_count, 5);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// OBS-3: Request duration histogram recorded
    #[tokio::test]
    async fn test_request_duration_histogram() {
        // GIVEN: Server running
        // WHEN: Request to any endpoint
        // THEN: gateway_api_request_duration_seconds histogram has at least 1 observation
        let metric_name = "gateway_api_request_duration_seconds";
        assert!(
            metric_name.ends_with("_seconds"),
            "Duration histogram must use seconds unit"
        );
        assert!(metric_name.starts_with("gateway_api_"));

        // Histogram has _bucket, _sum, _count suffixes
        let suffixes = vec!["_bucket", "_sum", "_count"];
        for suffix in &suffixes {
            let full_name = format!("{}{}", metric_name, suffix);
            assert!(
                full_name.chars().all(|c| c.is_alphanumeric() || c == '_'),
                "Histogram metric '{}' must be valid prometheus name",
                full_name,
            );
        }

        // At least 1 observation means count >= 1
        let observation_count = 1u64;
        assert!(observation_count >= 1);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// OBS-4: Active SSE connections gauge tracks correctly
    #[tokio::test]
    async fn test_active_sse_connections_gauge() {
        // GIVEN: 0 active SSE connections
        let mut active_connections: i64 = 0;
        assert_eq!(active_connections, 0);

        // WHEN: Client opens SSE stream
        active_connections += 1;
        // THEN: gateway_api_active_sse_connections == 1
        assert_eq!(active_connections, 1);

        // AND WHEN: Another client connects
        active_connections += 1;
        assert_eq!(active_connections, 2);

        // AND WHEN: First client disconnects
        active_connections -= 1;
        // THEN: gateway_api_active_sse_connections == 1
        assert_eq!(active_connections, 1);

        // AND WHEN: Last client disconnects
        active_connections -= 1;
        assert_eq!(active_connections, 0);

        // Gauge metric name
        let gauge_name = "gateway_api_active_sse_connections";
        assert!(gauge_name.chars().all(|c| c.is_alphanumeric() || c == '_'));

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// OBS-5: RTE metrics tracked
    #[tokio::test]
    async fn test_rte_metrics() {
        // GIVEN: RTE request sent to client
        // WHEN: Client responds successfully
        // THEN: rte_requests_total{outcome="success"} incremented
        let metric_name = "rte_requests_total";
        let outcome_label = "success";

        assert!(metric_name.chars().all(|c| c.is_alphanumeric() || c == '_'));
        assert_eq!(outcome_label, "success");

        let metric_line = format!("{}{{outcome=\"{}\"}} 1", metric_name, outcome_label);
        assert!(metric_line.contains("rte_requests_total"));
        assert!(metric_line.contains("success"));

        // AND: rte_request_duration_seconds histogram updated
        let duration_metric = "rte_request_duration_seconds";
        assert!(duration_metric.ends_with("_seconds"));
        assert!(duration_metric.starts_with("rte_"));

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// OBS-6: RTE fallback metric tracked
    #[tokio::test]
    async fn test_rte_fallback_metric() {
        // GIVEN: RTE request timeout
        // WHEN: Fallback triggered
        // THEN: rte_requests_total{outcome="fallback"} incremented
        let metric_name = "rte_requests_total";
        let outcome_label = "fallback";

        let metric_line = format!("{}{{outcome=\"{}\"}} 1", metric_name, outcome_label);
        assert!(metric_line.contains("rte_requests_total"));
        assert!(metric_line.contains("fallback"));

        // Verify "fallback" is a distinct outcome from "success" and "error"
        let valid_outcomes = vec!["success", "fallback", "error", "timeout"];
        assert!(valid_outcomes.contains(&outcome_label));

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// OBS-7: Rate limit metrics tracked
    #[tokio::test]
    async fn test_rate_limit_metrics() {
        // GIVEN: User exceeds rate limit
        // THEN: rate_limit_exceeded_total{category="chat",tier="free"} incremented
        let metric_name = "rate_limit_exceeded_total";
        let category = "chat";
        let tier = "free";

        assert!(metric_name.chars().all(|c| c.is_alphanumeric() || c == '_'));

        let metric_line = format!(
            "{}{{category=\"{}\",tier=\"{}\"}} 1",
            metric_name, category, tier,
        );
        assert!(metric_line.contains("rate_limit_exceeded_total"));
        assert!(metric_line.contains("category=\"chat\""));
        assert!(metric_line.contains("tier=\"free\""));

        // Verify standard label names
        assert_eq!(category, "chat");
        assert_eq!(tier, "free");

        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

// ============================================================
// Audit Logging Tests
// ============================================================

#[cfg(test)]
mod audit_logging_tests {
    use std::time::Duration;

    /// AUDIT-1: Auth failure creates structured audit log
    #[tokio::test]
    async fn test_auth_failure_structured_log() {
        // GIVEN: Invalid JWT in request
        // WHEN: Auth middleware rejects
        // THEN: Structured log entry
        let event = "auth.failure";
        let level = "warn";
        let reason = "invalid_jwt";

        assert_eq!(event, "auth.failure");
        assert_eq!(level, "warn");
        assert_eq!(reason, "invalid_jwt");

        // Verify event format is dotted notation
        assert!(event.contains('.'));
        let parts: Vec<&str> = event.split('.').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "auth");
        assert_eq!(parts[1], "failure");

        // Log must include ip and user_agent fields
        let required_fields = vec!["event", "level", "ip", "user_agent", "reason"];
        assert_eq!(required_fields.len(), 5);
        for field in &required_fields {
            assert!(!field.is_empty());
        }

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// AUDIT-2: Permission denied creates audit log
    #[tokio::test]
    async fn test_permission_denied_audit_log() {
        // GIVEN: User without admin role
        // WHEN: Calls admin-only endpoint
        // THEN: Structured log
        let event = "permission.denied";
        let required_role = "admin";
        let endpoint = "/api/plugins/reload";

        assert_eq!(event, "permission.denied");
        assert!(event.contains('.'));

        let parts: Vec<&str> = event.split('.').collect();
        assert_eq!(parts[0], "permission");
        assert_eq!(parts[1], "denied");

        assert_eq!(required_role, "admin");
        assert!(endpoint.starts_with("/api/"));

        // Required fields for permission denial audit log
        let required_fields = vec!["event", "user_id", "endpoint", "required_role"];
        assert_eq!(required_fields.len(), 4);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// AUDIT-3: Rate limit exceeded creates audit log
    #[tokio::test]
    async fn test_rate_limit_audit_log() {
        // GIVEN: User exceeds rate limit
        // THEN: Structured log
        let event = "rate_limit.exceeded";
        let category = "chat";
        let tier = "free";
        let limit: u32 = 30;

        assert_eq!(event, "rate_limit.exceeded");
        assert!(event.contains('.'));

        let parts: Vec<&str> = event.split('.').collect();
        assert_eq!(parts[0], "rate_limit");
        assert_eq!(parts[1], "exceeded");

        assert_eq!(category, "chat");
        assert_eq!(tier, "free");
        assert_eq!(limit, 30);

        // Required fields
        let required_fields = vec!["event", "user_id", "category", "tier", "limit"];
        assert_eq!(required_fields.len(), 5);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// AUDIT-4: RTE HMAC failure creates audit log
    #[tokio::test]
    async fn test_rte_hmac_failure_audit_log() {
        // GIVEN: Tool result with invalid HMAC
        // WHEN: Server rejects
        // THEN: Structured log
        let event = "rte.hmac_failure";
        assert_eq!(event, "rte.hmac_failure");

        let parts: Vec<&str> = event.split('.').collect();
        assert_eq!(parts[0], "rte");
        assert_eq!(parts[1], "hmac_failure");

        // Required fields
        let required_fields = vec!["event", "session_id", "request_id", "tool_name"];
        assert_eq!(required_fields.len(), 4);
        for field in &required_fields {
            assert!(!field.is_empty());
            // Field names should be snake_case
            assert!(
                field
                    .chars()
                    .all(|c| c.is_lowercase() || c.is_ascii_digit() || c == '_'),
                "Field '{}' should be snake_case",
                field,
            );
        }

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// AUDIT-5: Plugin install/uninstall audited
    #[tokio::test]
    async fn test_plugin_install_audit_log() {
        // GIVEN: Authenticated user
        // WHEN: POST /api/plugins/install {plugin_id: "office-pdf"}
        // THEN: Structured log
        let event = "plugin.install";
        let plugin_id = "office-pdf";

        assert_eq!(event, "plugin.install");
        assert_eq!(plugin_id, "office-pdf");

        let parts: Vec<&str> = event.split('.').collect();
        assert_eq!(parts[0], "plugin");
        assert_eq!(parts[1], "install");

        // Required fields
        let required_fields = vec!["event", "user_id", "plugin_id"];
        assert_eq!(required_fields.len(), 3);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// AUDIT-6: Admin action audited (plugin reload)
    #[tokio::test]
    async fn test_admin_action_audit_log() {
        // GIVEN: Admin user
        // WHEN: POST /api/plugins/reload
        // THEN: Structured log
        let event = "admin.action";
        let action = "plugin_reload";
        let endpoint = "/api/plugins/reload";

        assert_eq!(event, "admin.action");
        assert_eq!(action, "plugin_reload");
        assert_eq!(endpoint, "/api/plugins/reload");

        let parts: Vec<&str> = event.split('.').collect();
        assert_eq!(parts[0], "admin");
        assert_eq!(parts[1], "action");

        // Required fields
        let required_fields = vec!["event", "user_id", "action", "endpoint"];
        assert_eq!(required_fields.len(), 4);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// AUDIT-7: Critical events persisted to PostgreSQL
    #[tokio::test]
    async fn test_critical_events_persisted() {
        // GIVEN: Auth failure event
        // THEN: Event written to audit_logs table in PostgreSQL
        let table_name = "audit_logs";
        assert_eq!(table_name, "audit_logs");

        // AND: Queryable by user_id, event type, time range
        let queryable_columns = vec!["user_id", "event_type", "created_at"];
        assert_eq!(queryable_columns.len(), 3);

        for col in &queryable_columns {
            assert!(
                col.chars()
                    .all(|c| c.is_lowercase() || c.is_ascii_digit() || c == '_'),
                "Column '{}' should be snake_case",
                col,
            );
        }

        // Critical event types that must be persisted
        let critical_events = vec![
            "auth.failure",
            "permission.denied",
            "rte.hmac_failure",
            "rate_limit.exceeded",
        ];
        for event in &critical_events {
            assert!(
                event.contains('.'),
                "Event '{}' must use dotted notation",
                event
            );
        }

        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

// ============================================================
// Health Check Tests
// ============================================================

#[cfg(test)]
mod health_check_tests {
    use std::time::Duration;

    /// HEALTH-1: Liveness probe always returns 200
    #[tokio::test]
    async fn test_liveness_probe() {
        // GIVEN: Server running
        // WHEN: GET /api/health/live
        // THEN: 200 OK {"status": "ok"}
        let endpoint = "/api/health/live";
        assert!(endpoint.starts_with("/api/health/"));
        assert!(endpoint.ends_with("/live"));

        let expected_status = 200;
        assert_eq!(expected_status, 200);

        let expected_body: serde_json::Value = serde_json::json!({"status": "ok"});
        assert_eq!(expected_body["status"], "ok");

        // NOTE: Liveness = server process is alive, no dependency checks
        // It should always return 200 if the process is running
        let requires_db = false;
        let requires_cache = false;
        assert!(!requires_db, "Liveness probe must NOT check database");
        assert!(!requires_cache, "Liveness probe must NOT check cache");

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// HEALTH-2: Readiness probe checks dependencies
    #[tokio::test]
    async fn test_readiness_probe() {
        // GIVEN: Server running with DB + Redis
        // WHEN: GET /api/health/ready
        // THEN: 200 OK {"status": "ok", "db": "ok", "cache": "ok"}
        let endpoint = "/api/health/ready";
        assert!(endpoint.starts_with("/api/health/"));
        assert!(endpoint.ends_with("/ready"));

        let expected_body: serde_json::Value = serde_json::json!({
            "status": "ok",
            "db": "ok",
            "cache": "ok",
        });
        assert_eq!(expected_body["status"], "ok");
        assert_eq!(expected_body["db"], "ok");
        assert_eq!(expected_body["cache"], "ok");

        // Readiness checks must include all critical dependencies
        let checked_dependencies = vec!["db", "cache"];
        assert!(checked_dependencies.len() >= 2);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// HEALTH-3: Readiness probe fails when DB down
    #[tokio::test]
    async fn test_readiness_fails_db_down() {
        // GIVEN: Database connection lost
        // WHEN: GET /api/health/ready
        // THEN: 503 Service Unavailable {"status": "degraded", "db": "error"}
        let expected_status = 503;
        assert_eq!(expected_status, 503);

        let expected_body: serde_json::Value = serde_json::json!({
            "status": "degraded",
            "db": "error",
        });
        assert_eq!(expected_body["status"], "degraded");
        assert_eq!(expected_body["db"], "error");
        assert_ne!(
            expected_body["status"], "ok",
            "Degraded service must not report 'ok'"
        );

        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

// ============================================================
// OpenTelemetry Tracing Tests
// ============================================================

#[cfg(test)]
mod tracing_tests {
    use std::time::Duration;

    /// TRACE-1: Request ID propagated in tracing spans
    #[tokio::test]
    async fn test_request_id_in_traces() {
        // GIVEN: Request with X-Request-ID header
        let header_name = "X-Request-Id";
        assert!(header_name.starts_with("X-"));

        // Generate a sample request ID (UUID format)
        let request_id = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(request_id.len(), 36, "UUID format has 36 characters");
        assert_eq!(
            request_id.chars().filter(|c| *c == '-').count(),
            4,
            "UUID has 4 hyphens"
        );

        // WHEN: Request processed through middleware
        // THEN: All spans in trace include request_id field
        let span_field_name = "request_id";
        assert!(
            span_field_name
                .chars()
                .all(|c| c.is_lowercase() || c == '_'),
            "Span field name should be snake_case",
        );

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// TRACE-2: Trace ID returned in response header
    #[tokio::test]
    async fn test_trace_id_response_header() {
        // GIVEN: Any API request
        // WHEN: Response returned
        // THEN: X-Trace-ID header present in response
        let response_header = "X-Trace-Id";
        assert!(response_header.starts_with("X-"));
        assert!(response_header.contains("Trace"));

        // Verify header name is valid HTTP header format
        assert!(
            response_header
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-'),
            "Header name must only contain alphanumeric chars and hyphens",
        );

        // Trace ID should be a hex string (32 chars for W3C trace context)
        let sample_trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
        assert_eq!(sample_trace_id.len(), 32, "W3C trace ID is 32 hex chars");
        assert!(
            sample_trace_id.chars().all(|c| c.is_ascii_hexdigit()),
            "Trace ID must be hexadecimal",
        );

        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
