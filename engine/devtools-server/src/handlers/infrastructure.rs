//! Infrastructure metrics endpoints — Prometheus proxy, Docker containers, health, storage.

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info};

use crate::error::{ApiError, ApiErrorDetail};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Build a full Prometheus API URL from the base URL and path.
fn prom_url(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

/// Helper to create an internal ApiError from a message.
fn internal_error(msg: impl Into<String>) -> ApiError {
    ApiError {
        error: ApiErrorDetail {
            code: "internal".into(),
            message: msg.into(),
        },
    }
}

/// Helper to create a bad-request ApiError.
fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError {
        error: ApiErrorDetail {
            code: "invalid_input".into(),
            message: msg.into(),
        },
    }
}

// ---------------------------------------------------------------------------
// GET /v1/metrics/query — PromQL proxy
// ---------------------------------------------------------------------------

/// Query parameters for PromQL proxy.
#[derive(Debug, Deserialize)]
pub struct PromQLParams {
    /// PromQL expression.
    pub query: String,
    /// Range start (ISO8601). If provided with `end`, uses query_range.
    pub start: Option<String>,
    /// Range end (ISO8601).
    pub end: Option<String>,
    /// Step duration (e.g. "15s").
    pub step: Option<String>,
}

/// Dangerous patterns that must not appear in PromQL queries.
const BLOCKED_PATTERNS: &[&str] = &["admin", "/api/v1/admin", "config"];

/// GET /v1/metrics/query — proxy a PromQL query to Prometheus.
pub async fn query_prometheus(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PromQLParams>,
) -> Result<impl IntoResponse, ApiError> {
    info!(query = %params.query, "PromQL query requested");

    // Validate query — reject dangerous patterns
    let query_lower = params.query.to_lowercase();
    for pattern in BLOCKED_PATTERNS {
        if query_lower.contains(pattern) {
            return Err(bad_request(format!(
                "Query contains blocked pattern: {pattern}"
            )));
        }
    }

    let (url, query_pairs) = if params.start.is_some() && params.end.is_some() {
        // Range query
        let mut pairs: Vec<(&str, String)> = vec![("query", params.query.clone())];
        if let Some(ref start) = params.start {
            pairs.push(("start", start.clone()));
        }
        if let Some(ref end) = params.end {
            pairs.push(("end", end.clone()));
        }
        if let Some(ref step) = params.step {
            pairs.push(("step", step.clone()));
        } else {
            pairs.push(("step", "15s".to_string()));
        }
        (
            prom_url(&state.prometheus_url, "/api/v1/query_range"),
            pairs,
        )
    } else {
        // Instant query
        let pairs = vec![("query", params.query.clone())];
        (prom_url(&state.prometheus_url, "/api/v1/query"), pairs)
    };

    let resp = state
        .http_client
        .get(&url)
        .query(&query_pairs)
        .send()
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to reach Prometheus");
            internal_error(format!("Prometheus unreachable: {e}"))
        })?;

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        error!(error = %e, "Failed to parse Prometheus response");
        internal_error(format!("Invalid Prometheus response: {e}"))
    })?;

    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// GET /v1/metrics/containers — Docker container list with stats
// ---------------------------------------------------------------------------

/// Container info returned by the containers endpoint.
#[derive(Debug, Serialize)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub status: String,
    pub cpu_percent: f64,
    pub memory_mb: f64,
    pub memory_limit_mb: f64,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub uptime_seconds: i64,
    pub labels: std::collections::HashMap<String, String>,
}

