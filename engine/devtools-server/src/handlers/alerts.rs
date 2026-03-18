//! DT8: Dual-Layer Unified Alert Engine handlers.
//!
//! Provides in-memory alert rule management, active alert tracking,
//! alert history, notification channels, SSE streaming, and summary endpoints.
//! Layer 1 (Platform-Ops) rules are loaded from Prometheus YAML as read-only defaults.
//! Layer 2 (Per-App User) rules are API-managed.

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::error::{ApiError, ApiErrorDetail};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Scope of an alert rule — platform-wide or per-application.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "app_id")]
pub enum AlertScope {
    Platform,
    App(String),
}

/// Alert severity level.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    Critical,
    Warning,
}

/// Query type for evaluating alert conditions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum AlertQuery {
    #[serde(rename = "prom")]
    Prom { promql: String },
    #[serde(rename = "log")]
    Log { logql: String },
    #[serde(rename = "custom")]
    Custom { check_type: String },
}

/// An alert rule definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub scope: AlertScope,
    pub severity: AlertSeverity,
    pub query: AlertQuery,
    /// How long the condition must be true before firing (seconds).
    pub for_duration_secs: u64,
    pub enabled: bool,
    /// Channel IDs to notify when this rule fires.
    pub channels: Vec<String>,
}

/// A currently active (firing) alert.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveAlert {
    pub id: String,
    pub rule_id: String,
    pub rule_name: String,
    pub scope: AlertScope,
    pub severity: AlertSeverity,
    pub message: String,
    /// When the alert fired (RFC 3339).
    pub fired_at: String,
    pub acknowledged: bool,
}

/// An entry in the alert history log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertHistoryEntry {
    pub rule_id: String,
    pub rule_name: String,
    pub scope: AlertScope,
    pub severity: AlertSeverity,
    /// When the alert fired (RFC 3339).
    pub fired_at: String,
    /// When the alert was resolved, if resolved.
    pub resolved_at: Option<String>,
    /// Duration in seconds between fired and resolved.
    pub duration_secs: Option<u64>,
}

/// A notification channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannel {
    pub id: String,
    /// Channel type: "webhook", "in_app", "sse".
    pub channel_type: String,
    /// Channel-specific configuration (e.g. `{ "url": "...", "hmac_secret": "..." }`).
    pub config: serde_json::Value,
    pub enabled: bool,
}

/// In-memory state for the alert engine.
#[derive(Debug)]
pub struct AlertState {
    pub rules: Vec<AlertRule>,
    pub active_alerts: Vec<ActiveAlert>,
    pub history: Vec<AlertHistoryEntry>,
    pub channels: Vec<NotificationChannel>,
    /// Broadcast sender for SSE alert events.
    pub sse_tx: broadcast::Sender<SseAlertEvent>,
}

/// An SSE alert event pushed to connected clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseAlertEvent {
    pub event_type: String, // "alert_firing", "alert_resolved", "alert_acknowledged", "test_fire"
    pub alert: ActiveAlert,
}

impl AlertState {
    /// Create a new `AlertState` with the 14 default rules (6 platform + 8 per-app).
    pub fn with_defaults() -> Self {
        let (sse_tx, _) = broadcast::channel(256);
        AlertState {
            rules: default_rules(),
            active_alerts: Vec::new(),
            history: Vec::new(),
            channels: vec![
                NotificationChannel {
                    id: "default-sse".into(),
                    channel_type: "sse".into(),
                    config: serde_json::json!({}),
                    enabled: true,
                },
                NotificationChannel {
                    id: "default-in-app".into(),
                    channel_type: "in_app".into(),
                    config: serde_json::json!({}),
                    enabled: true,
                },
            ],
            sse_tx,
        }
    }
}

// ---------------------------------------------------------------------------
// Default rules (14 total)
// ---------------------------------------------------------------------------

