//! Plugin catalog — global registry of all available plugins.
//!
//! Scans configured directories, loads plugins via [`PluginLoader`],
//! and provides lookup/browsing/reload capabilities.

use std::collections::HashMap;
use std::path::PathBuf;

use super::error::{PluginError, PluginResult};
use super::loader::{LoadedPlugin, PluginLoader};
use super::manifest::CatalogEntry;

/// Global plugin catalog loaded from configured directories.
pub struct PluginCatalog {
    /// Loaded plugins keyed by name.
    plugins: HashMap<String, LoadedPlugin>,

    /// Directories to scan for plugins.
    catalog_dirs: Vec<PathBuf>,
}

impl PluginCatalog {
    /// Create an empty catalog with the given scan directories.
    pub fn new(catalog_dirs: Vec<PathBuf>) -> Self {
        Self {
            plugins: HashMap::new(),
            catalog_dirs,
        }
    }

    /// Scan configured directories and load all plugins.
    ///
    /// Returns the number of plugins discovered.
    pub fn discover(&mut self) -> usize {
        let loaded = PluginLoader::discover(&self.catalog_dirs);
        self.plugins.clear();
        for plugin in loaded {
            self.plugins.insert(plugin.manifest.name.clone(), plugin);
        }
        self.plugins.len()
    }

    /// List all loaded plugins.
    pub fn list_all(&self) -> Vec<&LoadedPlugin> {
        let mut plugins: Vec<_> = self.plugins.values().collect();
        plugins.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        plugins
    }

    /// Get a specific plugin by name.
    pub fn get(&self, name: &str) -> Option<&LoadedPlugin> {
        self.plugins.get(name)
    }

    /// Reload the catalog by re-scanning directories.
    ///
    /// Returns the number of plugins after reload.
    pub fn reload(&mut self) -> usize {
        tracing::info!(
            "Reloading plugin catalog from {} directories",
            self.catalog_dirs.len()
        );
        self.discover()
    }

    /// Return the total number of loaded plugins.
    pub fn count(&self) -> usize {
        self.plugins.len()
    }

    /// Build catalog entries for API responses.
    ///
    /// The `installed` field is populated based on the given installed set.
    pub fn browse(&self, installed_names: &[String]) -> Vec<CatalogEntry> {
        self.list_all()
            .iter()
            .map(|p| CatalogEntry {
                name: p.manifest.name.clone(),
                description: p.manifest.description.clone(),
                version: p.manifest.version.clone(),
                format: p.manifest.format.to_string(),
                author: p.manifest.author.clone(),
                skills_count: p.skills.len(),
                references: p
                    .reference_paths
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect(),
                has_scripts: p.scripts_dir.is_some(),
                has_mcp: p.mcp_config.is_some(),
                installed: installed_names.contains(&p.manifest.name),
            })
            .collect()
    }

    /// Get a single catalog entry by name.
    pub fn get_entry(&self, name: &str, installed: bool) -> PluginResult<CatalogEntry> {
        let p = self
            .get(name)
            .ok_or_else(|| PluginError::NotFound(name.to_string()))?;

        Ok(CatalogEntry {
            name: p.manifest.name.clone(),
            description: p.manifest.description.clone(),
            version: p.manifest.version.clone(),
            format: p.manifest.format.to_string(),
            author: p.manifest.author.clone(),
            skills_count: p.skills.len(),
            references: p
                .reference_paths
                .iter()
                .map(|(name, _)| name.clone())
                .collect(),
            has_scripts: p.scripts_dir.is_some(),
            has_mcp: p.mcp_config.is_some(),
            installed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn test_catalog_discover_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let mut catalog = PluginCatalog::new(vec![tmp.path().to_path_buf()]);
        let count = catalog.discover();
        assert_eq!(count, 0);
        assert_eq!(catalog.count(), 0);
        assert!(catalog.list_all().is_empty());
    }

    #[test]
    fn test_catalog_discover_four_plugins() {
        let tmp = TempDir::new().unwrap();
        create_temp_plugin(tmp.path(), "pdf");
        create_temp_plugin(tmp.path(), "docx");
        create_temp_plugin(tmp.path(), "pptx");
        create_temp_plugin(tmp.path(), "xlsx");

        let mut catalog = PluginCatalog::new(vec![tmp.path().to_path_buf()]);
        let count = catalog.discover();
        assert_eq!(count, 4);

        let all = catalog.list_all();
        assert_eq!(all.len(), 4);
        // Sorted alphabetically
        assert_eq!(all[0].manifest.name, "docx");
        assert_eq!(all[1].manifest.name, "pdf");
        assert_eq!(all[2].manifest.name, "pptx");
        assert_eq!(all[3].manifest.name, "xlsx");
    }

    #[test]
    fn test_catalog_get_existing() {
        let tmp = TempDir::new().unwrap();
        create_temp_plugin(tmp.path(), "pdf");

        let mut catalog = PluginCatalog::new(vec![tmp.path().to_path_buf()]);
        catalog.discover();

        assert!(catalog.get("pdf").is_some());
        assert_eq!(catalog.get("pdf").unwrap().manifest.name, "pdf");
    }

    #[test]
    fn test_catalog_get_nonexistent() {
        let tmp = TempDir::new().unwrap();
        let mut catalog = PluginCatalog::new(vec![tmp.path().to_path_buf()]);
        catalog.discover();

        assert!(catalog.get("missing").is_none());
    }

    #[test]
    fn test_catalog_browse_with_installed() {
        let tmp = TempDir::new().unwrap();
        create_temp_plugin(tmp.path(), "pdf");
        create_temp_plugin(tmp.path(), "docx");

        let mut catalog = PluginCatalog::new(vec![tmp.path().to_path_buf()]);
        catalog.discover();

        let entries = catalog.browse(&["pdf".to_string()]);
        assert_eq!(entries.len(), 2);

        let pdf = entries.iter().find(|e| e.name == "pdf").unwrap();
        assert!(pdf.installed);

        let docx = entries.iter().find(|e| e.name == "docx").unwrap();
        assert!(!docx.installed);
    }

    #[test]
    fn test_catalog_reload() {
        let tmp = TempDir::new().unwrap();
        create_temp_plugin(tmp.path(), "pdf");

        let mut catalog = PluginCatalog::new(vec![tmp.path().to_path_buf()]);
        let count1 = catalog.discover();
        assert_eq!(count1, 1);

        // Add another plugin and reload
        create_temp_plugin(tmp.path(), "docx");
        let count2 = catalog.reload();
        assert_eq!(count2, 2);
    }

    #[test]
    fn test_catalog_get_entry() {
        let tmp = TempDir::new().unwrap();
        create_temp_plugin(tmp.path(), "pdf");

        let mut catalog = PluginCatalog::new(vec![tmp.path().to_path_buf()]);
        catalog.discover();

        let entry = catalog.get_entry("pdf", true).unwrap();
        assert_eq!(entry.name, "pdf");
        assert!(entry.installed);
        assert_eq!(entry.format, "ClaudeSkills");

        let err = catalog.get_entry("missing", false);
        assert!(err.is_err());
    }
}
