//! # Audit Middleware (CP16 Phase 1)
//!
//! Captures request metadata and writes audit entries after the response is produced.
//! Non-blocking — audit writes are spawned as background tasks.

use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

use super::auth::AuthContext;

// ============================================================================
// Types
// ============================================================================

/// A single audit log entry.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub organization_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub status_code: u16,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// In-memory audit store for development and testing.
#[derive(Debug, Clone, Default)]
pub struct AuditStore {
    entries: Arc<std::sync::RwLock<Vec<AuditEntry>>>,
}

impl AuditStore {
    /// Create a new empty audit store.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(std::sync::RwLock::new(Vec::new())),
        }
    }

    /// Write an audit entry.
    pub fn write(&self, entry: AuditEntry) {
        match self.entries.write() {
            Ok(mut entries) => {
                entries.push(entry);
                // Keep last 10000 entries
                let len = entries.len();
                if len > 10_000 {
                    entries.drain(..len - 10_000);
                }
            }
            Err(e) => tracing::error!("Audit store write lock poisoned: {}", e),
        }
    }

    /// Query audit entries with optional filters.
    pub fn query(
        &self,
        user_id: Option<Uuid>,
        action: Option<&str>,
        limit: usize,
    ) -> Vec<AuditEntry> {
        match self.entries.read() {
            Ok(entries) => entries
                .iter()
                .rev()
                .filter(|e| {
                    user_id.map_or(true, |uid| e.user_id == Some(uid))
                        && action.map_or(true, |a| e.action.starts_with(a))
                })
                .take(limit)
                .cloned()
                .collect(),
            Err(e) => {
                tracing::error!("Audit store read lock poisoned: {}", e);
                Vec::new()
            }
        }
    }

    /// Count total entries.
    pub fn count(&self) -> usize {
        self.entries.read().map(|e| e.len()).unwrap_or(0)
    }
}

// ============================================================================
// Action Classification
// ============================================================================

/// Derive the action name from HTTP method + path.
fn derive_action(method: &str, path: &str) -> String {
    match (method, path) {
        ("POST", p) if p.starts_with("/api/chat") => "chat.send".into(),
        ("POST", p) if p.contains("/plan-approval") => "chat.plan_approval".into(),
        ("POST", "/api/jobs") => "job.submit".into(),
        ("POST", p) if p.contains("/cancel") => "job.cancel".into(),
        ("POST", p) if p.contains("/instruct") => "job.instruct".into(),
        ("POST", p) if p.contains("/step-result") => "job.step_result".into(),
        ("POST", p) if p.starts_with("/api/tools") => "tool.execute".into(),
        ("POST", "/api/auth/login") => "auth.login".into(),
        ("POST", "/api/auth/register") => "auth.register".into(),
        ("DELETE", p) if p.starts_with("/api/auth/sessions") => "auth.revoke_session".into(),
        ("POST", p) if p.starts_with("/api/mcp") => "mcp.action".into(),
        ("PUT", p) if p.starts_with("/api/permissions") => "permission.update".into(),
        ("POST", p) if p.starts_with("/api/admin") => "admin.action".into(),
        ("GET", _) => "read".into(),
        ("POST", _) => "write".into(),
        ("PUT", _) => "update".into(),
        ("DELETE", _) => "delete".into(),
        _ => "unknown".into(),
    }
}

/// Derive resource type from path.
fn derive_resource_type(path: &str) -> String {
    if path.starts_with("/api/chat") {
        "conversation"
    } else if path.starts_with("/api/jobs") {
        "job"
    } else if path.starts_with("/api/tools") {
        "tool"
    } else if path.starts_with("/api/auth") {
        "auth"
    } else if path.starts_with("/api/mcp") {
        "mcp_server"
    } else if path.starts_with("/api/permissions") {
        "permission"
    } else if path.starts_with("/api/admin") {
        "system"
    } else if path.starts_with("/api/browser") {
        "browser"
    } else if path.starts_with("/api/usage") {
        "usage"
    } else {
        "unknown"
    }
    .into()
}