fn default_rules() -> Vec<AlertRule> {
    vec![
        // Layer 1 — Platform-Ops (6 rules)
        AlertRule {
            id: "platform-disk-critical".into(),
            name: "HostDiskCritical".into(),
            scope: AlertScope::Platform,
            severity: AlertSeverity::Critical,
            query: AlertQuery::Prom {
                promql: r#"node_filesystem_avail_bytes{mountpoint="/"} / node_filesystem_size_bytes < 0.15"#.into(),
            },
            for_duration_secs: 600,
            enabled: true,
            channels: vec!["default-sse".into()],
        },
        AlertRule {
            id: "platform-target-down".into(),
            name: "PrometheusTargetDown".into(),
            scope: AlertScope::Platform,
            severity: AlertSeverity::Critical,
            query: AlertQuery::Prom {
                promql: "up == 0".into(),
            },
            for_duration_secs: 300,
            enabled: true,
            channels: vec!["default-sse".into()],
        },
        AlertRule {
            id: "platform-memory-pressure".into(),
            name: "HighMemoryPressure".into(),
            scope: AlertScope::Platform,
            severity: AlertSeverity::Warning,
            query: AlertQuery::Prom {
                promql: "(1 - node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes) > 0.9".into(),
            },
            for_duration_secs: 600,
            enabled: true,
            channels: vec!["default-sse".into()],
        },
        AlertRule {
            id: "platform-replication-lag".into(),
            name: "DatabaseReplicationLag".into(),
            scope: AlertScope::Platform,
            severity: AlertSeverity::Critical,
            query: AlertQuery::Prom {
                promql: "pg_replication_lag > 5".into(),
            },
            for_duration_secs: 300,
            enabled: true,
            channels: vec!["default-sse".into()],
        },
        AlertRule {
            id: "platform-service-down".into(),
            name: "ServiceProcessDown".into(),
            scope: AlertScope::Platform,
            severity: AlertSeverity::Critical,
            query: AlertQuery::Prom {
                promql: r#"up{job=~"weir|canal|river"} == 0"#.into(),
            },
            for_duration_secs: 120,
            enabled: true,
            channels: vec!["default-sse".into()],
        },
        AlertRule {
            id: "platform-cert-expiring".into(),
            name: "CertificateExpiringSoon".into(),
            scope: AlertScope::Platform,
            severity: AlertSeverity::Warning,
            query: AlertQuery::Prom {
                promql: "(probe_ssl_earliest_cert_expiry - time()) / 86400 < 14".into(),
            },
            for_duration_secs: 3600,
            enabled: true,
            channels: vec!["default-sse".into()],
        },
        // Layer 2 — Per-App User (8 rules, template scope)
        AlertRule {
            id: "app-high-cpu".into(),
            name: "AppHighCPU".into(),
            scope: AlertScope::App("$APP_ID".into()),
            severity: AlertSeverity::Warning,
            query: AlertQuery::Prom {
                promql: r#"rate(container_cpu_usage_seconds_total{app_id="$APP_ID"}[5m]) * 100 > 80"#.into(),
            },
            for_duration_secs: 300,
            enabled: true,
            channels: vec!["default-sse".into(), "default-in-app".into()],
        },
        AlertRule {
            id: "app-high-memory".into(),
            name: "AppHighMemory".into(),
            scope: AlertScope::App("$APP_ID".into()),
            severity: AlertSeverity::Warning,
            query: AlertQuery::Prom {
                promql: r#"container_memory_usage_bytes{app_id="$APP_ID"} / container_spec_memory_limit_bytes > 0.85"#.into(),
            },
            for_duration_secs: 300,
            enabled: true,
            channels: vec!["default-sse".into(), "default-in-app".into()],
        },
        AlertRule {
            id: "app-container-down".into(),
            name: "AppContainerDown".into(),
            scope: AlertScope::App("$APP_ID".into()),
            severity: AlertSeverity::Critical,
            query: AlertQuery::Custom {
                check_type: "container_exited".into(),
            },
            for_duration_secs: 0,
            enabled: true,
            channels: vec!["default-sse".into(), "default-in-app".into()],
        },
        AlertRule {
            id: "app-high-error-rate".into(),
            name: "AppHighErrorRate".into(),
            scope: AlertScope::App("$APP_ID".into()),
            severity: AlertSeverity::Warning,
            query: AlertQuery::Prom {
                promql: r#"rate(http_requests_total{app_id="$APP_ID",code=~"5.."}[5m]) / rate(http_requests_total{app_id="$APP_ID"}[5m]) > 0.05"#.into(),
            },
            for_duration_secs: 300,
            enabled: true,
            channels: vec!["default-sse".into(), "default-in-app".into()],
        },
        AlertRule {
            id: "app-high-latency".into(),
            name: "AppHighLatency".into(),
            scope: AlertScope::App("$APP_ID".into()),
            severity: AlertSeverity::Warning,
            query: AlertQuery::Prom {
                promql: r#"histogram_quantile(0.95, rate(http_request_duration_seconds_bucket{app_id="$APP_ID"}[5m])) > 3"#.into(),
            },
            for_duration_secs: 300,
            enabled: true,
            channels: vec!["default-sse".into(), "default-in-app".into()],
        },
        AlertRule {
            id: "app-function-error-rate".into(),
            name: "FunctionHighErrorRate".into(),
            scope: AlertScope::App("$APP_ID".into()),
            severity: AlertSeverity::Warning,
            query: AlertQuery::Prom {
                promql: r#"rate(function_invocations_total{app_id="$APP_ID",status="error"}[5m]) / rate(function_invocations_total{app_id="$APP_ID"}[5m]) > 0.1"#.into(),
            },
            for_duration_secs: 300,
            enabled: true,
            channels: vec!["default-sse".into(), "default-in-app".into()],
        },
        AlertRule {
            id: "app-cron-failures".into(),
            name: "CronConsecutiveFailures".into(),
            scope: AlertScope::App("$APP_ID".into()),
            severity: AlertSeverity::Warning,
            query: AlertQuery::Custom {
                check_type: "cron_consecutive_failures_3".into(),
            },
            for_duration_secs: 0,
            enabled: true,
            channels: vec!["default-sse".into(), "default-in-app".into()],
        },
        AlertRule {
            id: "app-storage-quota".into(),
            name: "StorageQuotaWarning".into(),
            scope: AlertScope::App("$APP_ID".into()),
            severity: AlertSeverity::Warning,
            query: AlertQuery::Custom {
                check_type: "storage_usage_80_percent".into(),
            },
            for_duration_secs: 300,
            enabled: true,
            channels: vec!["default-sse".into(), "default-in-app".into()],
        },
    ]
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

fn not_found(msg: impl Into<String>) -> ApiError {
    ApiError {
        error: ApiErrorDetail {
            code: "not_found".into(),
            message: msg.into(),
        },
    }
}

fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError {
        error: ApiErrorDetail {
            code: "invalid_input".into(),
            message: msg.into(),
        },
    }
}

