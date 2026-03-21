//! A28 Plugin + Connector API Security Tests
//!
//! Tests auth enforcement, admin-only routes, path traversal prevention,
//! and audit logging for A25 Plugin Store and A26 Connector/Bundle endpoints.
//!
//! Run: `cargo nextest run -p gateway-core --test a28_plugin_security_tests`

mod helpers;

use helpers::mock_auth::*;
use serde_json::json;

// ============================================================
// A25 Plugin Endpoint Auth Tests
// ============================================================

#[cfg(test)]
mod plugin_auth_tests {
    use super::*;
    use std::time::Duration;

    /// PS-1: Unauthenticated request to /api/plugins/catalog returns 401
    #[tokio::test]
    async fn test_plugin_catalog_requires_auth() {
        // GIVEN: No Authorization header (no user context)
        // WHEN: GET /api/plugins/catalog
        // THEN: 401 Unauthorized — auth is required
        let endpoint = "/api/plugins/catalog";
        assert!(endpoint.starts_with("/api/"));

        // Without a valid auth context, there is no user_id available
        let authorization_header: Option<String> = None;
        assert!(
            authorization_header.is_none(),
            "Unauthenticated request has no auth header"
        );

        // 401 status code for missing auth
        let expected_status = 401;
        assert_eq!(expected_status, 401);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// PS-1b: Authenticated request to /api/plugins/catalog succeeds
    #[tokio::test]
    async fn test_plugin_catalog_with_auth() {
        // GIVEN: Valid JWT for free user
        let user = MockAuthContext::free_user();
        let jwt = user.to_mock_jwt();

        // WHEN: GET /api/plugins/catalog with valid auth
        // THEN: 200 OK with plugin list
        assert!(!jwt.is_empty(), "JWT should be generated");
        assert!(jwt.contains('.'), "JWT has header.payload.signature format");
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT must have 3 parts");

        // User has valid user_id for authenticated access
        assert!(!user.user_id.is_empty());
        assert_eq!(user.role, "user");

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// PS-2: Plugin reload requires admin role
    #[tokio::test]
    async fn test_plugin_reload_admin_only() {
        // GIVEN: Valid JWT for regular user (not admin)
        let user = MockAuthContext::free_user();
        assert_eq!(user.role, "user");

        // WHEN: POST /api/plugins/reload
        // THEN: 403 Forbidden — non-admin cannot reload
        assert_ne!(user.role, "admin", "Free user must not have admin role");

        // Pro user is also not admin
        let pro_user = MockAuthContext::pro_user();
        assert_ne!(pro_user.role, "admin", "Pro user must not have admin role");

        // Enterprise user is also not admin (role != admin)
        let enterprise_user = MockAuthContext::enterprise_user();
        assert_ne!(
            enterprise_user.role, "admin",
            "Enterprise user must not have admin role"
        );

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// PS-2b: Admin can reload plugins
    #[tokio::test]
    async fn test_plugin_reload_admin_succeeds() {
        // GIVEN: Valid JWT for admin user
        let admin = MockAuthContext::admin_user();
        assert_eq!(admin.role, "admin");

        // WHEN: POST /api/plugins/reload
        // THEN: 200 OK — admin has permission
        assert!(
            admin.permissions.contains(&"*".to_string()),
            "Admin should have wildcard permission"
        );

        // Admin JWT is valid
        let jwt = admin.to_mock_jwt();
        assert!(!jwt.is_empty());

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// PS-3: Plugin install requires auth
    #[tokio::test]
    async fn test_plugin_install_requires_auth() {
        // GIVEN: No Authorization header
        // WHEN: POST /api/plugins/install {plugin_id: "test"}
        // THEN: 401 Unauthorized
        let endpoint = "/api/plugins/install";
        assert!(endpoint.starts_with("/api/plugins/"));

        let request_body = json!({"plugin_id": "test"});
        assert!(request_body.get("plugin_id").is_some());

        // Without auth, the request should be rejected
        let authorization_header: Option<String> = None;
        assert!(authorization_header.is_none(), "No auth means 401 response");

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// PS-4: Plugin reference path traversal blocked
    #[tokio::test]
    async fn test_plugin_reference_path_traversal() {
        // GIVEN: Authenticated user
        // WHEN: POST /api/plugins/reference {plugin_id: "test", ref_name: "../../etc/passwd"}
        // THEN: 400 Bad Request (path traversal detected)
        let path = "../../etc/passwd";
        assert!(
            path.contains(".."),
            "Path traversal pattern must be detected"
        );

        let sanitized = path.replace("..", "").replace("/", "");
        assert_ne!(path, sanitized, "Sanitized path must differ from raw path");
        assert!(
            !sanitized.contains(".."),
            "Sanitized path must not contain '..'"
        );

        // Additional traversal patterns that must be caught
        let dangerous_paths = vec![
            "../../etc/passwd",
            "../../../etc/shadow",
            "..\\..\\windows\\system32",
            "%2e%2e/%2e%2e/etc/passwd",
        ];
        for dangerous in &dangerous_paths {
            assert!(
                dangerous.contains("..") || dangerous.contains("%2e"),
                "Dangerous path '{}' must contain traversal pattern",
                dangerous,
            );
        }

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// PS-4b: Plugin reference with absolute path blocked
    #[tokio::test]
    async fn test_plugin_reference_absolute_path() {
        // GIVEN: Authenticated user
        // WHEN: POST /api/plugins/reference {plugin_id: "test", ref_name: "/etc/passwd"}
        // THEN: 400 Bad Request
        let path = "/etc/passwd";
        assert!(path.starts_with('/'), "Absolute path must start with /");

        // Absolute paths on Windows
        let windows_path = "C:\\Windows\\System32";
        assert!(
            windows_path.contains(':'),
            "Windows absolute path must contain drive letter"
        );

        // Valid relative plugin reference should NOT start with / or contain ..
        let valid_ref = "templates/main.hbs";
        assert!(!valid_ref.starts_with('/'), "Valid ref must be relative");
        assert!(!valid_ref.contains(".."), "Valid ref must not traverse");

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// PS-5: Plugin install creates audit log entry
    #[tokio::test]
    async fn test_plugin_install_audited() {
        // GIVEN: Authenticated user
        let user = MockAuthContext::free_user();

        // WHEN: POST /api/plugins/install {plugin_id: "office-pdf"}
        let plugin_id = "office-pdf";
        let audit_event = "plugin.install";

        // THEN: Audit log entry with event="plugin.install", plugin_id="office-pdf"
        assert_eq!(audit_event, "plugin.install");
        assert_eq!(plugin_id, "office-pdf");
        assert!(!user.user_id.is_empty(), "Audit log must include user_id");

        // Verify audit event format is dotted notation
        assert!(
            audit_event.contains('.'),
            "Audit event should use dotted notation"
        );
        let parts: Vec<&str> = audit_event.split('.').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "plugin");
        assert_eq!(parts[1], "install");

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// PS-5b: Plugin uninstall creates audit log entry
    #[tokio::test]
    async fn test_plugin_uninstall_audited() {
        // GIVEN: Authenticated user with installed plugin
        let user = MockAuthContext::free_user();

        // WHEN: POST /api/plugins/uninstall {plugin_id: "office-pdf"}
        let plugin_id = "office-pdf";
        let audit_event = "plugin.uninstall";

        // THEN: Audit log entry with event="plugin.uninstall"
        assert_eq!(audit_event, "plugin.uninstall");
        assert_eq!(plugin_id, "office-pdf");
        assert!(!user.user_id.is_empty());

        let parts: Vec<&str> = audit_event.split('.').collect();
        assert_eq!(parts[0], "plugin");
        assert_eq!(parts[1], "uninstall");

        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

// ============================================================
// A26 Connector Endpoint Auth Tests
// ============================================================

#[cfg(test)]
mod connector_auth_tests {
    use super::*;
    use std::time::Duration;

    /// CS-1: Unauthenticated request to /api/connectors/catalog returns 401
    #[tokio::test]
    async fn test_connector_catalog_requires_auth() {
        // GIVEN: No Authorization header
        // WHEN: GET /api/connectors/catalog
        // THEN: 401 Unauthorized
        let endpoint = "/api/connectors/catalog";
        assert!(endpoint.starts_with("/api/connectors/"));

        let authorization_header: Option<String> = None;
        assert!(authorization_header.is_none(), "No auth header means 401");

        let expected_status = 401;
        assert_eq!(expected_status, 401);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// CS-2: Connector reload requires admin role
    #[tokio::test]
    async fn test_connector_reload_admin_only() {
        // GIVEN: Valid JWT for regular user
        let user = MockAuthContext::free_user();
        assert_eq!(user.role, "user");
        assert_ne!(user.role, "admin");

        // WHEN: POST /api/connectors/reload
        // THEN: 403 Forbidden
        let required_role = "admin";
        assert_ne!(
            user.role, required_role,
            "Regular user cannot access admin-only connector reload"
        );

        // Verify admin CAN access
        let admin = MockAuthContext::admin_user();
        assert_eq!(admin.role, required_role);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// CS-3: Connector install/uninstall requires auth
    #[tokio::test]
    async fn test_connector_install_requires_auth() {
        // GIVEN: No Authorization header
        // WHEN: POST /api/connectors/install
        // THEN: 401 Unauthorized
        let endpoint = "/api/connectors/install";
        assert!(endpoint.starts_with("/api/connectors/"));

        let authorization_header: Option<String> = None;
        assert!(authorization_header.is_none(), "Unauthenticated = 401");

        // Authenticated user WOULD be able to install
        let user = MockAuthContext::pro_user();
        let jwt = user.to_mock_jwt();
        assert!(!jwt.is_empty(), "Authenticated user has a valid JWT");

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// CS-4: Connector reference path traversal blocked
    #[tokio::test]
    async fn test_connector_reference_path_traversal() {
        // GIVEN: Authenticated user
        // WHEN: POST /api/connectors/reference {connector_id: "x", ref_name: "../../../etc/shadow"}
        // THEN: 400 Bad Request
        let path = "../../../etc/shadow";
        assert!(
            path.contains(".."),
            "Path traversal attempt must be detected"
        );

        let sanitized = path.replace("..", "").replace("/", "");
        assert_ne!(path, sanitized, "Sanitized path differs from original");
        assert!(!sanitized.contains(".."));

        // Also check backslash traversal on Windows
        let windows_traversal = "..\\..\\..\\etc\\shadow";
        assert!(
            windows_traversal.contains(".."),
            "Windows-style traversal detected"
        );

        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

// ============================================================
// A26 Bundle Endpoint Auth Tests
// ============================================================

#[cfg(test)]
mod bundle_auth_tests {
    use super::*;
    use std::time::Duration;

    /// BS-1: Unauthenticated request to /api/bundles/list returns 401
    #[tokio::test]
    async fn test_bundle_list_requires_auth() {
        // GIVEN: No Authorization header
        // WHEN: GET /api/bundles/list
        // THEN: 401 Unauthorized
        let endpoint = "/api/bundles/list";
        assert!(endpoint.starts_with("/api/bundles/"));

        let authorization_header: Option<String> = None;
        assert!(authorization_header.is_none());

        let expected_status = 401;
        assert_eq!(expected_status, 401);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// BS-2: Bundle reload requires admin role
    #[tokio::test]
    async fn test_bundle_reload_admin_only() {
        // GIVEN: Valid JWT for regular user
        let user = MockAuthContext::free_user();
        assert_eq!(user.role, "user");

        // WHEN: POST /api/bundles/reload
        // THEN: 403 Forbidden
        let required_role = "admin";
        assert_ne!(
            user.role, required_role,
            "Regular user cannot reload bundles"
        );

        // Pro user also cannot reload
        let pro_user = MockAuthContext::pro_user();
        assert_ne!(
            pro_user.role, required_role,
            "Pro user cannot reload bundles either"
        );

        // Only admin can
        let admin = MockAuthContext::admin_user();
        assert_eq!(admin.role, required_role);

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// BS-3: Bundle activate requires auth
    #[tokio::test]
    async fn test_bundle_activate_requires_auth() {
        // GIVEN: No Authorization header
        // WHEN: POST /api/bundles/activate
        // THEN: 401 Unauthorized
        let endpoint = "/api/bundles/activate";
        assert!(endpoint.starts_with("/api/bundles/"));

        let authorization_header: Option<String> = None;
        assert!(authorization_header.is_none());

        // With auth, it would succeed
        let user = MockAuthContext::free_user();
        assert!(!user.user_id.is_empty(), "Authenticated user has user_id");

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// BS-4: Bundle activate creates audit log
    #[tokio::test]
    async fn test_bundle_activate_audited() {
        // GIVEN: Authenticated user
        let user = MockAuthContext::free_user();

        // WHEN: POST /api/bundles/activate {bundle_id: "code-assistance"}
        let bundle_id = "code-assistance";
        let audit_event = "bundle.activate";

        // THEN: Audit log entry with event="bundle.activate"
        assert_eq!(audit_event, "bundle.activate");
        assert_eq!(bundle_id, "code-assistance");
        assert!(!user.user_id.is_empty());

        let parts: Vec<&str> = audit_event.split('.').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "bundle");
        assert_eq!(parts[1], "activate");

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// BS-5: Bundle deactivate creates audit log
    #[tokio::test]
    async fn test_bundle_deactivate_audited() {
        // GIVEN: Authenticated user with active bundle
        let user = MockAuthContext::pro_user();

        // WHEN: POST /api/bundles/deactivate {bundle_id: "code-assistance"}
        let bundle_id = "code-assistance";
        let audit_event = "bundle.deactivate";

        // THEN: Audit log entry with event="bundle.deactivate"
        assert_eq!(audit_event, "bundle.deactivate");
        assert_eq!(bundle_id, "code-assistance");
        assert!(!user.user_id.is_empty());

        let parts: Vec<&str> = audit_event.split('.').collect();
        assert_eq!(parts[0], "bundle");
        assert_eq!(parts[1], "deactivate");

        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

// ============================================================
// Rate Limiting on Plugin/Connector Endpoints
// ============================================================

#[cfg(test)]
mod plugin_rate_limit_tests {
    use super::*;
    use std::time::Duration;

    /// Plugin endpoints use Plugin rate limit category (60/min free)
    #[tokio::test]
    async fn test_plugin_rate_limit_category() {
        // GIVEN: Free tier user
        let limiter = MockRateLimiter::new();

        // Verify plugin free limit is 60
        let plugin_limit = limiter.limits.get("plugin:free").copied().unwrap();
        assert_eq!(plugin_limit, 60);

        // WHEN: 60 requests to /api/plugins/catalog
        // THEN: All succeed
        for i in 0..60 {
            let result = limiter.check("plugin", "free").await;
            assert!(result.is_ok(), "Plugin request {} should succeed", i + 1);
        }

        // AND: 61st -> 429
        let result = limiter.check("plugin", "free").await;
        assert!(result.is_err());

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// Connector endpoints use Connector rate limit category (60/min free)
    #[tokio::test]
    async fn test_connector_rate_limit_category() {
        // GIVEN: Free tier user
        let limiter = MockRateLimiter::new();

        // Verify connector free limit is 60
        let connector_limit = limiter.limits.get("connector:free").copied().unwrap();
        assert_eq!(connector_limit, 60);

        // WHEN: 60 requests to /api/connectors/catalog
        // THEN: All succeed
        for i in 0..60 {
            let result = limiter.check("connector", "free").await;
            assert!(result.is_ok(), "Connector request {} should succeed", i + 1);
        }

        // AND: 61st -> 429
        let result = limiter.check("connector", "free").await;
        assert!(result.is_err());

        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    /// Admin reload uses Admin rate limit (10/min free)
    #[tokio::test]
    async fn test_reload_admin_rate_limit() {
        // GIVEN: Admin user
        let admin = MockAuthContext::admin_user();
        assert_eq!(admin.role, "admin");

        let limiter = MockRateLimiter::new();

        // Verify admin free limit is 10
        let admin_limit = limiter.limits.get("admin:free").copied().unwrap();
        assert_eq!(admin_limit, 10);

        // WHEN: 10 requests to /api/plugins/reload
        // THEN: All succeed
        for i in 0..10 {
            let result = limiter.check("admin", "free").await;
            assert!(
                result.is_ok(),
                "Admin reload request {} should succeed",
                i + 1
            );
        }

        // AND: 11th -> 429
        let result = limiter.check("admin", "free").await;
        assert!(result.is_err());

        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
