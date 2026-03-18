//! Job system configuration types.

use serde::{Deserialize, Serialize};

/// Configuration for the async job system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobsConfig {
    /// Whether the job system is enabled.
    pub enabled: bool,
    /// Maximum number of concurrently executing jobs.
    pub max_concurrent: usize,
    /// Interval (ms) to poll the job store for queued jobs.
    pub poll_interval_ms: u64,
    /// Maximum execution time per job (seconds).
    pub job_timeout_secs: u64,
    /// Default collaboration mode when not specified by the job.
    pub default_mode: String,
    /// Default model when not specified by the job.
    pub default_model: Option<String>,
    /// Webhook notification configuration.
    pub webhook: WebhookConfig,
    /// Recovery configuration for server restarts.
    pub recovery: RecoveryConfig,
}

/// Webhook notification configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Whether webhook notifications are enabled.
    pub enabled: bool,
    /// Webhook URL to POST notifications to.
    pub url: String,
    /// Events that trigger notifications (e.g. "completed", "failed").
    pub events: Vec<String>,
}

/// Recovery configuration for interrupted jobs on server restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryConfig {
    /// Whether automatic recovery is enabled.
    pub enabled: bool,
    /// Recovery strategy: "requeue" (Running → Queued) or "skip".
    pub strategy: String,
}

impl Default for JobsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_concurrent: 3,
            poll_interval_ms: 1000,
            job_timeout_secs: 1800,
            default_mode: "auto".to_string(),
            default_model: Some("qwen3-max-2026-01-23".to_string()),
            webhook: WebhookConfig::default(),
            recovery: RecoveryConfig::default(),
        }
    }
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            events: vec!["completed".to_string(), "failed".to_string()],
        }
    }
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strategy: "requeue".to_string(),
        }
    }
}