/// GET /v1/metrics/containers — list Docker containers with stats.
pub async fn list_containers(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    info!("Listing Docker containers");

    let docker = state
        .docker
        .as_ref()
        .ok_or_else(|| internal_error("Docker is not available on this host"))?;

    use bollard::container::{ListContainersOptions, StatsOptions};
    use futures::StreamExt;

    let opts = ListContainersOptions::<String> {
        all: true,
        ..Default::default()
    };

    let containers = docker.list_containers(Some(opts)).await.map_err(|e| {
        error!(error = %e, "Failed to list Docker containers");
        internal_error(format!("Docker error: {e}"))
    })?;

    let mut results: Vec<ContainerInfo> = Vec::new();

    for c in containers {
        // Filter to canal-* and app-* (hosted apps) and hosting-* containers by name
        let names = c.names.unwrap_or_default();
        let name = names
            .first()
            .map(|n| n.trim_start_matches('/').to_string())
            .unwrap_or_default();
        if !name.starts_with("canal-")
            && !name.starts_with("app-")
            && !name.starts_with("hosting-")
        {
            continue;
        }

        let id = c.id.clone().unwrap_or_default();
        let status = c.status.clone().unwrap_or_default();
        let labels = c.labels.clone().unwrap_or_default();

        // Calculate uptime from Created timestamp
        let uptime_seconds = c
            .created
            .map(|ts| chrono::Utc::now().timestamp() - ts)
            .unwrap_or(0);

        // Get one-shot stats
        let (cpu_percent, memory_mb, memory_limit_mb, net_rx, net_tx) =
            if c.state.as_deref() == Some("running") {
                let stats_opts = StatsOptions {
                    stream: false,
                    one_shot: true,
                };
                match docker.stats(&id, Some(stats_opts)).next().await {
                    Some(Ok(stats)) => {
                        let cpu = calculate_cpu_percent(&stats);
                        let mem_usage = stats.memory_stats.usage.unwrap_or(0) as f64 / 1_048_576.0;
                        let mem_limit = stats.memory_stats.limit.unwrap_or(0) as f64 / 1_048_576.0;
                        let (rx, tx) = aggregate_network(&stats);
                        (cpu, mem_usage, mem_limit, rx, tx)
                    }
                    _ => (0.0, 0.0, 0.0, 0, 0),
                }
            } else {
                (0.0, 0.0, 0.0, 0, 0)
            };

        results.push(ContainerInfo {
            id,
            name,
            status,
            cpu_percent,
            memory_mb,
            memory_limit_mb,
            network_rx_bytes: net_rx,
            network_tx_bytes: net_tx,
            uptime_seconds,
            labels,
        });
    }

    Ok(Json(results))
}

/// Calculate CPU usage percentage from Docker stats.
fn calculate_cpu_percent(stats: &bollard::container::Stats) -> f64 {
    let cpu_delta = stats.cpu_stats.cpu_usage.total_usage as f64
        - stats.precpu_stats.cpu_usage.total_usage as f64;
    let system_delta = stats.cpu_stats.system_cpu_usage.unwrap_or(0) as f64
        - stats.precpu_stats.system_cpu_usage.unwrap_or(0) as f64;
    let num_cpus = stats.cpu_stats.online_cpus.unwrap_or(1) as f64;

    if system_delta > 0.0 && cpu_delta >= 0.0 {
        (cpu_delta / system_delta) * num_cpus * 100.0
    } else {
        0.0
    }
}

/// Aggregate network rx/tx bytes across all interfaces.
fn aggregate_network(stats: &bollard::container::Stats) -> (u64, u64) {
    let mut rx: u64 = 0;
    let mut tx: u64 = 0;
    if let Some(ref networks) = stats.networks {
        for net in networks.values() {
            rx += net.rx_bytes;
            tx += net.tx_bytes;
        }
    }
    (rx, tx)
}

// ---------------------------------------------------------------------------
// GET /v1/metrics/health — Aggregate health check
// ---------------------------------------------------------------------------

/// Health status for a single component.
#[derive(Debug, Serialize)]
pub struct ComponentHealth {
    pub status: String,
    pub message: Option<String>,
}

/// Aggregate health response.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub overall: String,
    pub sources: std::collections::HashMap<String, ComponentHealth>,
}

