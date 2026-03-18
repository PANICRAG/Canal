//! Runtime registry — per-user connector subscriptions and bundle activations.
//!
//! Manages the runtime state of which connectors are installed and which
//! bundles are active for each user. Persisted to JSON files.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// Per-user bundle activation entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleActivation {
    /// Bundle version.
    pub version: String,

    /// When the bundle was activated.
    pub activated_at: String,

    /// Priority for ordering (lower = higher priority).
    #[serde(default)]
    pub priority: u32,
}

/// Per-user activation state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserActivations {
    /// List of active bundle names (in priority order).
    pub active: Vec<String>,

    /// Bundle activation details.
    pub bundles: HashMap<String, BundleActivation>,
}

/// Runtime registry for per-user connector subscriptions and bundle activations.
pub struct RuntimeRegistry {
    /// User ID → active bundles.
    activations: HashMap<String, UserActivations>,

    /// Path to the activations persistence file.
    activations_path: PathBuf,

    /// Write lock for atomic file operations.
    write_lock: Mutex<()>,
}

impl RuntimeRegistry {
    /// Create a new runtime registry with a persistence path.
    pub fn new(activations_path: PathBuf) -> Self {
        Self {
            activations: HashMap::new(),
            activations_path,
            write_lock: Mutex::new(()),
        }
    }

    /// Load activations from the persistence file.
    pub fn load(activations_path: PathBuf) -> Result<Self, RuntimeError> {
        let activations = if activations_path.exists() {
            let content = std::fs::read_to_string(&activations_path)
                .map_err(|e| RuntimeError::Io(format!("read activations: {}", e)))?;
            serde_json::from_str(&content)
                .map_err(|e| RuntimeError::Parse(format!("parse activations: {}", e)))?
        } else {
            HashMap::new()
        };

        Ok(Self {
            activations,
            activations_path,
            write_lock: Mutex::new(()),
        })
    }

    /// Activate a bundle for a user.
    pub async fn activate_bundle(
        &mut self,
        user_id: &str,
        bundle_name: &str,
        version: &str,
    ) -> Result<(), RuntimeError> {
        let user = self.activations.entry(user_id.to_string()).or_default();

        if user.active.contains(&bundle_name.to_string()) {
            return Err(RuntimeError::AlreadyActive(bundle_name.to_string()));
        }

        user.active.push(bundle_name.to_string());
        user.bundles.insert(
            bundle_name.to_string(),
            BundleActivation {
                version: version.to_string(),
                activated_at: chrono::Utc::now().to_rfc3339(),
                priority: user.active.len() as u32 - 1,
            },
        );

        self.save().await
    }

    /// Deactivate a bundle for a user.
    pub async fn deactivate_bundle(
        &mut self,
        user_id: &str,
        bundle_name: &str,
    ) -> Result<(), RuntimeError> {
        let user = self.activations.entry(user_id.to_string()).or_default();

        if !user.active.contains(&bundle_name.to_string()) {
            return Err(RuntimeError::NotActive(bundle_name.to_string()));
        }

        user.active.retain(|n| n != bundle_name);
        user.bundles.remove(bundle_name);

        self.save().await
    }

    /// Get active bundle names for a user.
    pub fn get_active_bundles(&self, user_id: &str) -> Vec<String> {
        self.activations
            .get(user_id)
            .map(|u| u.active.clone())
            .unwrap_or_default()
    }

