//! Connector category system — maps `~~category` placeholders to active connectors.
//!
//! Categories provide an abstraction layer between plugin bundles and
//! concrete connector implementations. A bundle declares that it needs
//! `~~file-system` capabilities, and the category resolver finds which
//! connectors satisfy that requirement.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// A connector category definition from config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryDefinition {
    /// Display name for UI.
    pub display_name: String,

    /// Human-readable description.
    pub description: String,

    /// Default connector names that satisfy this category.
    #[serde(default)]
    pub default_connectors: Vec<String>,

    /// Optional platform restriction (e.g., "macos", "windows").
    #[serde(default)]
    pub platform: Option<String>,

    /// Icon identifier for UI.
    #[serde(default)]
    pub icon: Option<String>,
}

/// A connector category with its ID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorCategory {
    /// Category ID (e.g., "~~file-system").
    pub id: String,

    /// Category definition.
    #[serde(flatten)]
    pub definition: CategoryDefinition,
}

/// Configuration file structure for connector categories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoriesConfig {
    /// Category definitions keyed by category ID.
    pub categories: HashMap<String, CategoryDefinition>,
}

/// Resolves `~~category` placeholders to active connector namespaces.
///
/// Each connector registers which categories it satisfies. When a bundle
/// declares required/optional categories, the resolver finds matching
/// connectors from the user's active set.
pub struct CategoryResolver {
    /// category_id → Vec<(connector_name, priority)>
    registry: HashMap<String, Vec<(String, u32)>>,

    /// Category definitions loaded from config.
    definitions: HashMap<String, CategoryDefinition>,
}

impl CategoryResolver {
    /// Create an empty resolver.
    pub fn new() -> Self {
        Self {
            registry: HashMap::new(),
            definitions: HashMap::new(),
        }
    }

    /// Create a resolver from a categories config.
    pub fn from_config(config: CategoriesConfig) -> Self {
        let mut resolver = Self::new();

        for (cat_id, def) in &config.categories {
            // Register default connectors for each category
            for (priority, connector) in def.default_connectors.iter().enumerate() {
                resolver.register(cat_id, connector, priority as u32);
            }
        }

        resolver.definitions = config.categories;
        resolver
    }

    /// Create a resolver with built-in default categories.
    pub fn with_defaults() -> Self {
        let config = Self::default_config();
        Self::from_config(config)
    }

    /// Register a connector for a category with a priority.
    ///
    /// Lower priority values are preferred.
    pub fn register(&mut self, category: &str, connector: &str, priority: u32) {
        let entries = self.registry.entry(category.to_string()).or_default();
        entries.push((connector.to_string(), priority));
        entries.sort_by_key(|x| x.1);
    }

