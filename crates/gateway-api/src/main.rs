//! AI Gateway HTTP API Server

use axum::Router;
use std::net::SocketAddr;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod crypto;
mod error;
mod execution;
mod metrics;
mod middleware;
mod remote_agent;
mod routes;
mod state;
mod websocket;

use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,gateway_api=debug,gateway_core=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load environment variables
    dotenvy::dotenv().ok();

    tracing::info!("Starting AI Gateway API server");

    // Create application state
    let state = AppState::new().await?;

    // Start background learning scheduler (triggers learn() when experience buffer reaches threshold)
    #[cfg(feature = "learning")]
    state.spawn_learning_scheduler();

    // Start WebSocket cleanup task
    websocket::spawn_cleanup_task(state.ws_manager.clone());

    // Start RTE pending execution eviction task (A28)
    {
        let rte_pending = state.rte_pending.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                let evicted = rte_pending.evict_expired();
                if evicted > 0 {
                    tracing::info!(evicted, "RTE: evicted expired pending tool executions");
                }
            }
        });
    }

    // Initialize Supabase auth if configured (A28)
    let supabase_auth = middleware::supabase::SupabaseAuth::from_env();
    if let Some(ref auth) = supabase_auth {
        // Pre-fetch JWKS keys at startup
        let auth_clone = auth.clone();
        if let Err(e) = auth_clone.refresh_keys().await {
            tracing::warn!(error = %e, "Initial JWKS fetch failed — will retry on first request");
        }
        // Background JWKS refresh (every 1 hour)
        let auth_bg = auth.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.tick().await; // skip immediate tick (already fetched above)
            loop {
                interval.tick().await;
                if let Err(e) = auth_bg.refresh_keys().await {
                    tracing::warn!(error = %e, "Periodic JWKS refresh failed");
                }
            }
        });
    }

    // Start rate limiter eviction task (A28)
    {
        let rate_limiter = state.rate_limiter.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                let evicted = rate_limiter.evict_idle(std::time::Duration::from_secs(600));
                if evicted > 0 {
                    tracing::info!(
                        evicted,
                        buckets_active = rate_limiter.bucket_count(),
                        "Rate limiter: evicted idle buckets"
                    );
                }
            }
        });
    }

    // Build router
    // Layer order (outermost applied first, innermost closest to handler):
    //   body_limit → cors → trace → auth → rate_limit → security_headers → handler
    let rate_limiter_ext = state.rate_limiter.as_ref().clone();
    let mut app = Router::new()
        .nest("/api", routes::api_routes())
        .nest("/ws", websocket::routes())
        .route("/metrics", axum::routing::get(metrics_handler))
        .layer(axum::middleware::from_fn(security_headers_middleware))
        .layer(axum::middleware::from_fn(
            middleware::logging::logging_middleware,
        ))
        .layer(axum::middleware::from_fn(
            middleware::audit::audit_middleware,
        ))
        .layer(axum::Extension(state.audit_store.clone()))
        .layer(axum::middleware::from_fn(middleware::rbac::rbac_middleware))
        .layer(axum::middleware::from_fn(
            middleware::rate_limit::rate_limit_middleware,
        ))
        .layer(axum::Extension(rate_limiter_ext))
        .layer(axum::middleware::from_fn(middleware::auth::auth_middleware));

    // Inject RS256 KeyPair so auth middleware can verify RS256 tokens
    let key_pair = canal_auth::load_key_pair();
    app = app.layer(axum::Extension(key_pair));

    // Inject JWKS cache for cross-service token verification (e.g., platform-service tokens)
    if let Some(jwks_cache) = canal_auth::JwksCache::from_env() {
        let cache_clone = jwks_cache.clone();
        // Pre-fetch JWKS keys at startup (non-fatal if platform-service isn't up yet)
        tokio::spawn(async move {
            match cache_clone.refresh().await {
                Ok(n) => tracing::info!(keys = n, "Initial JWKS fetch OK"),
                Err(e) => {
                    tracing::warn!(error = %e, "Initial JWKS fetch failed — will retry on first request")
                }
            }
        });
        // Background refresh every 5 minutes
        let cache_bg = jwks_cache.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            interval.tick().await; // skip first (already fetched above)
            loop {
                interval.tick().await;
                if let Err(e) = cache_bg.refresh().await {
                    tracing::warn!(error = %e, "Periodic JWKS refresh failed");
                }
            }
        });
        app = app.layer(axum::Extension(jwks_cache));
    }

    // Inject Supabase auth into request extensions (must be outer to auth middleware)
    if let Some(sa) = supabase_auth {
        app = app.layer(axum::Extension(sa));
    }

    let app = app
        .layer(TraceLayer::new_for_http())
        .layer(build_cors_layer())
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            10 * 1024 * 1024, // 10 MB
        ))
        .layer(axum::middleware::from_fn(
            middleware::request_id::request_id_middleware,
        ))
        .with_state(state);

    // Get port from environment or use default
    let port: u16 = std::env::var("SERVER_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4000);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("Server shut down gracefully");
    Ok(())
}