    /// Get active bundle activations for a user.
    pub fn get_active_bundle_details(&self, user_id: &str) -> Vec<(String, BundleActivation)> {
        self.activations
            .get(user_id)
            .map(|u| {
                u.active
                    .iter()
                    .filter_map(|name| u.bundles.get(name).map(|a| (name.clone(), a.clone())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns all active bundles across all users.
    ///
    /// Each entry is `(user_id, Vec<bundle_name>)`.
    /// Used for startup reconnection of MCP servers.
    pub fn all_active_bundles(&self) -> Vec<(String, Vec<String>)> {
        self.activations
            .iter()
            .map(|(uid, ua)| (uid.clone(), ua.active.clone()))
            .collect()
    }

    /// Check if a bundle is active for a user.
    pub fn is_active(&self, user_id: &str, bundle_name: &str) -> bool {
        self.activations
            .get(user_id)
            .map(|u| u.active.contains(&bundle_name.to_string()))
            .unwrap_or(false)
    }

    /// Save activations to disk atomically.
    async fn save(&self) -> Result<(), RuntimeError> {
        let _guard = self.write_lock.lock().await;

        let content = serde_json::to_string_pretty(&self.activations)
            .map_err(|e| RuntimeError::Serialization(e.to_string()))?;

        if let Some(parent) = self.activations_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| RuntimeError::Io(format!("create dir: {}", e)))?;
        }

        let tmp_path = self.activations_path.with_extension("tmp");
        std::fs::write(&tmp_path, &content)
            .map_err(|e| RuntimeError::Io(format!("write tmp: {}", e)))?;
        std::fs::rename(&tmp_path, &self.activations_path)
            .map_err(|e| RuntimeError::Io(format!("rename: {}", e)))?;

        Ok(())
    }
}

/// Errors from runtime registry operations.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// Filesystem I/O error.
    #[error("runtime io error: {0}")]
    Io(String),

    /// Parse error.
    #[error("runtime parse error: {0}")]
    Parse(String),

    /// Serialization error.
    #[error("runtime serialization error: {0}")]
    Serialization(String),

    /// Bundle already active.
    #[error("bundle already active: {0}")]
    AlreadyActive(String),

    /// Bundle not active.
    #[error("bundle not active: {0}")]
    NotActive(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_activate_bundle() {
        let tmp = TempDir::new().unwrap();
        let mut registry = RuntimeRegistry::new(tmp.path().join("activations.json"));

        registry
            .activate_bundle("user1", "code-assistance", "1.0.0")
            .await
            .unwrap();

        assert!(registry.is_active("user1", "code-assistance"));
        let active = registry.get_active_bundles("user1");
        assert_eq!(active, vec!["code-assistance"]);
    }

    #[tokio::test]
    async fn test_deactivate_bundle() {
        let tmp = TempDir::new().unwrap();
        let mut registry = RuntimeRegistry::new(tmp.path().join("activations.json"));

        registry
            .activate_bundle("user1", "code-assistance", "1.0.0")
            .await
            .unwrap();
        registry
            .deactivate_bundle("user1", "code-assistance")
            .await
            .unwrap();

        assert!(!registry.is_active("user1", "code-assistance"));
        assert!(registry.get_active_bundles("user1").is_empty());
    }

    #[tokio::test]
    async fn test_activate_duplicate() {
        let tmp = TempDir::new().unwrap();
        let mut registry = RuntimeRegistry::new(tmp.path().join("activations.json"));

        registry
            .activate_bundle("user1", "code-assistance", "1.0.0")
            .await
            .unwrap();
        let result = registry
            .activate_bundle("user1", "code-assistance", "1.0.0")
            .await;
        assert!(matches!(result, Err(RuntimeError::AlreadyActive(_))));
    }

    #[tokio::test]
    async fn test_deactivate_not_active() {
        let tmp = TempDir::new().unwrap();
        let mut registry = RuntimeRegistry::new(tmp.path().join("activations.json"));

        let result = registry.deactivate_bundle("user1", "nonexistent").await;
        assert!(matches!(result, Err(RuntimeError::NotActive(_))));
    }

    #[tokio::test]
    async fn test_multiple_bundles() {
        let tmp = TempDir::new().unwrap();
        let mut registry = RuntimeRegistry::new(tmp.path().join("activations.json"));

        registry
            .activate_bundle("user1", "code-assistance", "1.0.0")
            .await
            .unwrap();
        registry
            .activate_bundle("user1", "data-science", "1.0.0")
            .await
            .unwrap();

        let active = registry.get_active_bundles("user1");
        assert_eq!(active.len(), 2);
        assert!(active.contains(&"code-assistance".to_string()));
        assert!(active.contains(&"data-science".to_string()));
    }

    #[tokio::test]
    async fn test_user_isolation() {
        let tmp = TempDir::new().unwrap();
        let mut registry = RuntimeRegistry::new(tmp.path().join("activations.json"));

        registry
            .activate_bundle("user1", "code-assistance", "1.0.0")
            .await
            .unwrap();
        registry
            .activate_bundle("user2", "data-science", "1.0.0")
            .await
            .unwrap();

        assert!(registry.is_active("user1", "code-assistance"));
        assert!(!registry.is_active("user1", "data-science"));
        assert!(!registry.is_active("user2", "code-assistance"));
        assert!(registry.is_active("user2", "data-science"));
    }

    #[tokio::test]
    async fn test_persistence_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("activations.json");

        // Write
        {
            let mut registry = RuntimeRegistry::new(path.clone());
            registry
                .activate_bundle("user1", "code-assistance", "1.0.0")
                .await
                .unwrap();
            registry
                .activate_bundle("user1", "data-science", "2.0.0")
                .await
                .unwrap();
        }

        // Read back
        let registry = RuntimeRegistry::load(path).unwrap();
        assert!(registry.is_active("user1", "code-assistance"));
        assert!(registry.is_active("user1", "data-science"));

        let details = registry.get_active_bundle_details("user1");
        assert_eq!(details.len(), 2);
    }

    #[tokio::test]
    async fn test_get_active_bundle_details() {
        let tmp = TempDir::new().unwrap();
        let mut registry = RuntimeRegistry::new(tmp.path().join("activations.json"));

        registry
            .activate_bundle("user1", "code-assistance", "1.0.0")
            .await
            .unwrap();

        let details = registry.get_active_bundle_details("user1");
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].0, "code-assistance");
        assert_eq!(details[0].1.version, "1.0.0");
    }

    #[test]
    fn test_empty_user() {
        let tmp = TempDir::new().unwrap();
        let registry = RuntimeRegistry::new(tmp.path().join("activations.json"));

        assert!(registry.get_active_bundles("nobody").is_empty());
        assert!(!registry.is_active("nobody", "anything"));
    }
}
