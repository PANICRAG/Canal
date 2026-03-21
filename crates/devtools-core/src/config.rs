//! DevTools configuration

use serde::{Deserialize, Serialize};

/// Configuration for the DevTools service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevtoolsConfig {
    /// HTTP server port (devtools-server only).
    #[serde(default = "default_port")]
    pub port: u16,

    /// Maximum number of traces to keep in memory before LRU eviction.
    #[serde(default = "default_max_traces")]
    pub max_traces: usize,

    /// Maximum observations per trace.
    #[serde(default = "default_max_observations_per_trace")]
    pub max_observations_per_trace: usize,

    /// Enable LRU eviction of old traces.
    #[serde(default = "default_lru_eviction")]
    pub lru_eviction: bool,

    /// CORS allowed origins.
    #[serde(default = "default_cors_origins")]
    pub cors_allowed_origins: Vec<String>,

    /// Enable authentication.
    #[serde(default = "default_auth_enabled")]
    pub auth_enabled: bool,

    /// Maximum age of traces in hours before cleanup (0 = no limit).
    #[serde(default)]
    pub retention_max_age_hours: u64,
}

impl Default for DevtoolsConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            max_traces: default_max_traces(),
            max_observations_per_trace: default_max_observations_per_trace(),
            lru_eviction: default_lru_eviction(),
            cors_allowed_origins: default_cors_origins(),
            auth_enabled: default_auth_enabled(),
            retention_max_age_hours: 0,
        }
    }
}

fn default_port() -> u16 {
    4200
}

fn default_max_traces() -> usize {
    10000
}

fn default_max_observations_per_trace() -> usize {
    5000
}

fn default_lru_eviction() -> bool {
    true
}

fn default_cors_origins() -> Vec<String> {
    vec!["http://localhost:5173".into(), "tauri://localhost".into()]
}

fn default_auth_enabled() -> bool {
    true
}

// ============================================================================
// Langfuse export configuration (feature-gated)
// ============================================================================

/// Configuration for Langfuse trace export.
///
/// Reads from environment variables. Returns `None` from `from_env()` if
/// required keys are not set — making Langfuse export purely opt-in.
#[cfg(feature = "langfuse")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LangfuseConfig {
    /// Langfuse API host.
    #[serde(default = "default_langfuse_host")]
    pub host: String,
    /// Langfuse public key (used as Basic auth username).
    pub public_key: String,
    /// Langfuse secret key (used as Basic auth password).
    pub secret_key: String,
    /// Flush interval in milliseconds.
    #[serde(default = "default_flush_interval_ms")]
    pub flush_interval_ms: u64,
    /// Maximum batch size before auto-flush.
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

#[cfg(feature = "langfuse")]
impl LangfuseConfig {
    /// Load configuration from environment variables.
    ///
    /// Returns `None` if `LANGFUSE_PUBLIC_KEY` or `LANGFUSE_SECRET_KEY` are not set.
    pub fn from_env() -> Option<Self> {
        let public_key = std::env::var("LANGFUSE_PUBLIC_KEY").ok()?;
        let secret_key = std::env::var("LANGFUSE_SECRET_KEY").ok()?;
        Some(Self {
            host: std::env::var("LANGFUSE_HOST").unwrap_or_else(|_| default_langfuse_host()),
            public_key,
            secret_key,
            flush_interval_ms: std::env::var("LANGFUSE_FLUSH_INTERVAL_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default_flush_interval_ms()),
            batch_size: std::env::var("LANGFUSE_BATCH_SIZE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default_batch_size()),
        })
    }
}

#[cfg(feature = "langfuse")]
fn default_langfuse_host() -> String {
    "https://cloud.langfuse.com".to_string()
}

#[cfg(feature = "langfuse")]
fn default_flush_interval_ms() -> u64 {
    500
}

#[cfg(feature = "langfuse")]
fn default_batch_size() -> usize {
    50
}

#[cfg(all(feature = "langfuse", test))]
mod langfuse_config_tests {
    use super::*;

    // Note: env var tests are combined into a single test to avoid
    // parallel test interference (env vars are process-global).

    #[test]
    fn test_from_env_lifecycle() {
        // Phase 1: No keys → None
        std::env::remove_var("LANGFUSE_PUBLIC_KEY");
        std::env::remove_var("LANGFUSE_SECRET_KEY");
        std::env::remove_var("LANGFUSE_HOST");
        assert!(LangfuseConfig::from_env().is_none());

        // Phase 2: With keys → Some with defaults
        std::env::set_var("LANGFUSE_PUBLIC_KEY", "pk-test");
        std::env::set_var("LANGFUSE_SECRET_KEY", "sk-test");
        let config = LangfuseConfig::from_env().unwrap();
        assert_eq!(config.public_key, "pk-test");
        assert_eq!(config.secret_key, "sk-test");
        assert_eq!(config.host, "https://cloud.langfuse.com");
        assert_eq!(config.flush_interval_ms, 500);
        assert_eq!(config.batch_size, 50);

        // Phase 3: Custom host
        std::env::set_var("LANGFUSE_HOST", "https://my-langfuse.example.com");
        let config = LangfuseConfig::from_env().unwrap();
        assert_eq!(config.host, "https://my-langfuse.example.com");

        // Cleanup
        std::env::remove_var("LANGFUSE_PUBLIC_KEY");
        std::env::remove_var("LANGFUSE_SECRET_KEY");
        std::env::remove_var("LANGFUSE_HOST");
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = LangfuseConfig {
            host: "https://cloud.langfuse.com".into(),
            public_key: "pk-test".into(),
            secret_key: "sk-test".into(),
            flush_interval_ms: 500,
            batch_size: 50,
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: LangfuseConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.host, config.host);
        assert_eq!(deserialized.public_key, config.public_key);
    }

    #[test]
    fn test_devtools_config_with_langfuse_field() {
        // Verify LangfuseConfig serializes independently
        let config = LangfuseConfig {
            host: "https://cloud.langfuse.com".into(),
            public_key: "pk-123".into(),
            secret_key: "sk-456".into(),
            flush_interval_ms: 1000,
            batch_size: 25,
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["flush_interval_ms"], 1000);
        assert_eq!(json["batch_size"], 25);
    }
}
