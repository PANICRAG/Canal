//! API routes

use axum::http::StatusCode;
use axum::{routing::get, Router};

use crate::state::AppState;

/// Middleware that requires a valid authenticated user (any role).
///
/// Returns 401 UNAUTHORIZED if no AuthContext is present in request extensions.
/// Applied to routes that handle dangerous operations (code exec, filesystem, git, etc.).
async fn require_auth(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    let _auth = request
        .extensions()
        .get::<canal_auth::AuthContext>()
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(next.run(request).await)
}

// Platform-concern routes removed — served by platform-service:
// account, auth (routes), billing_v2, console, gift_cards, usage
pub mod admin;
pub mod agent;
pub mod artifacts;
pub mod automation;
#[cfg(feature = "cache")]
pub mod cache;
pub mod chat;
pub mod code;
pub mod connectors;
pub mod containers;
#[cfg(feature = "graph")]
pub mod debug;
#[cfg(feature = "devtools")]
pub mod devtools;
pub mod filesystem;
pub mod git;
#[cfg(feature = "orchestration")]
pub mod graph;
pub mod health;
#[cfg(feature = "jobs")]
pub mod jobs;
#[cfg(feature = "learning")]
pub mod learning;
pub mod mcp;
pub mod memory;
pub mod permissions;
pub mod plugins;
pub mod profiles;
#[cfg(feature = "prompt-constraints")]
pub mod prompts;
pub mod sessions;
pub mod settings;
pub mod sync;
pub mod tasks;
pub mod tools;
pub mod workflows;

/// Create the API router
pub fn api_routes() -> Router<AppState> {
    let router = Router::new()
        // Health check endpoints
        .route("/health", get(health::health_check))
        .route("/health/ready", get(health::readiness_check))
        .route("/health/live", get(health::liveness_check))
        // Auth endpoints moved to platform-service
        // Chat endpoints
        .nest("/chat", chat::routes())
        // Tool endpoints (auth required — R4-C3)
        .nest("/tools", tools::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Code execution endpoints (auth required — R4-C4)
        .nest("/code", code::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Filesystem endpoints (auth required — R4-C5)
        .nest("/filesystem", filesystem::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Workflow endpoints (auth required — R4-H9)
        .nest("/workflows", workflows::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Artifact endpoints (auth required — R4-H6)
        .nest("/artifacts", artifacts::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Memory endpoints (auth required — R4-C8 IDOR fix)
        .nest("/memory", memory::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // MCP Server management endpoints (auth required — R4-C2)
        .nest("/mcp", mcp::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Session management endpoints
        .nest("/sessions", sessions::routes())
        // Git endpoints (auth required — R4-C6)
        .nest("/git", git::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Settings endpoints (auth required — R4-C7)
        .nest("/settings", settings::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Container management endpoints (auth required — R4-H13)
        .nest("/containers", containers::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Agent endpoints (auth required — R4-H1)
        .nest("/agent", agent::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Five-layer automation endpoints (auth required — R4-H3)
        .nest("/automation", automation::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Background task endpoints (auth required — R4-H8)
        .nest("/tasks", tasks::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Permission management endpoints
        .nest("/permissions", permissions::routes())
        // Model routing profile endpoints (auth required — R4-H19)
        .nest("/profiles", profiles::routes()
            .layer(axum::middleware::from_fn(require_auth)))
        // Usage/billing/gift-cards moved to platform-service
        // Browser extension endpoints removed (CV8: replaced by canal-cv)
        // Plugin store endpoints (catalog, install, uninstall) — deprecated, use /connectors
        .nest("/plugins", plugins::routes())
        // Connector endpoints (replaces /plugins with unified naming)
        .nest("/connectors", connectors::connector_routes())
        // Bundle endpoints (plugin bundles = connectors + skills + prompts)
        .nest("/bundles", connectors::bundle_routes())
        // Chat sync endpoints (offline-first pull/push protocol)
        .nest("/sync", sync::routes())
        // Admin dashboard endpoints
        .nest("/admin", admin::routes());
    // R4-H17: Removed unprotected convenience aliases for /keys and /llm/providers.
    // These routes are served under /api/admin/* with require_admin middleware.

    // Graph and collaboration endpoints (auth required — R4-H2)
    #[cfg(feature = "orchestration")]
    let router = router.nest(
        "/graph",
        graph::routes().layer(axum::middleware::from_fn(require_auth)),
    );

    // Cache endpoints (auth required — R4-H5)
    #[cfg(feature = "cache")]
    let router = router.nest(
        "/cache",
        cache::routes().layer(axum::middleware::from_fn(require_auth)),
    );

    // Learning endpoints (auth required — R4-H4)
    #[cfg(feature = "learning")]
    let router = router.nest(
        "/learning",
        learning::routes().layer(axum::middleware::from_fn(require_auth)),
    );

    // Prompt constraints endpoints (requires prompt-constraints feature)
    #[cfg(feature = "prompt-constraints")]
    let router = router.nest("/prompts", prompts::routes());

    // Async job endpoints (auth required — R4-H12)
    #[cfg(feature = "jobs")]
    let router = router.nest(
        "/jobs",
        jobs::routes().layer(axum::middleware::from_fn(require_auth)),
    );

    // Debug dashboard endpoints (requires graph feature + DEV_MODE env)
    #[cfg(feature = "graph")]
    let router = router
        .nest("/debug", debug::debug_routes())
        .route("/debug/health", get(health::readiness_check));

    // Billing moved to platform-service

    // DevTools status endpoint (auth required — R4-H7)
    #[cfg(feature = "devtools")]
    let router = router
        .nest(
            "/devtools",
            devtools::routes().layer(axum::middleware::from_fn(require_auth)),
        )
        .route("/devtools/traces", get(devtools::list_traces))
        .route("/devtools/traces/{id}", get(devtools::get_trace));

    router
}
