//! # RBAC Middleware (CP16 Phase 2)
//!
//! Enforces scope-based access control derived from user role + tier.
//! Reads scope requirements from route path + method; checks against
//! the authenticated user's permissions.

use axum::{extract::Request, middleware::Next, response::Response};

use super::auth::AuthContext;
use crate::error::ApiError;

// ============================================================================
// Scope Derivation
// ============================================================================

/// Derive the required scope from HTTP method + path.
///
/// Returns `None` for open endpoints (health, auth login/register).
fn derive_scope(method: &str, path: &str) -> Option<&'static str> {
    // Public endpoints — no scope required
    if path.starts_with("/api/health") || path == "/api/auth/login" || path == "/api/auth/register"
    {
        return None;
    }

    match (method, path) {
        // Chat
        ("POST", p) if p.starts_with("/api/chat") => Some("chat:write"),
        ("GET", p) if p.starts_with("/api/chat") => Some("chat:read"),

        // Jobs
        ("POST", p) if p.starts_with("/api/jobs") => Some("jobs:write"),
        ("GET", p) if p.starts_with("/api/jobs") => Some("jobs:read"),

        // Tools
        ("POST", p) if p.starts_with("/api/tools") => Some("tools:execute"),

        // MCP
        ("POST", p) if p.starts_with("/api/mcp") => Some("mcp:write"),
        ("GET", p) if p.starts_with("/api/mcp") => Some("mcp:read"),

        // Admin
        ("POST", p) | ("PUT", p) | ("DELETE", p) if p.starts_with("/api/admin") => {
            Some("admin:write")
        }
        ("GET", p) if p.starts_with("/api/admin") => Some("admin:read"),

        // Console (CP16a)
        ("GET", p) if p.starts_with("/api/console") => Some("console:read"),

        // Permissions
        ("PUT", p) if p.starts_with("/api/permissions") => Some("permissions:write"),

        // Everything else — require basic authenticated access
        ("GET", _) => Some("read"),
        _ => Some("write"),
    }
}

/// Check if the user's role + permissions satisfy the required scope.
fn has_scope(auth: &AuthContext, scope: &str) -> bool {
    // Admin role has all scopes
    if auth.role == "admin" || auth.permissions.contains(&"*".to_string()) {
        return true;
    }

    // Check direct permission match
    if auth.permissions.contains(&scope.to_string()) {
        return true;
    }

    // Check wildcard namespace match (e.g., "chat:*" covers "chat:read" and "chat:write")
    if let Some(namespace) = scope.split(':').next() {
        let wildcard = format!("{namespace}:*");
        if auth.permissions.contains(&wildcard) {
            return true;
        }
    }

    // Default tier-based access: basic read/write scopes are available to all authenticated users
    matches!(
        scope,
        "read" | "write" | "chat:read" | "chat:write" | "tools:execute"
    )
}

// ============================================================================
// Middleware
// ============================================================================

/// RBAC middleware that enforces scope-based access control.
///
/// Install after auth middleware so AuthContext is available.
/// When AuthContext is missing (unauthenticated), the middleware passes through —
/// auth middleware handles the 401 rejection.
pub async fn rbac_middleware(
    auth: Option<axum::Extension<AuthContext>>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();

    // Derive required scope
    let required_scope = derive_scope(&method, &path);

    // If no scope required (public endpoint), pass through
    let Some(scope) = required_scope else {
        return Ok(next.run(request).await);
    };

    // If auth context is available, check scope
    if let Some(axum::Extension(ref auth_ctx)) = auth {
        if !has_scope(auth_ctx, scope) {
            return Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                format!(
                    "Scope '{}' required. Your role '{}' does not include it.",
                    scope, auth_ctx.role
                ),
            ));
        }
    }
    // If no auth context (unauthenticated), let it pass through to auth middleware rejection

    Ok(next.run(request).await)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::auth::UserTier;
    use uuid::Uuid;

    fn make_auth(role: &str, permissions: Vec<&str>) -> AuthContext {
        AuthContext {
            user_id: Uuid::new_v4(),
            org_id: None,
            email: "test@example.com".into(),
            role: role.into(),
            permissions: permissions.into_iter().map(|s| s.to_string()).collect(),
            tier: UserTier::Pro,
            session_id: None,
        }
    }

    #[test]
    fn test_admin_has_all_scopes() {
        let auth = make_auth("admin", vec![]);
        assert!(has_scope(&auth, "admin:write"));
        assert!(has_scope(&auth, "mcp:read"));
        assert!(has_scope(&auth, "anything"));
    }

    #[test]
    fn test_wildcard_permission() {
        let auth = make_auth("user", vec!["*"]);
        assert!(has_scope(&auth, "admin:write"));
    }

    #[test]
    fn test_namespace_wildcard() {
        let auth = make_auth("user", vec!["mcp:*"]);
        assert!(has_scope(&auth, "mcp:read"));
        assert!(has_scope(&auth, "mcp:write"));
        assert!(!has_scope(&auth, "admin:write"));
    }

    #[test]
    fn test_default_tier_access() {
        let auth = make_auth("user", vec![]);
        // Basic scopes available to all authenticated users
        assert!(has_scope(&auth, "chat:read"));
        assert!(has_scope(&auth, "chat:write"));
        assert!(has_scope(&auth, "tools:execute"));
        // Admin scopes not available
        assert!(!has_scope(&auth, "admin:write"));
        assert!(!has_scope(&auth, "mcp:write"));
    }

    #[test]
    fn test_scope_derivation() {
        assert_eq!(derive_scope("POST", "/api/chat/send"), Some("chat:write"));
        assert_eq!(derive_scope("GET", "/api/jobs/123"), Some("jobs:read"));
        assert_eq!(
            derive_scope("POST", "/api/tools/execute"),
            Some("tools:execute")
        );
        assert_eq!(
            derive_scope("POST", "/api/admin/users"),
            Some("admin:write")
        );
        assert_eq!(derive_scope("GET", "/api/health"), None);
        assert_eq!(derive_scope("POST", "/api/auth/login"), None);
    }

    #[test]
    fn test_console_scope() {
        assert_eq!(
            derive_scope("GET", "/api/console/audit"),
            Some("console:read")
        );
        let auth = make_auth("user", vec!["console:read"]);
        assert!(has_scope(&auth, "console:read"));
    }
}