/// Parse scope query parameter: "platform" or "app:{app_id}".
fn parse_scope_filter(scope: &str) -> Option<AlertScope> {
    if scope == "platform" {
        Some(AlertScope::Platform)
    } else {
        scope
            .strip_prefix("app:")
            .map(|app_id| AlertScope::App(app_id.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ScopeFilter {
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryFilter {
    pub start: Option<String>,
    pub end: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateRuleRequest {
    pub name: String,
    pub scope: AlertScope,
    pub severity: AlertSeverity,
    pub query: AlertQuery,
    pub for_duration_secs: Option<u64>,
    pub enabled: Option<bool>,
    pub channels: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRuleRequest {
    pub name: Option<String>,
    pub severity: Option<AlertSeverity>,
    pub query: Option<AlertQuery>,
    pub for_duration_secs: Option<u64>,
    pub enabled: Option<bool>,
    pub channels: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct CreateChannelRequest {
    pub channel_type: String,
    pub config: serde_json::Value,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateChannelRequest {
    pub channel_type: Option<String>,
    pub config: Option<serde_json::Value>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct AlertSummary {
    pub total_rules: usize,
    pub active_alerts_count: usize,
    pub critical_count: usize,
    pub warning_count: usize,
}

// ---------------------------------------------------------------------------
// Handler: Rule Management (6 endpoints)
// ---------------------------------------------------------------------------

/// GET /v1/alerts/rules — list all rules, optionally filtered by scope.
pub async fn list_rules(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ScopeFilter>,
) -> Result<impl IntoResponse, ApiError> {
    let alert_state = state.alert_state.read().await;

    let rules: Vec<&AlertRule> = if let Some(ref scope_str) = params.scope {
        let scope = parse_scope_filter(scope_str)
            .ok_or_else(|| bad_request(format!("Invalid scope: {scope_str}")))?;
        alert_state
            .rules
            .iter()
            .filter(|r| r.scope == scope)
            .collect()
    } else {
        alert_state.rules.iter().collect()
    };

    Ok(Json(serde_json::json!({ "rules": rules })))
}

/// GET /v1/alerts/rules/{id} — get a single rule by ID.
pub async fn get_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let alert_state = state.alert_state.read().await;
    let rule = alert_state
        .rules
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| not_found(format!("Rule not found: {id}")))?;
    Ok(Json(serde_json::json!({ "rule": rule })))
}

/// POST /v1/alerts/rules — create a new alert rule (Layer 2 only).
pub async fn create_rule(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateRuleRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Layer 1 rules are YAML-defined; only per-app rules can be created via API
    if req.scope == AlertScope::Platform {
        return Err(bad_request(
            "Platform-scoped rules are managed via Prometheus YAML, not the API",
        ));
    }

    let id = format!("user-{}", uuid::Uuid::new_v4());
    let rule = AlertRule {
        id: id.clone(),
        name: req.name,
        scope: req.scope,
        severity: req.severity,
        query: req.query,
        for_duration_secs: req.for_duration_secs.unwrap_or(300),
        enabled: req.enabled.unwrap_or(true),
        channels: req.channels.unwrap_or_default(),
    };

    let mut alert_state = state.alert_state.write().await;
    alert_state.rules.push(rule.clone());
    info!(rule_id = %id, "Alert rule created");

    Ok(Json(serde_json::json!({ "rule": rule })))
}

/// PUT /v1/alerts/rules/{id} — update an existing rule.
pub async fn update_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateRuleRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let mut alert_state = state.alert_state.write().await;
    let rule = alert_state
        .rules
        .iter_mut()
        .find(|r| r.id == id)
        .ok_or_else(|| not_found(format!("Rule not found: {id}")))?;

    if let Some(name) = req.name {
        rule.name = name;
    }
    if let Some(severity) = req.severity {
        rule.severity = severity;
    }
    if let Some(query) = req.query {
        rule.query = query;
    }
    if let Some(dur) = req.for_duration_secs {
        rule.for_duration_secs = dur;
    }
    if let Some(enabled) = req.enabled {
        rule.enabled = enabled;
    }
    if let Some(channels) = req.channels {
        rule.channels = channels;
    }

    info!(rule_id = %id, "Alert rule updated");
    let rule = rule.clone();
    Ok(Json(serde_json::json!({ "rule": rule })))
}

/// DELETE /v1/alerts/rules/{id} — delete a rule.
pub async fn delete_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let mut alert_state = state.alert_state.write().await;
    let len_before = alert_state.rules.len();
    alert_state.rules.retain(|r| r.id != id);
    if alert_state.rules.len() == len_before {
        return Err(not_found(format!("Rule not found: {id}")));
    }
    // Also remove any active alerts for this rule
    alert_state.active_alerts.retain(|a| a.rule_id != id);
    info!(rule_id = %id, "Alert rule deleted");
    Ok(Json(serde_json::json!({ "deleted": true })))
}

/// POST /v1/alerts/test/{rule_id} — test-fire an alert for a rule.
pub async fn test_fire(
    State(state): State<Arc<AppState>>,
    Path(rule_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let mut alert_state = state.alert_state.write().await;
    let rule = alert_state
        .rules
        .iter()
        .find(|r| r.id == rule_id)
        .ok_or_else(|| not_found(format!("Rule not found: {rule_id}")))?
        .clone();

    let alert = ActiveAlert {
        id: format!("test-{}", uuid::Uuid::new_v4()),
        rule_id: rule.id.clone(),
        rule_name: rule.name.clone(),
        scope: rule.scope.clone(),
        severity: rule.severity.clone(),
        message: format!("[TEST] Alert rule '{}' test-fired", rule.name),
        fired_at: chrono::Utc::now().to_rfc3339(),
        acknowledged: false,
    };

    // Broadcast SSE event
    let event = SseAlertEvent {
        event_type: "test_fire".into(),
        alert: alert.clone(),
    };
    let _ = alert_state.sse_tx.send(event);

    alert_state.active_alerts.push(alert.clone());
    info!(rule_id = %rule_id, "Test alert fired");

    Ok(Json(serde_json::json!({ "alert": alert })))
}

// ---------------------------------------------------------------------------
// Handler: Active Alerts (2 endpoints)
// ---------------------------------------------------------------------------

/// GET /v1/alerts/active — list all active (firing) alerts, optionally filtered by scope.
pub async fn list_active(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ScopeFilter>,
) -> Result<impl IntoResponse, ApiError> {
    let alert_state = state.alert_state.read().await;

    let alerts: Vec<&ActiveAlert> = if let Some(ref scope_str) = params.scope {
        let scope = parse_scope_filter(scope_str)
            .ok_or_else(|| bad_request(format!("Invalid scope: {scope_str}")))?;
        alert_state
            .active_alerts
            .iter()
            .filter(|a| a.scope == scope)
            .collect()
    } else {
        alert_state.active_alerts.iter().collect()
    };

    Ok(Json(serde_json::json!({ "alerts": alerts })))
}

/// POST /v1/alerts/{id}/acknowledge — acknowledge an active alert.
pub async fn acknowledge_alert(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let mut alert_state = state.alert_state.write().await;
    let alert = alert_state
        .active_alerts
        .iter_mut()
        .find(|a| a.id == id)
        .ok_or_else(|| not_found(format!("Active alert not found: {id}")))?;

    alert.acknowledged = true;
    info!(alert_id = %id, "Alert acknowledged");

    let alert = alert.clone();

    // Broadcast SSE event
    let event = SseAlertEvent {
        event_type: "alert_acknowledged".into(),
        alert: alert.clone(),
    };
    let _ = alert_state.sse_tx.send(event);

    Ok(Json(serde_json::json!({ "alert": alert })))
}

// ---------------------------------------------------------------------------
// Handler: History (1 endpoint)
// ---------------------------------------------------------------------------

/// GET /v1/alerts/history — query alert history with optional time range and scope filters.
pub async fn list_history(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HistoryFilter>,
) -> Result<impl IntoResponse, ApiError> {
    let alert_state = state.alert_state.read().await;

    let mut entries: Vec<&AlertHistoryEntry> = alert_state.history.iter().collect();

    // Filter by scope
    if let Some(ref scope_str) = params.scope {
        if let Some(scope) = parse_scope_filter(scope_str) {
            entries.retain(|e| e.scope == scope);
        }
    }

    // Filter by time range
    if let Some(ref start) = params.start {
        entries.retain(|e| e.fired_at.as_str() >= start.as_str());
    }
    if let Some(ref end) = params.end {
        entries.retain(|e| e.fired_at.as_str() <= end.as_str());
    }

    Ok(Json(serde_json::json!({ "history": entries })))
}

// ---------------------------------------------------------------------------
// Handler: Notification Channels (4 endpoints)
// ---------------------------------------------------------------------------

/// GET /v1/alerts/channels — list all notification channels.
pub async fn list_channels(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    let alert_state = state.alert_state.read().await;
    Ok(Json(
        serde_json::json!({ "channels": alert_state.channels }),
    ))
}

/// POST /v1/alerts/channels — create a notification channel.
pub async fn create_channel(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateChannelRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let valid_types = ["webhook", "in_app", "sse"];
    if !valid_types.contains(&req.channel_type.as_str()) {
        return Err(bad_request(format!(
            "Invalid channel_type: {}. Must be one of: webhook, in_app, sse",
            req.channel_type
        )));
    }

    let id = format!("ch-{}", uuid::Uuid::new_v4());
    let channel = NotificationChannel {
        id: id.clone(),
        channel_type: req.channel_type,
        config: req.config,
        enabled: req.enabled.unwrap_or(true),
    };

    let mut alert_state = state.alert_state.write().await;
    alert_state.channels.push(channel.clone());
    info!(channel_id = %id, "Notification channel created");

    Ok(Json(serde_json::json!({ "channel": channel })))
}

/// PUT /v1/alerts/channels/{id} — update a notification channel.
pub async fn update_channel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateChannelRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let mut alert_state = state.alert_state.write().await;
    let channel = alert_state
        .channels
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| not_found(format!("Channel not found: {id}")))?;

    if let Some(channel_type) = req.channel_type {
        channel.channel_type = channel_type;
    }
    if let Some(config) = req.config {
        channel.config = config;
    }
    if let Some(enabled) = req.enabled {
        channel.enabled = enabled;
    }

    info!(channel_id = %id, "Notification channel updated");
    let channel = channel.clone();
    Ok(Json(serde_json::json!({ "channel": channel })))
}

/// DELETE /v1/alerts/channels/{id} — delete a notification channel.
pub async fn delete_channel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let mut alert_state = state.alert_state.write().await;
    let len_before = alert_state.channels.len();
    alert_state.channels.retain(|c| c.id != id);
    if alert_state.channels.len() == len_before {
        return Err(not_found(format!("Channel not found: {id}")));
    }
    info!(channel_id = %id, "Notification channel deleted");
    Ok(Json(serde_json::json!({ "deleted": true })))
}

// ---------------------------------------------------------------------------
// Handler: SSE Stream (1 endpoint)
// ---------------------------------------------------------------------------

/// GET /v1/alerts/stream — SSE stream of real-time alert events.
pub async fn alert_stream(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    let alert_state = state.alert_state.read().await;
    let mut rx = alert_state.sse_tx.subscribe();
    drop(alert_state);

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    yield Ok::<_, std::convert::Infallible>(
                        Event::default()
                            .event(&event.event_type)
                            .data(data)
                    );
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(missed = n, "SSE client lagged, skipped events");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Ok(Sse::new(Box::pin(stream)).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

// ---------------------------------------------------------------------------
// Handler: Summary (1 endpoint)
// ---------------------------------------------------------------------------

/// GET /v1/alerts/summary — alert system summary.
pub async fn alert_summary(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    let alert_state = state.alert_state.read().await;

    let critical_count = alert_state
        .active_alerts
        .iter()
        .filter(|a| a.severity == AlertSeverity::Critical && !a.acknowledged)
        .count();
    let warning_count = alert_state
        .active_alerts
        .iter()
        .filter(|a| a.severity == AlertSeverity::Warning && !a.acknowledged)
        .count();

    let summary = AlertSummary {
        total_rules: alert_state.rules.len(),
        active_alerts_count: alert_state.active_alerts.len(),
        critical_count,
        warning_count,
    };

    Ok(Json(serde_json::json!({ "summary": summary })))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_rules_count() {
        let rules = default_rules();
        assert_eq!(
            rules.len(),
            14,
            "Expected 14 default rules (6 platform + 8 per-app)"
        );
    }

    #[test]
    fn test_default_rules_platform_count() {
        let rules = default_rules();
        let platform_count = rules
            .iter()
            .filter(|r| r.scope == AlertScope::Platform)
            .count();
        assert_eq!(platform_count, 6, "Expected 6 platform-scoped rules");
    }

    #[test]
    fn test_default_rules_app_count() {
        let rules = default_rules();
        let app_count = rules
            .iter()
            .filter(|r| matches!(r.scope, AlertScope::App(_)))
            .count();
        assert_eq!(app_count, 8, "Expected 8 app-scoped rules");
    }

    #[test]
    fn test_default_rules_all_enabled() {
        let rules = default_rules();
        assert!(
            rules.iter().all(|r| r.enabled),
            "All default rules should be enabled"
        );
    }

    #[test]
    fn test_default_rules_unique_ids() {
        let rules = default_rules();
        let mut ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 14, "All rule IDs must be unique");
    }

    #[test]
    fn test_alert_state_with_defaults() {
        let state = AlertState::with_defaults();
        assert_eq!(state.rules.len(), 14);
        assert!(state.active_alerts.is_empty());
        assert!(state.history.is_empty());
        assert_eq!(state.channels.len(), 2); // default-sse + default-in-app
    }

    #[test]
    fn test_alert_state_default_channels() {
        let state = AlertState::with_defaults();
        let sse = state.channels.iter().find(|c| c.id == "default-sse");
        assert!(sse.is_some());
        assert_eq!(sse.unwrap().channel_type, "sse");

        let in_app = state.channels.iter().find(|c| c.id == "default-in-app");
        assert!(in_app.is_some());
        assert_eq!(in_app.unwrap().channel_type, "in_app");
    }

    #[test]
    fn test_parse_scope_filter_platform() {
        assert_eq!(parse_scope_filter("platform"), Some(AlertScope::Platform));
    }

    #[test]
    fn test_parse_scope_filter_app() {
        assert_eq!(
            parse_scope_filter("app:my-app-123"),
            Some(AlertScope::App("my-app-123".into()))
        );
    }

    #[test]
    fn test_parse_scope_filter_invalid() {
        assert_eq!(parse_scope_filter("invalid"), None);
        assert_eq!(parse_scope_filter(""), None);
    }

    #[test]
    fn test_alert_severity_serialization() {
        let critical = serde_json::to_string(&AlertSeverity::Critical).unwrap();
        assert_eq!(critical, r#""critical""#);
        let warning = serde_json::to_string(&AlertSeverity::Warning).unwrap();
        assert_eq!(warning, r#""warning""#);
    }

    #[test]
    fn test_alert_scope_serialization() {
        let platform = serde_json::to_string(&AlertScope::Platform).unwrap();
        assert!(platform.contains("Platform"));

        let app = serde_json::to_string(&AlertScope::App("my-app".into())).unwrap();
        assert!(app.contains("my-app"));
    }

    #[test]
    fn test_alert_query_prom_serialization() {
        let query = AlertQuery::Prom {
            promql: "up == 0".into(),
        };
        let json = serde_json::to_string(&query).unwrap();
        assert!(json.contains("prom"));
        assert!(json.contains("up == 0"));
    }

    #[test]
    fn test_alert_query_log_serialization() {
        let query = AlertQuery::Log {
            logql: r#"{job="docker"} |= "error""#.into(),
        };
        let json = serde_json::to_string(&query).unwrap();
        assert!(json.contains("log"));
    }

    #[test]
    fn test_alert_query_custom_serialization() {
        let query = AlertQuery::Custom {
            check_type: "container_exited".into(),
        };
        let json = serde_json::to_string(&query).unwrap();
        assert!(json.contains("custom"));
        assert!(json.contains("container_exited"));
    }

    #[test]
    fn test_active_alert_creation() {
        let alert = ActiveAlert {
            id: "test-1".into(),
            rule_id: "platform-disk-critical".into(),
            rule_name: "HostDiskCritical".into(),
            scope: AlertScope::Platform,
            severity: AlertSeverity::Critical,
            message: "Disk space low".into(),
            fired_at: "2026-03-07T00:00:00Z".into(),
            acknowledged: false,
        };
        assert!(!alert.acknowledged);
        assert_eq!(alert.severity, AlertSeverity::Critical);
    }

    #[test]
    fn test_alert_history_entry_serialization() {
        let entry = AlertHistoryEntry {
            rule_id: "platform-disk-critical".into(),
            rule_name: "HostDiskCritical".into(),
            scope: AlertScope::Platform,
            severity: AlertSeverity::Critical,
            fired_at: "2026-03-07T00:00:00Z".into(),
            resolved_at: Some("2026-03-07T01:00:00Z".into()),
            duration_secs: Some(3600),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("HostDiskCritical"));
        assert!(json.contains("3600"));
    }

    #[test]
    fn test_alert_summary_serialization() {
        let summary = AlertSummary {
            total_rules: 14,
            active_alerts_count: 3,
            critical_count: 1,
            warning_count: 2,
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("14"));
        assert!(json.contains("\"critical_count\":1"));
    }

    #[test]
    fn test_notification_channel_serialization() {
        let channel = NotificationChannel {
            id: "ch-1".into(),
            channel_type: "webhook".into(),
            config: serde_json::json!({
                "url": "https://hooks.slack.com/test",
                "hmac_secret": "secret123"
            }),
            enabled: true,
        };
        let json = serde_json::to_string(&channel).unwrap();
        assert!(json.contains("webhook"));
        assert!(json.contains("hooks.slack.com"));
    }

    #[test]
    fn test_sse_alert_event_serialization() {
        let event = SseAlertEvent {
            event_type: "alert_firing".into(),
            alert: ActiveAlert {
                id: "a-1".into(),
                rule_id: "r-1".into(),
                rule_name: "TestRule".into(),
                scope: AlertScope::Platform,
                severity: AlertSeverity::Warning,
                message: "test".into(),
                fired_at: "2026-03-07T00:00:00Z".into(),
                acknowledged: false,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("alert_firing"));
        assert!(json.contains("TestRule"));
    }

    #[test]
    fn test_create_rule_request_deserialization() {
        let json = r#"{
            "name": "CustomHighCPU",
            "scope": { "type": "App", "app_id": "my-app" },
            "severity": "warning",
            "query": { "type": "prom", "promql": "rate(cpu[5m]) > 0.8" }
        }"#;
        let req: CreateRuleRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "CustomHighCPU");
        assert_eq!(req.scope, AlertScope::App("my-app".into()));
        assert!(req.for_duration_secs.is_none());
        assert!(req.channels.is_none());
    }

    #[test]
    fn test_default_rules_severity_distribution() {
        let rules = default_rules();
        let critical = rules
            .iter()
            .filter(|r| r.severity == AlertSeverity::Critical)
            .count();
        let warning = rules
            .iter()
            .filter(|r| r.severity == AlertSeverity::Warning)
            .count();
        // Platform: 4 critical + 2 warning = 6
        // App: 1 critical + 7 warning = 8
        assert_eq!(critical, 5, "Expected 5 critical rules");
        assert_eq!(warning, 9, "Expected 9 warning rules");
        assert_eq!(critical + warning, 14);
    }

    #[test]
    fn test_alert_state_summary_counts() {
        let mut state = AlertState::with_defaults();
        state.active_alerts.push(ActiveAlert {
            id: "a1".into(),
            rule_id: "r1".into(),
            rule_name: "Rule1".into(),
            scope: AlertScope::Platform,
            severity: AlertSeverity::Critical,
            message: "critical".into(),
            fired_at: "2026-03-07T00:00:00Z".into(),
            acknowledged: false,
        });
        state.active_alerts.push(ActiveAlert {
            id: "a2".into(),
            rule_id: "r2".into(),
            rule_name: "Rule2".into(),
            scope: AlertScope::App("app1".into()),
            severity: AlertSeverity::Warning,
            message: "warning".into(),
            fired_at: "2026-03-07T00:00:00Z".into(),
            acknowledged: false,
        });
        state.active_alerts.push(ActiveAlert {
            id: "a3".into(),
            rule_id: "r3".into(),
            rule_name: "Rule3".into(),
            scope: AlertScope::Platform,
            severity: AlertSeverity::Critical,
            message: "acked".into(),
            fired_at: "2026-03-07T00:00:00Z".into(),
            acknowledged: true,
        });

        let critical = state
            .active_alerts
            .iter()
            .filter(|a| a.severity == AlertSeverity::Critical && !a.acknowledged)
            .count();
        let warning = state
            .active_alerts
            .iter()
            .filter(|a| a.severity == AlertSeverity::Warning && !a.acknowledged)
            .count();

        assert_eq!(critical, 1);
        assert_eq!(warning, 1);
        assert_eq!(state.active_alerts.len(), 3);
    }

    #[test]
    fn test_active_alerts_scope_filtering() {
        let mut state = AlertState::with_defaults();
        state.active_alerts.push(ActiveAlert {
            id: "a1".into(),
            rule_id: "r1".into(),
            rule_name: "PlatformAlert".into(),
            scope: AlertScope::Platform,
            severity: AlertSeverity::Critical,
            message: "platform issue".into(),
            fired_at: "2026-03-07T00:00:00Z".into(),
            acknowledged: false,
        });
        state.active_alerts.push(ActiveAlert {
            id: "a2".into(),
            rule_id: "r2".into(),
            rule_name: "AppAlert1".into(),
            scope: AlertScope::App("app-1".into()),
            severity: AlertSeverity::Warning,
            message: "app1 issue".into(),
            fired_at: "2026-03-07T00:00:00Z".into(),
            acknowledged: false,
        });
        state.active_alerts.push(ActiveAlert {
            id: "a3".into(),
            rule_id: "r3".into(),
            rule_name: "AppAlert2".into(),
            scope: AlertScope::App("app-2".into()),
            severity: AlertSeverity::Warning,
            message: "app2 issue".into(),
            fired_at: "2026-03-07T00:00:00Z".into(),
            acknowledged: false,
        });

        // Filter for platform
        let platform: Vec<_> = state
            .active_alerts
            .iter()
            .filter(|a| a.scope == AlertScope::Platform)
            .collect();
        assert_eq!(platform.len(), 1);

        // Filter for specific app
        let app1: Vec<_> = state
            .active_alerts
            .iter()
            .filter(|a| a.scope == AlertScope::App("app-1".into()))
            .collect();
        assert_eq!(app1.len(), 1);
        assert_eq!(app1[0].rule_name, "AppAlert1");

        // No filter = all
        assert_eq!(state.active_alerts.len(), 3);
    }

    #[test]
    fn test_rule_crud_operations() {
        let mut state = AlertState::with_defaults();
        let initial_count = state.rules.len();
        assert_eq!(initial_count, 14);

        // Create
        let new_rule = AlertRule {
            id: "user-custom-1".into(),
            name: "CustomRule".into(),
            scope: AlertScope::App("my-app".into()),
            severity: AlertSeverity::Warning,
            query: AlertQuery::Prom {
                promql: "custom_metric > 100".into(),
            },
            for_duration_secs: 60,
            enabled: true,
            channels: vec![],
        };
        state.rules.push(new_rule);
        assert_eq!(state.rules.len(), 15);

        // Update
        let rule = state
            .rules
            .iter_mut()
            .find(|r| r.id == "user-custom-1")
            .unwrap();
        rule.name = "UpdatedCustomRule".into();
        rule.enabled = false;
        let updated = state
            .rules
            .iter()
            .find(|r| r.id == "user-custom-1")
            .unwrap();
        assert_eq!(updated.name, "UpdatedCustomRule");
        assert!(!updated.enabled);

        // Delete
        state.rules.retain(|r| r.id != "user-custom-1");
        assert_eq!(state.rules.len(), 14);
        assert!(state
            .rules
            .iter()
            .find(|r| r.id == "user-custom-1")
            .is_none());
    }

    #[test]
    fn test_broadcast_channel_in_alert_state() {
        let state = AlertState::with_defaults();
        let mut rx = state.sse_tx.subscribe();

        let event = SseAlertEvent {
            event_type: "alert_firing".into(),
            alert: ActiveAlert {
                id: "a1".into(),
                rule_id: "r1".into(),
                rule_name: "Test".into(),
                scope: AlertScope::Platform,
                severity: AlertSeverity::Critical,
                message: "test".into(),
                fired_at: "2026-03-07T00:00:00Z".into(),
                acknowledged: false,
            },
        };

        state.sse_tx.send(event.clone()).unwrap();
        let received = rx.try_recv().unwrap();
        assert_eq!(received.event_type, "alert_firing");
        assert_eq!(received.alert.id, "a1");
    }
}
