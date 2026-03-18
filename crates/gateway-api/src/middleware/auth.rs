//! Authentication middleware — re-exports from canal-auth shared crate.
//!
//! All auth primitives (Claims, AuthContext, UserTier, JWT validation, middleware)
//! live in the canal-auth crate. This module re-exports them to preserve
//! existing import paths throughout gateway-api.

pub use canal_auth::supabase;
pub use canal_auth::{jwt_secret, validate_jwt, Claims};
pub use canal_auth::{AuthContext, UserTier};

/// Auth middleware that adds gateway-api-specific metrics recording.
///
/// Wraps canal_auth::auth_middleware and adds metrics counter calls.
pub async fn auth_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    // Delegate to the shared auth middleware
    canal_auth::auth_middleware(request, next).await
}