/// GET /v1/metrics/health — aggregate infrastructure health check.
pub async fn health_check(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    info!("Infrastructure health check");

    let mut sources = std::collections::HashMap::new();
    let mut all_healthy = true;

    // Check Prometheus
    let prom_health = state
        .http_client
        .get(prom_url(
            &state.prometheus_url,
            "/api/v1/status/runtimeinfo",
        ))
        .send()
        .await;
    match prom_health {
        Ok(resp) if resp.status().is_success() => {
            sources.insert(
                "prometheus".into(),
                ComponentHealth {
                    status: "healthy".into(),
                    message: None,
                },
            );
        }
        Ok(resp) => {
            all_healthy = false;
            sources.insert(
                "prometheus".into(),
                ComponentHealth {
                    status: "unhealthy".into(),
                    message: Some(format!("HTTP {}", resp.status())),
                },
            );
        }
        Err(e) => {
            all_healthy = false;
            sources.insert(
                "prometheus".into(),
                ComponentHealth {
                    status: "unreachable".into(),
                    message: Some(e.to_string()),
                },
            );
        }
    }

    // Check Docker
    match &state.docker {
        Some(docker) => match docker.ping().await {
            Ok(_) => {
                sources.insert(
                    "docker".into(),
                    ComponentHealth {
                        status: "healthy".into(),
                        message: None,
                    },
                );
            }
            Err(e) => {
                all_healthy = false;
                sources.insert(
                    "docker".into(),
                    ComponentHealth {
                        status: "unhealthy".into(),
                        message: Some(e.to_string()),
                    },
                );
            }
        },
        None => {
            all_healthy = false;
            sources.insert(
                "docker".into(),
                ComponentHealth {
                    status: "unavailable".into(),
                    message: Some("Docker client not initialized".into()),
                },
            );
        }
    }

    // Check scrape targets health
    let targets_resp = state
        .http_client
        .get(prom_url(&state.prometheus_url, "/api/v1/targets"))
        .send()
        .await;
    match targets_resp {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                let active = body["data"]["activeTargets"]
                    .as_array()
                    .map(|arr| arr.len())
                    .unwrap_or(0);
                let unhealthy_count = body["data"]["activeTargets"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter(|t| t["health"].as_str() != Some("up"))
                            .count()
                    })
                    .unwrap_or(0);
                if unhealthy_count > 0 {
                    all_healthy = false;
                }
                sources.insert(
                    "scrape_targets".into(),
                    ComponentHealth {
                        status: if unhealthy_count == 0 {
                            "healthy".into()
                        } else {
                            "degraded".into()
                        },
                        message: Some(format!("{active} targets, {unhealthy_count} unhealthy")),
                    },
                );
            }
        }
        _ => {
            // If we can't check targets, it's because Prometheus is down —
            // already captured above.
        }
    }

    let overall_status = if all_healthy { "healthy" } else { "degraded" };

    Ok(Json(HealthResponse {
        overall: overall_status.into(),
        sources,
    }))
}

// ---------------------------------------------------------------------------
// GET /v1/metrics/targets — Prometheus scrape target status
// ---------------------------------------------------------------------------

/// Simplified scrape target info.
#[derive(Debug, Serialize)]
pub struct ScrapeTarget {
    pub job: String,
    pub instance: String,
    pub health: String,
    pub last_scrape: String,
    pub scrape_duration_seconds: f64,
}

/// GET /v1/metrics/targets — simplified Prometheus scrape target status.
pub async fn scrape_targets(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    info!("Fetching scrape targets");

    let resp = state
        .http_client
        .get(prom_url(&state.prometheus_url, "/api/v1/targets"))
        .send()
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to reach Prometheus targets API");
            internal_error(format!("Prometheus unreachable: {e}"))
        })?;

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        error!(error = %e, "Failed to parse Prometheus targets response");
        internal_error(format!("Invalid Prometheus response: {e}"))
    })?;

    let targets: Vec<ScrapeTarget> = body["data"]["activeTargets"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|t| ScrapeTarget {
                    job: t["labels"]["job"].as_str().unwrap_or("").to_string(),
                    instance: t["labels"]["instance"].as_str().unwrap_or("").to_string(),
                    health: t["health"].as_str().unwrap_or("unknown").to_string(),
                    last_scrape: t["lastScrape"].as_str().unwrap_or("").to_string(),
                    scrape_duration_seconds: t["lastScrapeDuration"].as_f64().unwrap_or(0.0),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(targets))
}

