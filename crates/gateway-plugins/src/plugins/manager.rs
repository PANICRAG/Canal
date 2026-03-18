//! Plugin manager — coordinates catalog, subscriptions, and skill injection.
//!
//! Central entry point for plugin operations: browse, install, uninstall,
//! and get user-specific skills for system prompt injection.

use std::sync::Arc;

use tokio::sync::RwLock;

use crate::skills::definition::Skill;

use super::catalog::PluginCatalog;
use super::error::{PluginError, PluginResult};
use super::manifest::CatalogEntry;
use super::subscription::SubscriptionStore;

/// Plugin manager coordinating catalog and subscriptions.
pub struct PluginManager {
    /// Global plugin catalog (all available plugins).
    pub catalog: Arc<RwLock<PluginCatalog>>,

    /// Per-user subscription store.
    pub subscriptions: Arc<RwLock<SubscriptionStore>>,
}

impl PluginManager {
    /// Create a new plugin manager.
    pub fn new(catalog: PluginCatalog, subscriptions: SubscriptionStore) -> Self {
        Self {
            catalog: Arc::new(RwLock::new(catalog)),
            subscriptions: Arc::new(RwLock::new(subscriptions)),
        }
    }

    /// Get all skills for a user (from their installed plugins).
    pub async fn get_user_skills(&self, user_id: &str) -> Vec<Skill> {
        let subs = self.subscriptions.read().await;
        let installed = subs.list_installed(user_id);
        drop(subs);

        if installed.is_empty() {
            return Vec::new();
        }

        let catalog = self.catalog.read().await;
        let mut skills = Vec::new();

        for plugin_name in &installed {
            if let Some(plugin) = catalog.get(plugin_name) {
                for skill in &plugin.skills {
                    let mut s = skill.clone();
                    // Tag skill with plugin origin
                    s.metadata.plugin_name = Some(plugin_name.clone());
                    skills.push(s);
                }
            }
        }

        skills
    }

    /// Get a specific skill from a user's installed plugins.
    pub async fn get_user_skill(&self, user_id: &str, skill_name: &str) -> Option<Skill> {
        let subs = self.subscriptions.read().await;
        let installed = subs.list_installed(user_id);
        drop(subs);

        let catalog = self.catalog.read().await;

        for plugin_name in &installed {
            if let Some(plugin) = catalog.get(plugin_name) {
                for skill in &plugin.skills {
                    if skill.name == skill_name {
                        let mut s = skill.clone();
                        s.metadata.plugin_name = Some(plugin_name.clone());
                        return Some(s);
                    }
                }
            }
        }

        None
    }

    /// Get installed skill names for a user.
    pub async fn get_installed_skill_names(&self, user_id: &str) -> Vec<String> {
        let skills = self.get_user_skills(user_id).await;
        skills.into_iter().map(|s| s.name).collect()
    }

    /// Install a plugin for a user.
    pub async fn install_plugin(&self, user_id: &str, plugin_name: &str) -> PluginResult<()> {
        // Verify plugin exists in catalog
        {
            let catalog = self.catalog.read().await;
            if catalog.get(plugin_name).is_none() {
                return Err(PluginError::NotFound(plugin_name.to_string()));
            }
        }

        let mut subs = self.subscriptions.write().await;
        subs.install(user_id, plugin_name).await
    }

    /// Uninstall a plugin for a user.
    pub async fn uninstall_plugin(&self, user_id: &str, plugin_name: &str) -> PluginResult<()> {
        let mut subs = self.subscriptions.write().await;
        subs.uninstall(user_id, plugin_name).await
    }

    /// Browse the catalog with user's installed status.
    pub async fn browse_catalog(&self, user_id: &str) -> Vec<CatalogEntry> {
        let subs = self.subscriptions.read().await;
        let installed = subs.list_installed(user_id);
        drop(subs);

        let catalog = self.catalog.read().await;
        catalog.browse(&installed)
    }

    /// Get a single catalog entry with user's installed status.
    pub async fn get_catalog_entry(
        &self,
        user_id: &str,
        plugin_name: &str,
    ) -> PluginResult<CatalogEntry> {
        let subs = self.subscriptions.read().await;
        let installed = subs.is_installed(user_id, plugin_name);
        drop(subs);

        let catalog = self.catalog.read().await;
        catalog.get_entry(plugin_name, installed)
    }

    /// Reload the catalog.
    pub async fn reload_catalog(&self) -> usize {
        let mut catalog = self.catalog.write().await;
        catalog.reload()
    }