    /// Resolve a single category to matching active connectors.
    ///
    /// Returns connector names that both:
    /// 1. Are registered for the given category
    /// 2. Are in the user's active connector set
    pub fn resolve(&self, category: &str, active_connectors: &HashSet<String>) -> Vec<String> {
        self.registry
            .get(category)
            .map(|entries| {
                entries
                    .iter()
                    .filter(|(name, _)| active_connectors.contains(name))
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Resolve multiple categories and merge results (deduplicated).
    pub fn resolve_many(
        &self,
        categories: &[String],
        active_connectors: &HashSet<String>,
    ) -> Vec<String> {
        let mut result: Vec<String> = categories
            .iter()
            .flat_map(|cat| self.resolve(cat, active_connectors))
            .collect();
        result.sort();
        result.dedup();
        result
    }

    /// Get the default connectors for a category (from definition).
    pub fn get_default_connectors(&self, category: &str) -> Vec<String> {
        self.definitions
            .get(category)
            .map(|def| def.default_connectors.clone())
            .unwrap_or_default()
    }

    /// Get a category definition by ID.
    pub fn get_definition(&self, category: &str) -> Option<&CategoryDefinition> {
        self.definitions.get(category)
    }

    /// List all registered category IDs.
    pub fn list_categories(&self) -> Vec<String> {
        let mut cats: Vec<String> = self.definitions.keys().cloned().collect();
        cats.sort();
        cats
    }

    /// List all categories with their definitions (for API responses).
    pub fn list_categories_with_definitions(&self) -> Vec<ConnectorCategory> {
        let mut result: Vec<ConnectorCategory> = self
            .definitions
            .iter()
            .map(|(id, def)| ConnectorCategory {
                id: id.clone(),
                definition: def.clone(),
            })
            .collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }

    /// Build the default categories configuration.
    fn default_config() -> CategoriesConfig {
        let mut categories = HashMap::new();

        categories.insert(
            "~~file-system".to_string(),
            CategoryDefinition {
                display_name: "File System".to_string(),
                description: "Read, write, and manage files".to_string(),
                default_connectors: vec!["filesystem".to_string()],
                platform: None,
                icon: Some("folder".to_string()),
            },
        );

        categories.insert(
            "~~code-runner".to_string(),
            CategoryDefinition {
                display_name: "Code Execution".to_string(),
                description: "Execute code in sandboxed environments".to_string(),
                default_connectors: vec!["executor".to_string()],
                platform: None,
                icon: Some("terminal".to_string()),
            },
        );

        categories.insert(
            "~~web-browser".to_string(),
            CategoryDefinition {
                display_name: "Web Browser".to_string(),
                description: "Web automation and scraping".to_string(),
                default_connectors: vec!["browser".to_string()],
                platform: None,
                icon: Some("globe".to_string()),
            },
        );

        categories.insert(
            "~~mac-automation".to_string(),
            CategoryDefinition {
                display_name: "macOS Automation".to_string(),
                description: "AppleScript and macOS system control".to_string(),
                default_connectors: vec!["mac".to_string()],
                platform: Some("macos".to_string()),
                icon: Some("laptop".to_string()),
            },
        );

        categories.insert(
            "~~presentation".to_string(),
            CategoryDefinition {
                display_name: "Presentation Tools".to_string(),
                description: "Create and edit slide presentations".to_string(),
                default_connectors: vec!["pptx".to_string()],
                platform: None,
                icon: Some("presentation".to_string()),
            },
        );

        categories.insert(
            "~~spreadsheet".to_string(),
            CategoryDefinition {
                display_name: "Spreadsheet Tools".to_string(),
                description: "Read, write, and analyze spreadsheets".to_string(),
                default_connectors: vec!["xlsx".to_string()],
                platform: None,
                icon: Some("table".to_string()),
            },
        );

        categories.insert(
            "~~document".to_string(),
            CategoryDefinition {
                display_name: "Document Tools".to_string(),
                description: "Create and edit documents".to_string(),
                default_connectors: vec!["docx".to_string()],
                platform: None,
                icon: Some("file-text".to_string()),
            },
        );

        categories.insert(
            "~~pdf".to_string(),
            CategoryDefinition {
                display_name: "PDF Tools".to_string(),
                description: "Read, create, and manipulate PDFs".to_string(),
                default_connectors: vec!["pdf".to_string()],
                platform: None,
                icon: Some("file".to_string()),
            },
        );

        categories.insert(
            "~~data-warehouse".to_string(),
            CategoryDefinition {
                display_name: "Data Warehouse".to_string(),
                description: "Connect to data warehouses and databases".to_string(),
                default_connectors: Vec::new(),
                platform: None,
                icon: Some("database".to_string()),
            },
        );

        CategoriesConfig { categories }
    }
}

impl Default for CategoryResolver {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_resolve() {
        let mut resolver = CategoryResolver::new();
        resolver.register("~~file-system", "filesystem", 0);
        resolver.register("~~file-system", "local-fs", 1);

        let active: HashSet<String> = ["filesystem".to_string()].into_iter().collect();
        let result = resolver.resolve("~~file-system", &active);
        assert_eq!(result, vec!["filesystem"]);
    }

    #[test]
    fn test_resolve_no_active_match() {
        let mut resolver = CategoryResolver::new();
        resolver.register("~~file-system", "filesystem", 0);

        let active: HashSet<String> = ["other".to_string()].into_iter().collect();
        let result = resolver.resolve("~~file-system", &active);
        assert!(result.is_empty());
    }

    #[test]
    fn test_resolve_unknown_category() {
        let resolver = CategoryResolver::new();
        let active: HashSet<String> = ["filesystem".to_string()].into_iter().collect();
        let result = resolver.resolve("~~unknown", &active);
        assert!(result.is_empty());
    }

    #[test]
    fn test_resolve_many_dedup() {
        let mut resolver = CategoryResolver::new();
        resolver.register("~~file-system", "filesystem", 0);
        resolver.register("~~code-runner", "filesystem", 0); // same connector
        resolver.register("~~code-runner", "executor", 1);

        let active: HashSet<String> = ["filesystem".to_string(), "executor".to_string()]
            .into_iter()
            .collect();

        let result =
            resolver.resolve_many(&["~~file-system".into(), "~~code-runner".into()], &active);
        // Should be deduplicated and sorted
        assert_eq!(result, vec!["executor", "filesystem"]);
    }

    #[test]
    fn test_with_defaults() {
        let resolver = CategoryResolver::with_defaults();
        let cats = resolver.list_categories();
        assert!(cats.contains(&"~~file-system".to_string()));
        assert!(cats.contains(&"~~code-runner".to_string()));
        assert!(cats.contains(&"~~web-browser".to_string()));
        assert!(cats.contains(&"~~presentation".to_string()));
        assert!(cats.contains(&"~~spreadsheet".to_string()));
        assert!(cats.contains(&"~~document".to_string()));
        assert!(cats.contains(&"~~pdf".to_string()));
    }

    #[test]
    fn test_default_connectors() {
        let resolver = CategoryResolver::with_defaults();
        let defaults = resolver.get_default_connectors("~~file-system");
        assert_eq!(defaults, vec!["filesystem"]);

        let defaults = resolver.get_default_connectors("~~data-warehouse");
        assert!(defaults.is_empty());
    }

    #[test]
    fn test_from_config() {
        let mut categories = HashMap::new();
        categories.insert(
            "~~test".to_string(),
            CategoryDefinition {
                display_name: "Test".to_string(),
                description: "Test category".to_string(),
                default_connectors: vec!["test-conn".to_string()],
                platform: None,
                icon: None,
            },
        );

        let config = CategoriesConfig { categories };
        let resolver = CategoryResolver::from_config(config);

        let active: HashSet<String> = ["test-conn".to_string()].into_iter().collect();
        let result = resolver.resolve("~~test", &active);
        assert_eq!(result, vec!["test-conn"]);
    }

    #[test]
    fn test_list_categories_with_definitions() {
        let resolver = CategoryResolver::with_defaults();
        let cats = resolver.list_categories_with_definitions();
        assert!(!cats.is_empty());

        // Verify sorted by id
        for i in 1..cats.len() {
            assert!(cats[i - 1].id <= cats[i].id);
        }

        // Verify each has a display_name
        for cat in &cats {
            assert!(!cat.definition.display_name.is_empty());
        }
    }

    #[test]
    fn test_priority_ordering() {
        let mut resolver = CategoryResolver::new();
        resolver.register("~~test", "low-priority", 10);
        resolver.register("~~test", "high-priority", 1);

        let active: HashSet<String> = ["low-priority".to_string(), "high-priority".to_string()]
            .into_iter()
            .collect();
        let result = resolver.resolve("~~test", &active);
        // High priority should come first
        assert_eq!(result, vec!["high-priority", "low-priority"]);
    }

    #[test]
    fn test_get_definition() {
        let resolver = CategoryResolver::with_defaults();
        let def = resolver.get_definition("~~file-system").unwrap();
        assert_eq!(def.display_name, "File System");
        assert_eq!(def.icon, Some("folder".to_string()));

        assert!(resolver.get_definition("~~nonexistent").is_none());
    }
}
