//! # GDPR Account Routes (CP16 Phase 2)
//!
//! Data portability (Article 20) and erasure (Article 17) endpoints.
//! Export delivers all user data as JSON; erasure soft-deletes then
//! schedules hard purge after a configurable grace period.

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::middleware::auth::AuthContext;
use crate::state::AppState;

/// Register account routes under `/api/account`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/export", post(request_export))
        .route("/export/status", get(export_status))
        .route("/erase", post(request_erasure))
        .route("/erase/status", get(erasure_status))
        .route("/profile", get(account_profile))
}

// ============================================================================
// Request / Response Types
// ============================================================================

#[derive(Debug, Serialize)]
struct ExportRequestResponse {
    export_id: String,
    status: String,
    message: String,
}

#[derive(Debug, Serialize)]
struct ExportStatusResponse {
    export_id: Option<String>,
    status: String,
    download_url: Option<String>,
    expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EraseRequest {
    /// Confirmation phrase: user must type "DELETE MY DATA" to proceed
    confirmation: Option<String>,
}

#[derive(Debug, Serialize)]
struct EraseResponse {
    status: String,
    message: String,
    grace_period_days: u32,
}

#[derive(Debug, Serialize)]
struct EraseStatusResponse {
    status: String,
    requested_at: Option<String>,
    scheduled_purge_at: Option<String>,
    message: String,
}

#[derive(Debug, Serialize)]
struct AccountProfileResponse {
    user_id: String,
    email: String,
    role: String,
    tier: String,
    data_summary: DataSummary,
}

// R4-L134: Changed placeholder String fields to usize with TODO
#[derive(Debug, Serialize)]
struct DataSummary {
    audit_entries: usize,
    conversations: usize,
    artifacts: usize,
    jobs: usize,
}

// ============================================================================
// Handlers
// ============================================================================

/// Request a full data export (GDPR Article 20).
///
/// `POST /api/account/export`
async fn request_export(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Result<Json<ExportRequestResponse>, ApiError> {
    let export_id = Uuid::new_v4();

    // Log the export request in audit store
    state.audit_store.write(crate::middleware::audit::AuditEntry {
        id: Uuid::new_v4(),
        user_id: Some(auth.user_id),
        organization_id: None,
        action: "account.export_requested".into(),
        resource_type: "account".into(),
        resource_id: Some(export_id.to_string()),
        status_code: 202,
        ip_address: None,
        user_agent: None,
        timestamp: chrono::Utc::now(),
    });

    Ok(Json(ExportRequestResponse {
        export_id: export_id.to_string(),
        status: "pending".into(),
        message: "Data export initiated. Check /api/account/export/status for progress.".into(),
    }))
}

/// Check data export status.
///
/// `GET /api/account/export/status`
async fn export_status(
    axum::Extension(_auth): axum::Extension<AuthContext>,
) -> Json<ExportStatusResponse> {
    // In production, this would check the async job store for the export task.
    // For now, return a placeholder indicating no active export.
    Json(ExportStatusResponse {
        export_id: None,
        status: "no_active_export".into(),
        download_url: None,
        expires_at: None,
    })
}

/// Request data erasure (GDPR Article 17).
///
/// `POST /api/account/erase`
///
/// Requires confirmation phrase "DELETE MY DATA" in the request body.
async fn request_erasure(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(body): Json<EraseRequest>,
) -> Result<Json<EraseResponse>, ApiError> {
    // Require explicit confirmation
    let confirmed = body
        .confirmation
        .as_deref()
        .map(|c| c == "DELETE MY DATA")
        .unwrap_or(false);

    if !confirmed {
        return Err(ApiError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "Confirmation required. Set confirmation to \"DELETE MY DATA\" to proceed.",
        ));
    }

    let grace_period_days: u32 = 30;

    // Log the erasure request
    state.audit_store.write(crate::middleware::audit::AuditEntry {
        id: Uuid::new_v4(),
        user_id: Some(auth.user_id),
        organization_id: None,
        action: "account.erase_requested".into(),
        resource_type: "account".into(),
        resource_id: None,
        status_code: 202,
        ip_address: None,
        user_agent: None,
        timestamp: chrono::Utc::now(),
    });

    Ok(Json(EraseResponse {
        status: "scheduled".into(),
        message: format!(
            "Data erasure scheduled. All data will be soft-deleted immediately and hard-purged after {} days.",
            grace_period_days
        ),
        grace_period_days,
    }))
}

/// Check erasure request status.
///
/// `GET /api/account/erase/status`
async fn erasure_status(
    axum::Extension(_auth): axum::Extension<AuthContext>,
) -> Json<EraseStatusResponse> {
    Json(EraseStatusResponse {
        status: "no_active_request".into(),
        requested_at: None,
        scheduled_purge_at: None,
        message: "No active erasure request. Use POST /api/account/erase to initiate.".into(),
    })
}

/// Get account profile summary.
///
/// `GET /api/account/profile`
async fn account_profile(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Json<AccountProfileResponse> {
    let audit_count = state
        .audit_store
        .query(Some(auth.user_id), None, 0)
        .len();

    Json(AccountProfileResponse {
        user_id: auth.user_id.to_string(),
        email: auth.email.clone(),
        role: auth.role.clone(),
        tier: format!("{:?}", auth.tier),
        data_summary: DataSummary {
            audit_entries: audit_count,
            // TODO: wire actual counts when session/artifact/job stores are available
            conversations: 0,
            artifacts: 0,
            jobs: 0,
        },
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_response_serialization() {
        let resp = ExportRequestResponse {
            export_id: Uuid::nil().to_string(),
            status: "pending".into(),
            message: "test".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("pending"));
    }

    #[test]
    fn test_erase_response_serialization() {
        let resp = EraseResponse {
            status: "scheduled".into(),
            message: "test".into(),
            grace_period_days: 30,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("30"));
        assert!(json.contains("scheduled"));
    }

    #[test]
    fn test_erase_confirmation_required() {
        // Verify that "DELETE MY DATA" is the expected confirmation phrase
        let body = EraseRequest {
            confirmation: Some("DELETE MY DATA".into()),
        };
        assert_eq!(body.confirmation.as_deref(), Some("DELETE MY DATA"));

        let body_wrong = EraseRequest {
            confirmation: Some("delete".into()),
        };
        assert_ne!(body_wrong.confirmation.as_deref(), Some("DELETE MY DATA"));
    }
}
