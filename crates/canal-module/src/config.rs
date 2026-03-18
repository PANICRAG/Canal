//! Configuration loading for canal-server.
//!
//! Supports YAML config file + environment variable overrides.
//! Priority: env var > YAML > defaults.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level canal configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanalConfig {
    /// Listen host (default: "0.0.0.0").
    #[serde(default = "default_host")]
    pub host: String,

    /// Listen port (default: 8080).
    #[serde(default = "default_port")]
    pub port: u16,

    /// Which modules to load.
    #[serde(default)]
    pub modules: ModuleFlags,

    /// PostgreSQL connection URL (optional — only needed by session, billing, admin).
    pub database_url: Option<String>,
}

impl CanalConfig {
    /// Load configuration from YAML file + environment variable overrides.
    pub fn load() -> anyhow::Result<Self> {
        // 1. Load YAML if exists
        let mut config = if Path::new("config/canal.yaml").exists() {
            let content = std::fs::read_to_string("config/canal.yaml")?;
            serde_yaml::from_str(&content)?
        } else {
            Self::default()
        };

        // 2. Override from CANAL_MODULES env var (highest priority)
        if let Ok(val) = std::env::var("CANAL_MODULES") {
            config.modules = ModuleFlags::from_env(&val);
        }

        // 3. Override port from CANAL_PORT or ENGINE_PORT
        if let Ok(val) = std::env::var("CANAL_PORT") {
            if let Ok(port) = val.parse() {
                config.port = port;
            }
        } else if let Ok(val) = std::env::var("ENGINE_PORT") {
            if let Ok(port) = val.parse() {
                config.port = port;
            }
        }

        // 4. Override host from CANAL_HOST
        if let Ok(val) = std::env::var("CANAL_HOST") {
            config.host = val;
        }

        // 5. Override database URL from DATABASE_URL
        if let Ok(val) = std::env::var("DATABASE_URL") {
            config.database_url = Some(val);
        }

        Ok(config)
    }

    /// Get the listen address as "host:port".
    pub fn listen_addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

impl Default for CanalConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            modules: ModuleFlags::default(),
            database_url: None,
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    4000
}

/// Flags for which modules to load at runtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModuleFlags {
    #[serde(default)]
    pub engine: bool,
    #[serde(default)]
    pub session: bool,
    #[serde(default)]
    pub sandbox: bool,
    #[serde(default)]
    pub billing: bool,
    #[serde(default)]
    pub identity: bool,
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub platform: bool,
    /// Lightweight auth for engine-only deployments (mutually exclusive with `identity`).
    #[serde(default)]
    pub auth_shim: bool,
    /// Lightweight usage tracking for engine-only deployments (mutually exclusive with `billing`).
    #[serde(default)]
    pub usage_counter: bool,
}

impl ModuleFlags {
    /// Parse module flags from a comma-separated string, profile name, or "all".
    ///
    /// Supported profiles:
    /// - `"all"` — all platform modules (excludes auth_shim/usage_counter)
    /// - `"platform"` — platform + identity + billing + admin + session
    /// - `"engine-full"` — engine + auth_shim + usage_counter + session + sandbox
    /// - `"engine-lite"` — engine + auth_shim only (minimal standalone)
    pub fn from_env(val: &str) -> Self {
        let trimmed = val.trim();
        match trimmed.to_ascii_lowercase().as_str() {
            "all" => Self::all(),
            "platform" => Self::platform(),
            "engine-full" => Self::engine_full(),
            "engine-lite" => Self::engine_lite(),
            _ => {
                let parts: Vec<&str> = trimmed.split(',').map(str::trim).collect();
                Self {
                    engine: parts.contains(&"engine"),
                    session: parts.contains(&"session"),
                    sandbox: parts.contains(&"sandbox"),
                    billing: parts.contains(&"billing"),
                    identity: parts.contains(&"identity"),
                    admin: parts.contains(&"admin"),
                    platform: parts.contains(&"platform"),
                    auth_shim: parts.contains(&"auth_shim"),
                    usage_counter: parts.contains(&"usage_counter"),
                }
            }
        }
    }

    /// All platform modules enabled (excludes engine-only shims).
    pub fn all() -> Self {
        Self {
            engine: true,
            session: true,
            sandbox: true,
            billing: true,
            identity: true,
            admin: true,
            platform: true,
            auth_shim: false,
            usage_counter: false,
        }
    }

    /// Platform profile — runs the control plane.
    pub fn platform() -> Self {
        Self {
            engine: false,
            session: true,
            sandbox: false,
            billing: true,
            identity: true,
            admin: true,
            platform: true,
            auth_shim: false,
            usage_counter: false,
        }
    }

    /// Engine-full profile — standalone engine with auth + usage + session + sandbox.
    pub fn engine_full() -> Self {
        Self {
            engine: true,
            session: true,
            sandbox: true,
            billing: false,
            identity: false,
            admin: false,
            platform: false,
            auth_shim: true,
            usage_counter: true,
        }
    }

    /// Engine-lite profile — minimal standalone engine.
    pub fn engine_lite() -> Self {
        Self {
            engine: true,
            session: false,
            sandbox: false,
            billing: false,
            identity: false,
            admin: false,
            platform: false,
            auth_shim: true,
            usage_counter: false,
        }
    }

    /// Validate mutual exclusion rules. Returns an error message if violated.
    pub fn validate(&self) -> Result<(), String> {
        if self.identity && self.auth_shim {
            return Err(
                "identity and auth_shim are mutually exclusive — use identity for platform, auth_shim for engine".to_string()
            );
        }
        if self.billing && self.usage_counter {
            return Err(
                "billing and usage_counter are mutually exclusive — use billing for platform, usage_counter for engine".to_string()
            );
        }
        Ok(())
    }

    /// Returns a list of enabled module names.
    pub fn enabled_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.engine {
            names.push("engine");
        }
        if self.session {
            names.push("session");
        }
        if self.sandbox {
            names.push("sandbox");
        }
        if self.billing {
            names.push("billing");
        }
        if self.identity {
            names.push("identity");
        }
        if self.admin {
            names.push("admin");
        }
        if self.platform {
            names.push("platform");
        }
        if self.auth_shim {
            names.push("auth_shim");
        }
        if self.usage_counter {
            names.push("usage_counter");
        }
        names
    }

    /// Returns true if any module that requires a database is enabled.
    pub fn needs_database(&self) -> bool {
        self.session || self.billing || self.admin
    }
}