    /// Get reference content for a plugin.
    pub async fn get_reference(
        &self,
        plugin_name: &str,
        reference_name: &str,
    ) -> PluginResult<String> {
        let catalog = self.catalog.read().await;
        let plugin = catalog
            .get(plugin_name)
            .ok_or_else(|| PluginError::NotFound(plugin_name.to_string()))?;

        for (name, path) in &plugin.reference_paths {
            if name == reference_name {
                return std::fs::read_to_string(path).map_err(PluginError::Io);
            }
        }

        Err(PluginError::NotFound(format!(
            "reference {} in plugin {}",
            reference_name, plugin_name
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::catalog::PluginCatalog;
    use crate::plugins::subscription::SubscriptionStore;
    use std::fs;
    use tempfile::TempDir;

    fn create_temp_plugin(dir: &std::path::Path, name: &str) {
        let plugin_dir = dir.join(name);
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("SKILL.md"),
            format!(
                "---\nname: {}\ndescription: {} processing plugin\nversion: \"1.0.0\"\n---\n\n# {}\n\nContent here.",
                name, name, name
            ),
        )
        .unwrap();
    }

    fn setup_manager(tmp: &TempDir) -> PluginManager {
        let plugins_dir = tmp.path().join("plugins");
        fs::create_dir_all(&plugins_dir).unwrap();
        create_temp_plugin(&plugins_dir, "pdf");
        create_temp_plugin(&plugins_dir, "docx");
        create_temp_plugin(&plugins_dir, "xlsx");

        let mut catalog = PluginCatalog::new(vec![plugins_dir]);
        catalog.discover();

        let subs = SubscriptionStore::new(tmp.path().join("subs.json"));
        PluginManager::new(catalog, subs)
    }

    #[tokio::test]
    async fn test_manager_get_user_skills() {
        let tmp = TempDir::new().unwrap();
        let manager = setup_manager(&tmp);

        manager.install_plugin("user1", "pdf").await.unwrap();
        manager.install_plugin("user1", "docx").await.unwrap();

        let skills = manager.get_user_skills("user1").await;
        assert_eq!(skills.len(), 2);

        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"pdf"));
        assert!(names.contains(&"docx"));

        // Verify plugin_name is set
        for skill in &skills {
            assert!(skill.metadata.plugin_name.is_some());
        }
    }

    #[tokio::test]
    async fn test_manager_get_user_skills_empty() {
        let tmp = TempDir::new().unwrap();
        let manager = setup_manager(&tmp);

        let skills = manager.get_user_skills("user1").await;
        assert!(skills.is_empty());
    }

    #[tokio::test]
    async fn test_manager_install_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let manager = setup_manager(&tmp);

        let result = manager.install_plugin("user1", "nonexistent").await;
        assert!(matches!(result.unwrap_err(), PluginError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_manager_browse_catalog() {
        let tmp = TempDir::new().unwrap();
        let manager = setup_manager(&tmp);

        manager.install_plugin("user1", "pdf").await.unwrap();

        let entries = manager.browse_catalog("user1").await;
        assert_eq!(entries.len(), 3);

        let pdf = entries.iter().find(|e| e.name == "pdf").unwrap();
        assert!(pdf.installed);

        let docx = entries.iter().find(|e| e.name == "docx").unwrap();
        assert!(!docx.installed);
    }

    #[tokio::test]
    async fn test_manager_user_isolation() {
        let tmp = TempDir::new().unwrap();
        let manager = setup_manager(&tmp);

        manager.install_plugin("user_a", "pdf").await.unwrap();
        manager.install_plugin("user_b", "docx").await.unwrap();

        let skills_a = manager.get_user_skills("user_a").await;
        assert_eq!(skills_a.len(), 1);
        assert_eq!(skills_a[0].name, "pdf");

        let skills_b = manager.get_user_skills("user_b").await;
        assert_eq!(skills_b.len(), 1);
        assert_eq!(skills_b[0].name, "docx");
    }

    #[tokio::test]
    async fn test_manager_get_user_skill() {
        let tmp = TempDir::new().unwrap();
        let manager = setup_manager(&tmp);

        manager.install_plugin("user1", "pdf").await.unwrap();

        let skill = manager.get_user_skill("user1", "pdf").await;
        assert!(skill.is_some());
        assert_eq!(skill.unwrap().name, "pdf");

        // Not installed skill
        let skill = manager.get_user_skill("user1", "docx").await;
        assert!(skill.is_none());
    }

    #[tokio::test]
    async fn test_manager_get_catalog_entry() {
        let tmp = TempDir::new().unwrap();
        let manager = setup_manager(&tmp);

        manager.install_plugin("user1", "pdf").await.unwrap();

        let entry = manager.get_catalog_entry("user1", "pdf").await.unwrap();
        assert_eq!(entry.name, "pdf");
        assert!(entry.installed);

        let entry = manager.get_catalog_entry("user1", "docx").await.unwrap();
        assert!(!entry.installed);
    }

    #[tokio::test]
    async fn test_manager_reload_catalog() {
        let tmp = TempDir::new().unwrap();
        let manager = setup_manager(&tmp);

        let count = manager.reload_catalog().await;
        assert_eq!(count, 3); // pdf, docx, xlsx
    }

    #[tokio::test]
    async fn test_manager_get_reference() {
        let tmp = TempDir::new().unwrap();
        let plugins_dir = tmp.path().join("plugins");
        fs::create_dir_all(&plugins_dir).unwrap();
        create_temp_plugin(&plugins_dir, "pdf");

        // Add a reference file
        fs::write(
            plugins_dir.join("pdf/REFERENCE.md"),
            "# PDF Reference\n\nDetailed guide.",
        )
        .unwrap();

        let mut catalog = PluginCatalog::new(vec![plugins_dir]);
        catalog.discover();

        let subs = SubscriptionStore::new(tmp.path().join("subs.json"));
        let manager = PluginManager::new(catalog, subs);

        let content = manager.get_reference("pdf", "REFERENCE.md").await.unwrap();
        assert!(content.contains("PDF Reference"));

        // Non-existent reference
        let err = manager.get_reference("pdf", "MISSING.md").await;
        assert!(err.is_err());
    }
}