// ============================================================================
// Middleware
// ============================================================================

/// Audit middleware that logs all API requests.
///
/// Install after auth middleware so AuthContext is available.
pub async fn audit_middleware(
    audit_store: Option<axum::Extension<Arc<AuditStore>>>,
    auth: Option<axum::Extension<AuthContext>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let ip = request
        .headers()
        .get("x-forwarded-for")
        .or_else(|| request.headers().get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let user_agent = request
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let user_id = auth.as_ref().map(|a| a.user_id);
    // AuthContext does not yet carry organization_id — default to None
    let org_id: Option<Uuid> = None;

    let response = next.run(request).await;

    let status_code = response.status().as_u16();

    // Fire-and-forget audit write
    if let Some(axum::Extension(store)) = audit_store {
        let action = derive_action(&method, &path);
        let resource_type = derive_resource_type(&path);

        let entry = AuditEntry {
            id: Uuid::new_v4(),
            user_id,
            organization_id: org_id,
            action,
            resource_type,
            resource_id: None,
            status_code,
            ip_address: ip,
            user_agent,
            timestamp: chrono::Utc::now(),
        };

        tokio::spawn(async move {
            store.write(entry);
        });
    }

    response
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_store_write_query() {
        let store = AuditStore::new();

        let user_id = Uuid::new_v4();
        store.write(AuditEntry {
            id: Uuid::new_v4(),
            user_id: Some(user_id),
            organization_id: None,
            action: "chat.send".into(),
            resource_type: "conversation".into(),
            resource_id: None,
            status_code: 200,
            ip_address: Some("127.0.0.1".into()),
            user_agent: Some("test".into()),
            timestamp: chrono::Utc::now(),
        });

        assert_eq!(store.count(), 1);

        let results = store.query(Some(user_id), None, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action, "chat.send");
    }

    #[test]
    fn test_audit_store_filter_by_action() {
        let store = AuditStore::new();
        let user_id = Uuid::new_v4();

        for action in &["chat.send", "chat.send", "auth.login", "job.submit"] {
            store.write(AuditEntry {
                id: Uuid::new_v4(),
                user_id: Some(user_id),
                organization_id: None,
                action: action.to_string(),
                resource_type: "test".into(),
                resource_id: None,
                status_code: 200,
                ip_address: None,
                user_agent: None,
                timestamp: chrono::Utc::now(),
            });
        }

        let chat_entries = store.query(None, Some("chat"), 10);
        assert_eq!(chat_entries.len(), 2);

        let auth_entries = store.query(None, Some("auth"), 10);
        assert_eq!(auth_entries.len(), 1);
    }

    #[test]
    fn test_action_classification() {
        assert_eq!(derive_action("POST", "/api/chat/stream"), "chat.send");
        assert_eq!(derive_action("POST", "/api/jobs"), "job.submit");
        assert_eq!(derive_action("POST", "/api/auth/login"), "auth.login");
        assert_eq!(
            derive_action("DELETE", "/api/auth/sessions/123"),
            "auth.revoke_session"
        );
        assert_eq!(derive_action("POST", "/api/tools/execute"), "tool.execute");
        assert_eq!(derive_action("GET", "/api/health"), "read");
    }

    #[test]
    fn test_resource_type_classification() {
        assert_eq!(derive_resource_type("/api/chat/stream"), "conversation");
        assert_eq!(derive_resource_type("/api/jobs/123"), "job");
        assert_eq!(derive_resource_type("/api/auth/login"), "auth");
        assert_eq!(derive_resource_type("/api/mcp/servers"), "mcp_server");
    }

    #[test]
    fn test_audit_store_limit() {
        let store = AuditStore::new();
        for _ in 0..5 {
            store.write(AuditEntry {
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

        let results = store.query(None, None, 3);
        assert_eq!(results.len(), 3);
    }
}