/// Graceful shutdown signal handler.
///
/// Listens for Ctrl+C (all platforms) and SIGTERM (Unix only).
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("Received Ctrl+C, initiating graceful shutdown"),
        _ = terminate => tracing::info!("Received SIGTERM, initiating graceful shutdown"),
    }
}

/// Build CORS layer from environment configuration.
///
/// - In production (`CANAL_ENV=production`): requires `CORS_ALLOWED_ORIGINS` env var
///   (comma-separated list of allowed origins).
/// - In development: defaults to common localhost origins.
fn build_cors_layer() -> CorsLayer {
    use axum::http::{HeaderValue, Method};
    use tower_http::cors::AllowOrigin;

    let is_production = std::env::var("CANAL_ENV")
        .map(|v| v == "production")
        .unwrap_or(false);

    let origins: Vec<HeaderValue> = if let Ok(origins_str) = std::env::var("CORS_ALLOWED_ORIGINS") {
        origins_str
            .split(',')
            .filter_map(|s| {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    match HeaderValue::from_str(trimmed) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            tracing::warn!(origin = trimmed, error = %e, "Invalid CORS origin, skipping");
                            None
                        }
                    }
                }
            })
            .collect()
    } else if is_production {
        tracing::error!("CORS_ALLOWED_ORIGINS not set in production — CORS will reject all cross-origin requests");
        vec![]
    } else {
        // Development defaults
        vec![
            HeaderValue::from_static("http://localhost:5173"),
            HeaderValue::from_static("http://localhost:4000"),
            HeaderValue::from_static("http://localhost:3000"),
            HeaderValue::from_static("tauri://localhost"),
            HeaderValue::from_static("https://tauri.localhost"),
        ]
    };

    tracing::info!(
        origin_count = origins.len(),
        production = is_production,
        "CORS configured"
    );

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
            Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            axum::http::header::ACCEPT,
            axum::http::header::ORIGIN,
            axum::http::header::HeaderName::from_static("x-requested-with"),
        ])
        .allow_credentials(true)
}

/// Build security headers middleware.
///
/// Adds standard security response headers to every response:
/// - X-Content-Type-Options: nosniff
/// - X-Frame-Options: DENY
/// - X-XSS-Protection: 1; mode=block
/// - Referrer-Policy: strict-origin-when-cross-origin
/// - In production: Strict-Transport-Security with max-age=31536000
async fn security_headers_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{header::HeaderName, HeaderValue};

    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        HeaderName::from_static("x-xss-protection"),
        HeaderValue::from_static("1; mode=block"),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );

    // R4-M: Cache production check to avoid per-request env::var syscall
    static IS_PRODUCTION: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        std::env::var("CANAL_ENV")
            .map(|v| v == "production")
            .unwrap_or(false)
    });
    if *IS_PRODUCTION {
        headers.insert(
            HeaderName::from_static("strict-transport-security"),
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }

    response
}

/// Metrics endpoint handler using prometheus-metrics exporter
async fn metrics_handler() -> String {
    use std::sync::OnceLock;
    static METRICS: OnceLock<metrics::MetricsCollector> = OnceLock::new();

    let collector = METRICS.get_or_init(metrics::MetricsCollector::new);
    collector.render()
}
