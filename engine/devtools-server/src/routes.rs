//! Route registration for devtools-server.

use axum::middleware;
use axum::routing::{delete, get, post, put};
use axum::Router;
use canal_identity::IdentityService;
use std::sync::Arc;

use crate::auth;
use crate::handlers;
use crate::state::AppState;

/// Build the devtools server router.
///
/// Health endpoint is public. All other endpoints require Bearer auth.
pub fn build_router(state: Arc<AppState>, identity_service: Arc<IdentityService>) -> Router {
    // Public routes
    let public = Router::new().route("/v1/health", get(handlers::health));

    // Protected routes
    let protected = Router::new()
        // Ingest
        .route("/v1/traces", post(handlers::ingest::create_trace))
        .route("/v1/observations", post(handlers::ingest::create_observation))
        .route("/v1/ingest", post(handlers::ingest::batch_ingest))
        // Query — Traces
        .route("/v1/traces", get(handlers::traces::list_traces))
        .route("/v1/traces/{id}", get(handlers::traces::get_trace))
        .route("/v1/traces/{id}/export", get(handlers::traces::export_trace))
        // Query — Sessions
        .route("/v1/sessions", get(handlers::sessions::list_sessions))
        .route(
            "/v1/sessions/{id}/traces",
            get(handlers::sessions::get_session_traces),
        )
        // Metrics
        .route(
            "/v1/metrics/summary",
            get(handlers::metrics::get_metrics_summary),
        )
        // Infrastructure metrics
        .route(
            "/v1/metrics/query",
            get(handlers::infrastructure::query_prometheus),
        )
        .route(
            "/v1/metrics/containers",
            get(handlers::infrastructure::list_containers),
        )
        .route(
            "/v1/metrics/health",
            get(handlers::infrastructure::health_check),
        )
        .route(
            "/v1/metrics/targets",
            get(handlers::infrastructure::scrape_targets),
        )
        .route(
            "/v1/metrics/storage",
            get(handlers::infrastructure::storage_summary),
        )
        // Database observability (DT6)
        .route("/v1/database/stats", get(handlers::database::db_stats))
        .route("/v1/database/slow-queries", get(handlers::database::slow_queries))
        .route("/v1/database/connections", get(handlers::database::connections))
        .route("/v1/database/tables", get(handlers::database::tables))
        .route("/v1/database/indexes", get(handlers::database::indexes))
        .route("/v1/database/locks", get(handlers::database::locks))
        .route("/v1/database/replication", get(handlers::database::replication))
        .route("/v1/database/health", get(handlers::database::health))
        // Logs (Loki proxy)
        .route("/v1/logs/query", post(handlers::logs::query_logs))
        .route("/v1/logs/stream", get(handlers::logs::stream_logs))
        .route("/v1/logs/labels", get(handlers::logs::list_labels))
        .route(
            "/v1/logs/label/{name}/values",
            get(handlers::logs::label_values),
        )
        // Alerts — Rule Management (6)
        .route("/v1/alerts/rules", get(handlers::alerts::list_rules))
        .route("/v1/alerts/rules", post(handlers::alerts::create_rule))
        .route("/v1/alerts/rules/{id}", get(handlers::alerts::get_rule))
        .route("/v1/alerts/rules/{id}", put(handlers::alerts::update_rule))
        .route(
            "/v1/alerts/rules/{id}",
            delete(handlers::alerts::delete_rule),
        )
        .route(
            "/v1/alerts/test/{rule_id}",
            post(handlers::alerts::test_fire),
        )
        // Alerts — Active Alerts (2)
        .route("/v1/alerts/active", get(handlers::alerts::list_active))
        .route(
            "/v1/alerts/{id}/acknowledge",
            post(handlers::alerts::acknowledge_alert),
        )
        // Alerts — History (1)
        .route("/v1/alerts/history", get(handlers::alerts::list_history))
        // Alerts — Notification Channels (4)
        .route("/v1/alerts/channels", get(handlers::alerts::list_channels))
        .route(
            "/v1/alerts/channels",
            post(handlers::alerts::create_channel),
        )
        .route(
            "/v1/alerts/channels/{id}",
            put(handlers::alerts::update_channel),
        )
        .route(
            "/v1/alerts/channels/{id}",
            delete(handlers::alerts::delete_channel),
        )
        // Alerts — SSE Stream (1)
        .route("/v1/alerts/stream", get(handlers::alerts::alert_stream))
        // Alerts — Summary (1)
        .route("/v1/alerts/summary", get(handlers::alerts::alert_summary))
        // SSE
        .route("/v1/sse/traces/{id}", get(handlers::sse::sse_trace))
        .route("/v1/sse/global", get(handlers::sse::sse_global))
        // Projects
        .route("/v1/projects", post(handlers::projects::create_project))
        .route("/v1/projects", get(handlers::projects::list_projects))
        .route("/v1/projects/{id}", get(handlers::projects::get_project))
        .route(
            "/v1/projects/{id}",
            delete(handlers::projects::delete_project),
        )
        .route_layer(middleware::from_fn_with_state(
            identity_service,
            auth::require_auth,
        ));

    public.merge(protected).with_state(state)
}
