//! DevTools Server library — reusable Axum router for LLM observability.
//!
//! Use `build_app(port)` to create a fully configured `Router` that can be
//! served standalone (`main.rs`) or embedded inside another process (e.g. Tauri).

pub mod auth;
pub mod error;
pub mod handlers;
pub mod routes;
pub mod state;

use devtools_core::config::DevtoolsConfig;
use devtools_core::store::memory::{InMemoryEventBus, InMemoryTraceStore};
use devtools_core::DevtoolsService;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::state::AppState;

/// Build a fully configured Axum `Router` for the devtools server.
///
/// Includes CORS, tracing, auth middleware, and all route handlers.
/// The caller is responsible for binding a `TcpListener` and calling `axum::serve`.
pub async fn build_app(port: u16) -> axum::Router {
    let max_traces: usize = std::env::var("DEVTOOLS_MAX_TRACES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10000);

    let config = DevtoolsConfig {
        port,
        max_traces,
        ..Default::default()
    };

    // Build devtools service
    let trace_store = Arc::new(InMemoryTraceStore::new(config.max_traces));
    let event_bus = Arc::new(InMemoryEventBus::new());
    let devtools_service = Arc::new(DevtoolsService::new(trace_store, event_bus));
    info!(
        "DevTools service initialized (max_traces={})",
        config.max_traces
    );

    // Infrastructure metrics clients
    let prometheus_url =
        std::env::var("PROMETHEUS_URL").unwrap_or_else(|_| "http://localhost:9090".to_string());
    let loki_url =
        std::env::var("LOKI_URL").unwrap_or_else(|_| "http://localhost:3100".to_string());
    let http_client = reqwest::Client::new();
    info!(
        prometheus_url = %prometheus_url,
        loki_url = %loki_url,
        "Infrastructure metrics clients initialized"
    );

    // DT8: Alert engine state with 14 default rules
    let alert_state = Arc::new(tokio::sync::RwLock::new(
        handlers::alerts::AlertState::with_defaults(),
    ));
    info!("Alert engine initialized (14 default rules)");

    // DT6: Optional PostgreSQL pool for database observability
    #[cfg(feature = "postgres")]
    let db_pool = if let Ok(url) = std::env::var("DATABASE_URL") {
        match sqlx::PgPool::connect(&url).await {
            Ok(pool) => {
                info!("Connected to PostgreSQL for database observability");
                Some(pool)
            }
            Err(e) => {
                tracing::warn!("Failed to connect to PostgreSQL: {e}. Using mock data.");
                None
            }
        }
    } else {
        info!("DATABASE_URL not set. Database endpoints will return mock data.");
        None
    };
    #[cfg(not(feature = "postgres"))]
    let db_pool = None;

    let state = Arc::new(AppState {
        devtools: devtools_service,
        config,
        prometheus_url,
        http_client,
        loki_url,
        alert_state,
        db_pool,
    });

    routes::build_router(state)
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}
