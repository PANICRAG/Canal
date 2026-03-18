//! Health check endpoints

use axum::{extract::State, http::StatusCode, Json};
use serde_json::{json, Value};

use crate::state::AppState;

/// Basic health check — includes database connectivity and storage backend info.
pub async fn health_check(State(state): State<AppState>) -> Json<Value> {
    let db_ok = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();

    let storage_backend = if db_ok { "postgresql" } else { "unknown" };

    Json(json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "service": "ai-gateway",
        "database_connected": db_ok,
        "storage_backend": storage_backend
    }))
}

/// Readiness check - verifies all dependencies
pub async fn readiness_check(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    // Check database connection
    let db_check = sqlx::query("SELECT 1")
        .execute(&state.db)
        .await
        .map(|_| "ok")
        .unwrap_or("error");

    // Check LLM router
    let llm_router = state.llm_router.read().await;
    let llm_providers = llm_router.list_providers();
    let llm_check = if llm_providers.is_empty() {
        "no_providers"
    } else {
        "ok"
    };

    // Check MCP gateway
    let mcp_servers = state.mcp_gateway.list_servers().await;
    let mcp_check = if mcp_servers.is_empty() {
        "no_servers"
    } else {
        "ok"
    };

    // Determine overall status
    let is_ready = db_check == "ok" && llm_check == "ok";

    if is_ready {
        Ok(Json(json!({
            "status": "ready",
            "checks": {
                "database": db_check,
                "llm_router": llm_check,
                "llm_providers": llm_providers,
                "mcp_gateway": mcp_check,
                "mcp_server_count": mcp_servers.len()
            }
        })))
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

/// Liveness check - basic health
pub async fn liveness_check() -> Json<Value> {
    Json(json!({
        "status": "alive"
    }))
}
