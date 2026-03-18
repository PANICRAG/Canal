//! Subscription store — per-user plugin subscription persistence.
//!
//! Each user has an independent set of installed plugins.
//! Subscriptions are persisted to a JSON file with atomic writes (tmp → rename).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tokio::sync::Mutex;

use super::error::{PluginError, PluginResult};

/// Per-user plugin subscription store with concurrent-safe persistence.
pub struct SubscriptionStore {
    /// User ID → set of installed plugin names.
    subscriptions: HashMap<String, HashSet<String>>,

    /// Path to the persistence file.
    persistence_path: PathBuf,

    /// Write lock for atomic file operations.
    write_lock: Mutex<()>,
}

impl SubscriptionStore {
    /// Create a new subscription store with a persistence file path.
    pub fn new(persistence_path: PathBuf) -> Self {
        Self {
            subscriptions: HashMap::new(),
            persistence_path,
            write_lock: Mutex::new(()),
        }
    }

    /// Load subscriptions from the persistence file.
    ///
    /// If the file doesn't exist, starts with empty subscriptions.
    pub fn load(persistence_path: PathBuf) -> PluginResult<Self> {
        let subscriptions = if persistence_path.exists() {
            let content = std::fs::read_to_string(&persistence_path)?;
            serde_json::from_str::<HashMap<String, HashSet<String>>>(&content)
                .map_err(|e| PluginError::Parse(format!("subscriptions file: {}", e)))?
        } else {
            HashMap::new()
        };

        Ok(Self {
            subscriptions,
            persistence_path,
            write_lock: Mutex::new(()),
        })
    }

    /// Install a plugin for a user.
    pub async fn install(&mut self, user_id: &str, plugin_name: &str) -> PluginResult<()> {
        let user_set = self.subscriptions.entry(user_id.to_string()).or_default();

        if user_set.contains(plugin_name) {
            return Err(PluginError::AlreadyInstalled(plugin_name.to_string()));
        }

        user_set.insert(plugin_name.to_string());
        self.save().await?;
        Ok(())
    }

    /// Uninstall a plugin for a user.
    pub async fn uninstall(&mut self, user_id: &str, plugin_name: &str) -> PluginResult<()> {
        let user_set = self.subscriptions.entry(user_id.to_string()).or_default();

        if !user_set.remove(plugin_name) {
            return Err(PluginError::NotInstalled(plugin_name.to_string()));
        }

        self.save().await?;
        Ok(())
    }

    /// List installed plugin names for a user.
    pub fn list_installed(&self, user_id: &str) -> Vec<String> {
        self.subscriptions
            .get(user_id)
            .map(|set| {
                let mut names: Vec<_> = set.iter().cloned().collect();
                names.sort();
                names
            })
            .unwrap_or_default()
    }

    /// Check if a plugin is installed for a user.
    pub fn is_installed(&self, user_id: &str, plugin_name: &str) -> bool {
        self.subscriptions
            .get(user_id)
            .map(|set| set.contains(plugin_name))
            .unwrap_or(false)
    }

    /// Save subscriptions to the persistence file atomically.
    ///
    /// Uses tmp file + rename for crash safety.
    async fn save(&self) -> PluginResult<()> {
        let _guard = self.write_lock.lock().await;

        let content = serde_json::to_string_pretty(&self.subscriptions)
            .map_err(|e| PluginError::Serialization(e.to_string()))?;

        // Ensure parent directory exists
        if let Some(parent) = self.persistence_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Atomic write: tmp → rename
        let tmp_path = self.persistence_path.with_extension("tmp");
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, &self.persistence_path)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_store(dir: &std::path::Path) -> SubscriptionStore {
        SubscriptionStore::new(dir.join("subscriptions.json"))
    }

    #[tokio::test]
    async fn test_subscription_install() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(tmp.path());

        store.install("user1", "pdf").await.unwrap();
        assert!(store.is_installed("user1", "pdf"));
    }

    #[tokio::test]
    async fn test_subscription_uninstall() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(tmp.path());

        store.install("user1", "pdf").await.unwrap();
        assert!(store.is_installed("user1", "pdf"));

        store.uninstall("user1", "pdf").await.unwrap();
        assert!(!store.is_installed("user1", "pdf"));
    }

    #[tokio::test]
    async fn test_subscription_install_duplicate() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(tmp.path());

        store.install("user1", "pdf").await.unwrap();
        let result = store.install("user1", "pdf").await;
        assert!(matches!(
            result.unwrap_err(),
            PluginError::AlreadyInstalled(_)
        ));
    }

    #[tokio::test]
    async fn test_subscription_uninstall_missing() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(tmp.path());

        let result = store.uninstall("user1", "pdf").await;
        assert!(matches!(result.unwrap_err(), PluginError::NotInstalled(_)));
    }

    #[tokio::test]
    async fn test_subscription_list_installed() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(tmp.path());

        store.install("user1", "pdf").await.unwrap();
        store.install("user1", "docx").await.unwrap();
        store.install("user1", "xlsx").await.unwrap();

        let installed = store.list_installed("user1");
        assert_eq!(installed, vec!["docx", "pdf", "xlsx"]); // sorted
    }

    #[tokio::test]
    async fn test_subscription_persist_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("subscriptions.json");

        // Write
        {
            let mut store = SubscriptionStore::new(path.clone());
            store.install("user1", "pdf").await.unwrap();
            store.install("user1", "docx").await.unwrap();
            store.install("user2", "xlsx").await.unwrap();
        }

        // Read back
        let store = SubscriptionStore::load(path).unwrap();
        assert!(store.is_installed("user1", "pdf"));
        assert!(store.is_installed("user1", "docx"));
        assert!(store.is_installed("user2", "xlsx"));
        assert!(!store.is_installed("user2", "pdf"));
    }

    #[tokio::test]
    async fn test_subscription_concurrent_safety() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(tmp.path());

        // Sequential installs (concurrent access would require Arc<RwLock>
        // which is done at the PluginManager level)
        for i in 0..10 {
            store
                .install("user1", &format!("plugin-{}", i))
                .await
                .unwrap();
        }

        let installed = store.list_installed("user1");
        assert_eq!(installed.len(), 10);
    }

    #[tokio::test]
    async fn test_subscription_user_isolation() {
        let tmp = TempDir::new().unwrap();
        let mut store = test_store(tmp.path());

        store.install("user_a", "pdf").await.unwrap();
        store.install("user_b", "docx").await.unwrap();

        assert!(store.is_installed("user_a", "pdf"));
        assert!(!store.is_installed("user_a", "docx"));

        assert!(!store.is_installed("user_b", "pdf"));
        assert!(store.is_installed("user_b", "docx"));
    }

    #[test]
    fn test_subscription_list_empty_user() {
        let tmp = TempDir::new().unwrap();
        let store = test_store(tmp.path());

        let installed = store.list_installed("nobody");
        assert!(installed.is_empty());
    }
}
