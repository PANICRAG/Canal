//! Health check types for module status reporting.

use serde::Serialize;

/// Health status of a single module.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleHealth {
    pub name: String,
    pub status: HealthStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// Health status enum.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Aggregated health status across all modules.
#[derive(Debug, Serialize)]
pub struct GlobalHealth {
    pub status: HealthStatus,
    pub modules: Vec<ModuleHealth>,
}

impl GlobalHealth {
    /// Aggregate module health into a global status.
    pub fn from_modules(modules: Vec<ModuleHealth>) -> Self {
        let status = if modules.is_empty() {
            HealthStatus::Healthy
        } else if modules.iter().any(|m| m.status == HealthStatus::Unhealthy) {
            HealthStatus::Unhealthy
        } else if modules.iter().any(|m| m.status == HealthStatus::Degraded) {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };
        Self { status, modules }
    }
}
