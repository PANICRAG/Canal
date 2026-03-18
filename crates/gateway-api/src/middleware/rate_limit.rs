//! Rate limiting middleware.
//!
//! Applies per-user, per-endpoint-category, per-tier rate limits.
//! Adds X-RateLimit-* response headers for client visibility.

use axum::{
    extract::Request,
    http::{header::HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use gateway_core::rte::{EndpointCategory, RateLimitTier, RateLimiter};

use super::auth::AuthContext;

/// Convert auth middleware's UserTier to rate limiter's RateLimitTier.
fn to_rate_limit_tier(tier: &super::auth::UserTier) -> RateLimitTier {
    match tier {
        super::auth::UserTier::Free => RateLimitTier::Free,
        super::auth::UserTier::Pro => RateLimitTier::Pro,
        super::auth::UserTier::Enterprise => RateLimitTier::Enterprise,
    }
}

/// Derive a deterministic UUID from a client IP string for rate-limiting buckets.
/// Uses a simple hash to avoid requiring uuid v5 feature.
fn ip_to_uuid(ip: &str) -> uuid::Uuid {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ip.hash(&mut hasher);
    let hash = hasher.finish();
    // Build a deterministic UUID from the hash bytes
    let bytes = hash.to_le_bytes();
    let mut uuid_bytes = [0u8; 16];
    uuid_bytes[..8].copy_from_slice(&bytes);
    uuid_bytes[8..16].copy_from_slice(&bytes);
    uuid::Uuid::from_bytes(uuid_bytes)
}

/// Trusted proxy IPs — only trust X-Forwarded-For from these sources.
/// Caddy/reverse proxy on localhost is the typical trusted proxy.
fn trusted_proxy_ips() -> Vec<String> {
    std::env::var("TRUSTED_PROXY_IPS")
        .unwrap_or_else(|_| "127.0.0.1,::1".to_string())
        .split(',')
        .map(|s| s.trim().to_string())
        .collect()
}

/// Extract client IP from request headers (X-Forwarded-For, X-Real-IP) or connection info.
/// Only trusts X-Forwarded-For/X-Real-IP if the request comes from a trusted proxy.
fn extract_client_ip(request: &Request) -> String {
    let trusted = trusted_proxy_ips();

    // Get the direct connection IP from ConnectInfo if available
    let connect_ip = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip().to_string());

    // Only trust forwarded headers if request comes from a trusted proxy
    let from_trusted_proxy = connect_ip
        .as_ref()
        .map(|ip| trusted.iter().any(|t| t == ip))
        .unwrap_or(false);

    if from_trusted_proxy {
        // Check X-Forwarded-For (first IP in chain is the client)
        if let Some(xff) = request.headers().get("x-forwarded-for") {
            if let Ok(val) = xff.to_str() {
                if let Some(first_ip) = val.split(',').next() {
                    let ip = first_ip.trim();
                    if !ip.is_empty() {
                        return ip.to_string();
                    }
                }
            }
        }
        // Check X-Real-IP
        if let Some(xri) = request.headers().get("x-real-ip") {
            if let Ok(val) = xri.to_str() {
                let ip = val.trim();
                if !ip.is_empty() {
                    return ip.to_string();
                }
            }
        }
    }

    // Use direct connection IP or fallback
    connect_ip.unwrap_or_else(|| "unknown".to_string())
}

/// Rate limiting middleware.
///
/// Must run AFTER auth middleware (needs AuthContext in request extensions).
/// For unauthenticated auth endpoints (login, register), rate limits by client IP.
/// Adds the following response headers:
/// - `X-RateLimit-Limit`: max requests per window
/// - `X-RateLimit-Remaining`: remaining requests
/// - `X-RateLimit-Category`: endpoint category
/// - `Retry-After`: seconds until next allowed request (only on 429)
pub async fn rate_limit_middleware(request: Request, next: Next) -> Result<Response, StatusCode> {
    let path = request.uri().path().to_string();
    let category = EndpointCategory::from_path(&path);

    // Extract auth context (set by auth middleware)
    let auth_ctx = request.extensions().get::<AuthContext>().cloned();

    // Get rate limiter from request extensions (set by app state layer)
    let limiter = request.extensions().get::<RateLimiter>().cloned();
    let Some(limiter) = limiter else {
        // No rate limiter configured — pass through
        return Ok(next.run(request).await);
    };

    // R4-C11: For auth endpoints (login, register, TOTP), rate limit by IP even when
    // unauthenticated. This prevents brute-force attacks on login/TOTP endpoints.
    let (bucket_id, tier) = if let Some(ref ctx) = auth_ctx {
        (ctx.user_id, to_rate_limit_tier(&ctx.tier))
    } else if category == EndpointCategory::Auth {
        // Unauthenticated auth request — rate limit by IP at Free tier (strictest)
        let client_ip = extract_client_ip(&request);
        (ip_to_uuid(&client_ip), RateLimitTier::Free)
    } else {
        // Unauthenticated non-auth request — skip rate limiting (auth middleware rejects)
        return Ok(next.run(request).await);
    };

    let result = limiter.check(bucket_id, category, tier);

    // Record rate limit metrics
    let cat_str = category_str(category);
    let tier_str = match tier {
        RateLimitTier::Free => "free",
        RateLimitTier::Pro => "pro",
        RateLimitTier::Enterprise => "enterprise",
    };
    crate::metrics::record_rate_limit(cat_str, tier_str, result.allowed);

    if !result.allowed {
        let retry_after = result.retry_after.unwrap_or(60);
        let mut response = (
            StatusCode::TOO_MANY_REQUESTS,
            format!("Rate limit exceeded. Retry after {} seconds.", retry_after),
        )
            .into_response();

        let headers = response.headers_mut();
        set_rate_limit_headers(headers, &result, category);
        headers.insert(
            HeaderName::from_static("retry-after"),
            HeaderValue::from_str(&retry_after.to_string())
                .unwrap_or(HeaderValue::from_static("60")),
        );

        return Ok(response);
    }

    // Request allowed — run handler and add rate limit headers to response
    let mut response = next.run(request).await;
    set_rate_limit_headers(response.headers_mut(), &result, category);

    Ok(response)
}

/// Get string representation of an endpoint category.
fn category_str(category: EndpointCategory) -> &'static str {
    match category {
        EndpointCategory::Chat => "chat",
        EndpointCategory::ToolResult => "tool_result",
        EndpointCategory::Plugin => "plugin",
        EndpointCategory::Debug => "debug",
        EndpointCategory::Auth => "auth",
        EndpointCategory::Admin => "admin",
        EndpointCategory::Other => "other",
    }
}

/// Set X-RateLimit-* headers on a response.
fn set_rate_limit_headers(
    headers: &mut axum::http::HeaderMap,
    result: &gateway_core::rte::RateLimitResult,
    category: EndpointCategory,
) {
    if let Ok(v) = HeaderValue::from_str(&result.limit.to_string()) {
        headers.insert(HeaderName::from_static("x-ratelimit-limit"), v);
    }
    if let Ok(v) = HeaderValue::from_str(&result.remaining.to_string()) {
        headers.insert(HeaderName::from_static("x-ratelimit-remaining"), v);
    }
    headers.insert(
        HeaderName::from_static("x-ratelimit-category"),
        HeaderValue::from_static(category_str(category)),
    );
}
