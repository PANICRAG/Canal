//! Task Worker Binary
//!
//! This binary runs inside isolated Kubernetes pods and provides
//! gRPC services for code execution, file operations, and AI chat.

use std::net::SocketAddr;
use tokio::signal;
use tracing::info;

mod agent;
mod file_ops;
mod git;
mod shell;

// Only include gRPC when proto is compiled
#[cfg(feature = "grpc")]
mod grpc_server;

#[cfg(feature = "grpc")]
pub use canal_proto::worker as proto;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().unwrap()),
        )
        .json()
        .init();

    info!("Starting Task Worker");

    // Get configuration from environment
    let grpc_port: u16 = std::env::var("GRPC_PORT")
        .unwrap_or_else(|_| "50051".to_string())
        .parse()
        .expect("Invalid GRPC_PORT");

    let workspace_dir = std::env::var("WORKSPACE_DIR").unwrap_or_else(|_| "/workspace".to_string());

    info!(
        grpc_port = grpc_port,
        workspace_dir = %workspace_dir,
        "Configuration loaded"
    );

    // Ensure workspace directory exists
    tokio::fs::create_dir_all(&workspace_dir).await?;

    #[cfg(feature = "grpc")]
    {
        use grpc_server::TaskWorkerService;
        use tonic::transport::Server;

        // Create the gRPC service
        let worker_service = TaskWorkerService::new(workspace_dir);
        let service = proto::task_worker_server::TaskWorkerServer::new(worker_service);

        // Build server address
        let addr: SocketAddr = format!("0.0.0.0:{}", grpc_port).parse()?;

        info!(address = %addr, "Starting gRPC server");

        // Start the server with graceful shutdown
        Server::builder()
            .add_service(service)
            .serve_with_shutdown(addr, shutdown_signal())
            .await?;
    }

    #[cfg(not(feature = "grpc"))]
    {
        info!("gRPC not enabled. Install protoc and rebuild with 'grpc' feature.");
        info!("Running in standalone mode...");

        // Wait for shutdown signal
        shutdown_signal().await;
    }

    info!("Task Worker stopped");

    Ok(())
}

/// Wait for shutdown signal
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received Ctrl+C, shutting down"),
        _ = terminate => info!("Received SIGTERM, shutting down"),
    }
}