// ---------------------------------------------------------------------------
// GET /v1/metrics/storage — MinIO storage summary
// ---------------------------------------------------------------------------

/// Storage summary derived from MinIO Prometheus metrics.
#[derive(Debug, Serialize)]
pub struct StorageSummary {
    pub total_bytes: f64,
    pub total_objects: f64,
    pub buckets: Vec<BucketUsage>,
    pub rates: StorageRates,
}

/// S3 request and bandwidth rates.
#[derive(Debug, Serialize)]
pub struct StorageRates {
    pub requests_per_sec: f64,
    pub errors_per_sec: f64,
    pub bandwidth_in_bytes_per_sec: f64,
    pub bandwidth_out_bytes_per_sec: f64,
}

/// Per-bucket usage info.
#[derive(Debug, Serialize)]
pub struct BucketUsage {
    pub name: String,
    pub size_bytes: f64,
    pub object_count: f64,
}

/// GET /v1/metrics/storage — MinIO storage summary from Prometheus.
pub async fn storage_summary(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    info!("Fetching storage summary from Prometheus");

    // Helper closure to run an instant PromQL query
    let query_prom = |query: &str| {
        let client = state.http_client.clone();
        let url = prom_url(&state.prometheus_url, "/api/v1/query");
        let q = query.to_string();
        async move {
            let resp = client
                .get(&url)
                .query(&[("query", &q)])
                .send()
                .await
                .map_err(|e| internal_error(format!("Prometheus unreachable: {e}")))?;
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| internal_error(format!("Invalid Prometheus response: {e}")))?;
            Ok::<serde_json::Value, ApiError>(body)
        }
    };

    // Run queries concurrently
    let (bucket_bytes, bucket_objects, req_rate, err_rate, ingress, egress) = tokio::try_join!(
        query_prom("minio_bucket_usage_total_bytes"),
        query_prom("minio_bucket_usage_object_total"),
        query_prom("sum(rate(minio_s3_requests_total[5m]))"),
        query_prom("sum(rate(minio_s3_requests_errors_total[5m]))"),
        query_prom("sum(rate(minio_s3_traffic_received_bytes_total[5m]))"),
        query_prom("sum(rate(minio_s3_traffic_sent_bytes_total[5m]))"),
    )
    .map_err(|e| {
        error!("Failed to query MinIO metrics");
        e
    })?;

    // Extract per-bucket usage
    let mut buckets = Vec::new();
    let mut total_bytes: f64 = 0.0;
    let mut total_objects: f64 = 0.0;

    if let Some(results) = bucket_bytes["data"]["result"].as_array() {
        for r in results {
            let bucket_name = r["metric"]["bucket"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            let bytes = r["value"][1]
                .as_str()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0);
            total_bytes += bytes;

            // Find matching object count
            let objects = bucket_objects["data"]["result"]
                .as_array()
                .and_then(|arr| {
                    arr.iter()
                        .find(|o| o["metric"]["bucket"].as_str() == Some(&bucket_name))
                })
                .and_then(|o| o["value"][1].as_str())
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0);
            total_objects += objects;

            buckets.push(BucketUsage {
                name: bucket_name,
                size_bytes: bytes,
                object_count: objects,
            });
        }
    }

    let extract_scalar = |v: &serde_json::Value| -> f64 {
        v["data"]["result"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|r| r["value"][1].as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
    };

    Ok(Json(StorageSummary {
        total_bytes,
        total_objects,
        buckets,
        rates: StorageRates {
            requests_per_sec: extract_scalar(&req_rate),
            errors_per_sec: extract_scalar(&err_rate),
            bandwidth_in_bytes_per_sec: extract_scalar(&ingress),
            bandwidth_out_bytes_per_sec: extract_scalar(&egress),
        },
    }))
}
