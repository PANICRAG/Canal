//! DevTools Server — Standalone HTTP server for LLM observability.
//!
//! Listens on :4200 (configurable via DEVTOOLS_PORT env var).
//! Provides Langfuse-style trace ingestion and query API.

use tracing::info;

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "devtools_server=info,devtools_core=info".into()),
        )
        .init();

    let port: u16 = std::env::var("DEVTOOLS_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4200);

    let app = devtools_server::build_app(port).await;

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .expect("Failed to bind");

    info!("DevTools server listening on :{}", port);
    axum::serve(listener, app).await.expect("Server error");
}
