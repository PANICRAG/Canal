//! Shared application state for devtools-server.

use devtools_core::config::DevtoolsConfig;
use devtools_core::DevtoolsService;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::handlers::alerts::AlertState;

/// Shared state passed to all handlers.
pub struct AppState {
    pub devtools: Arc<DevtoolsService>,
    pub config: DevtoolsConfig,
    /// Prometheus base URL for metrics queries.
    pub prometheus_url: String,
    /// Shared HTTP client for proxying requests.
    pub http_client: reqwest::Client,
    /// Loki base URL for log queries.
    pub loki_url: String,
    /// DT8: Alert engine state (rules, active alerts, history, channels).
    pub alert_state: Arc<RwLock<AlertState>>,
    /// DT6: Optional PostgreSQL connection pool for database observability.
    /// None when DATABASE_URL is not configured — handlers fall back to mock data.
    #[cfg(feature = "postgres")]
    pub db_pool: Option<sqlx::PgPool>,
    #[cfg(not(feature = "postgres"))]
    pub db_pool: Option<()>,
}
