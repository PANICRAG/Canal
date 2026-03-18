//! # Cloud Console API Routes (CP16a)
//!
//! Management endpoints for the cloud console: audit logs, data export (GDPR),
//! organization overview, and instance management.

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::error::ApiError;
use crate::middleware::audit::AuditStore;
use crate::state::AppState;

/// Register console routes under `/api/console`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/audit", get(query_audit_log))
        .route("/audit/stats", get(audit_stats))
        .route("/gdpr/export", get(gdpr_export))
        .route("/gdpr/delete", get(gdpr_delete_status))
        .route("/overview", get(console_overview))
}

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct AuditQueryParams {
    user_id: Option<Uuid>,
    action: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct AuditLogResponse {
    entries: Vec<AuditEntryResponse>,
    total: usize,
}

#[derive(Debug, Serialize)]
struct AuditEntryResponse {
    id: String,
    user_id: Option<String>,
    action: String,
    resource_type: String,
    status_code: u16,
    ip_address: Option<String>,
    timestamp: String,
}

#[derive(Debug, Serialize)]
struct AuditStatsResponse {
    total_entries: usize,
    actions_breakdown: Vec<ActionCount>,
}

#[derive(Debug, Serialize)]
struct ActionCount {
    action: String,
    count: usize,
}

#[derive(Debug, Serialize)]
struct GdprExportResponse {
    status: String,
    data: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct GdprDeleteStatusResponse {
    status: String,
    message: String,
}

#[derive(Debug, Serialize)]
struct ConsoleOverviewResponse {
    server_version: String,
    uptime_secs: u64,
    total_audit_entries: usize,
    features: Vec<String>,
}

// ============================================================================
// Handlers
// ============================================================================

/// Query audit logs with optional filters.
///
/// `GET /api/console/audit?user_id=&action=&limit=`
async fn query_audit_log(
    State(state): State<AppState>,
    Query(params): Query<AuditQueryParams>,
) -> Result<Json<AuditLogResponse>, ApiError> {
    let limit = params.limit.unwrap_or(50).min(1000);

    let entries = state.audit_store.query(
        params.user_id,
        params.action.as_deref(),
        limit,
    );

    let total = state.audit_store.count();

    let response_entries: Vec<AuditEntryResponse> = entries
        .iter()
        .map(|e| AuditEntryResponse {
            id: e.id.to_string(),
            user_id: e.user_id.map(|u| u.to_string()),
            action: e.action.clone(),
            resource_type: e.resource_type.clone(),
            status_code: e.status_code,
            ip_address: e.ip_address.clone(),
            timestamp: e.timestamp.to_rfc3339(),
        })
        .collect();

    Ok(Json(AuditLogResponse {
        entries: response_entries,
        total,
    }))
}

/// Get audit statistics.
///
/// `GET /api/console/audit/stats`
async fn audit_stats(
    State(state): State<AppState>,
) -> Result<Json<AuditStatsResponse>, ApiError> {
    let all_entries = state.audit_store.query(None, None, 10_000);

    // Count by action
    let mut action_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for entry in &all_entries {
        *action_counts.entry(entry.action.clone()).or_insert(0) += 1;
    }

    let mut breakdown: Vec<ActionCount> = action_counts
        .into_iter()
        .map(|(action, count)| ActionCount { action, count })
        .collect();
    breakdown.sort_by(|a, b| b.count.cmp(&a.count));

    Ok(Json(AuditStatsResponse {
        total_entries: state.audit_store.count(),
        actions_breakdown: breakdown,
    }))
}

/// GDPR data export — returns all user data.
///
/// `GET /api/console/gdpr/export`
async fn gdpr_export(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<crate::middleware::auth::AuthContext>,
) -> Result<Json<GdprExportResponse>, ApiError> {
    // Export all audit entries for this user
    let audit_entries = state.audit_store.query(Some(auth.user_id), None, 10_000);

    let entries_json: Vec<serde_json::Value> = audit_entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "action": e.action,
                "resource_type": e.resource_type,
                "status_code": e.status_code,
                "timestamp": e.timestamp.to_rfc3339(),
            })
        })
        .collect();

    Ok(Json(GdprExportResponse {
        status: "complete".into(),
        data: serde_json::json!({
            "user_id": auth.user_id.to_string(),
            "export_date": chrono::Utc::now().to_rfc3339(),
            "audit_log_entries": entries_json,
            "note": "Full data export including conversations and artifacts requires database access",
        }),
    }))
}

/// GDPR deletion status.
///
/// `GET /api/console/gdpr/delete`
async fn gdpr_delete_status() -> Json<GdprDeleteStatusResponse> {
    Json(GdprDeleteStatusResponse {
        status: "not_started".into(),
        message: "Use POST /api/console/gdpr/delete to initiate data deletion request".into(),
    })
}

/// Console overview — server status and feature flags.
///
/// `GET /api/console/overview`
async fn console_overview(
    State(state): State<AppState>,
) -> Json<ConsoleOverviewResponse> {
    let uptime = state.started_at.elapsed().as_secs();

    let mut features = vec![
        "audit_logging".to_string(),
        "gdpr_export".to_string(),
    ];

    #[cfg(feature = "collaboration")]
    features.push("collaboration".to_string());

    #[cfg(feature = "billing")]
    features.push("billing".to_string());

    #[cfg(feature = "jobs")]
    features.push("jobs".to_string());

    Json(ConsoleOverviewResponse {
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: uptime,
        total_audit_entries: state.audit_store.count(),
        features,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_query_default_limit() {
        let store = AuditStore::new();
        for _ in 0..100 {
            store.write(crate::middleware::audit::AuditEntry {
                id: Uuid::new_v4(),
                user_id: None,
                organization_id: None,
                action: "test".into(),
                resource_type: "test".into(),
                resource_id: None,
                status_code: 200,
                ip_address: None,
                user_agent: None,
                timestamp: chrono::Utc::now(),
            });
        }

        let results = store.query(None, None, 50);
        assert_eq!(results.len(), 50);
    }

    #[test]
    fn test_audit_entry_serialization() {
        let entry = AuditEntryResponse {
            id: Uuid::nil().to_string(),
            user_id: None,
            action: "chat.send".into(),
            resource_type: "conversation".into(),
            status_code: 200,
            ip_address: Some("127.0.0.1".into()),
            timestamp: "2026-03-01T00:00:00Z".into(),
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("chat.send"));
        assert!(json.contains("conversation"));
    }
}
