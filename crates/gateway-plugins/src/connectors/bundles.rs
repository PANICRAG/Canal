//! Bundle manager — loads, manages, and resolves plugin bundles.
//!
//! A plugin bundle is a higher-level package that combines connectors,
//! skills, system prompts, and configuration into a coherent capability set.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::categories::CategoryResolver;

/// MCP server definition parsed from `.mcp.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerDef {
    /// Server name (key in mcpServers object).
    pub name: String,
    /// Transport type ("http" or "stdio").
    pub transport_type: String,
    /// Server URL (for HTTP transport).
    pub url: String,
    /// Optional auth token for Bearer authentication.
    #[serde(default)]
    pub auth_token: Option<String>,
}

/// Plugin bundle definition loaded from a `plugin.json` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleDefinition {
    /// Bundle unique name (directory name).
    pub name: String,

    /// Semantic version.
    pub version: String,

    /// Human-readable description.
    pub description: String,

    /// Bundle author.
    #[serde(default)]
    pub author: Option<String>,

    /// Required connector categories (must have at least one active connector).
    #[serde(default)]
    pub required_categories: Vec<String>,

    /// Optional connector categories (enhanced functionality if available).
    #[serde(default)]
    pub optional_categories: Vec<String>,

    /// Skill definition files (relative paths to SKILL.md files).
    #[serde(default)]
    pub skill_files: Vec<String>,

    /// System prompt to inject when bundle is active.
    #[serde(default)]
    pub system_prompt: Option<String>,

    /// Configuration overrides when bundle is active.
    #[serde(default)]
    pub config: BundleConfig,

    /// MCP servers loaded from `.mcp.json` (not part of plugin.json).
    #[serde(skip)]
    pub mcp_servers: Vec<McpServerDef>,
}

/// Bundle-specific configuration overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundleConfig {
    /// Timeout override in milliseconds.
    #[serde(default)]
    pub timeout_ms: Option<u64>,

    /// Max tokens override.
    #[serde(default)]
    pub max_tokens: Option<u32>,

    /// Additional custom config key-values.
    #[serde(default)]
    pub custom: HashMap<String, serde_json::Value>,
}

/// Result of resolving a bundle's connector requirements.
#[derive(Debug, Clone)]
pub struct BundleResolution {
    /// Bundle name.
    pub name: String,

    /// Resolved connector namespaces to enable.
    pub enabled_namespaces: Vec<String>,

    /// Warnings about optional connectors that are missing.
    pub warnings: Vec<String>,

    /// The bundle's system prompt (if any).
    pub system_prompt: Option<String>,

    /// The bundle's config overrides.
    pub config: BundleConfig,
}

/// Loads plugin bundle definitions from directories.
pub struct BundleLoader;

impl BundleLoader {
    /// Discover all bundles in the given directories.
    pub fn discover(dirs: &[PathBuf]) -> Vec<BundleDefinition> {
        let mut bundles = Vec::new();

        for dir in dirs {
            if !dir.is_dir() {
                tracing::warn!("Bundle directory does not exist: {}", dir.display());
                continue;
            }

            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) => {
                    tracing::warn!("Failed to read bundle directory {}: {}", dir.display(), e);
                    continue;
                }
            };

            for entry in entries.flatten() {
                let entry_path = entry.path();
                if !entry_path.is_dir() {
                    continue;
                }

                // Skip hidden directories
                if entry_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with('.'))
                    .unwrap_or(false)
                {
                    continue;
                }

                match Self::load_bundle(&entry_path) {
                    Ok(bundle) => {
                        tracing::info!("Loaded bundle: {} v{}", bundle.name, bundle.version);
                        bundles.push(bundle);
                    }
                    Err(e) => {
                        tracing::debug!(
                            "Skipping directory {} (not a bundle): {}",
                            entry_path.display(),
                            e
                        );
                    }
                }
            }
        }

        bundles
    }

    /// Load a single bundle from a directory.
    fn load_bundle(dir: &Path) -> Result<BundleDefinition, String> {
        let plugin_json = dir.join("plugin.json");
        if !plugin_json.exists() {
            return Err("no plugin.json found".to_string());
        }

        let content = std::fs::read_to_string(&plugin_json)
            .map_err(|e| format!("read plugin.json: {}", e))?;

        let mut bundle: BundleDefinition =
            serde_json::from_str(&content).map_err(|e| format!("parse plugin.json: {}", e))?;

        // Use directory name if name is empty
        if bundle.name.is_empty() {
            bundle.name = dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
        }

        // Load system prompt from PROMPT.md if not inline
        if bundle.system_prompt.is_none() {
            let prompt_path = dir.join("PROMPT.md");
            if prompt_path.exists() {
                if let Ok(prompt) = std::fs::read_to_string(&prompt_path) {
                    bundle.system_prompt = Some(prompt);
                }
            }
        }

        // Parse .mcp.json if present
        let mcp_path = dir.join(".mcp.json");
        if mcp_path.exists() {
            if let Ok(mcp_content) = std::fs::read_to_string(&mcp_path) {
                if let Ok(mcp_json) = serde_json::from_str::<serde_json::Value>(&mcp_content) {
                    if let Some(servers) = mcp_json.get("mcpServers").and_then(|s| s.as_object()) {
                        for (name, config) in servers {
                            let url = config.get("url").and_then(|u| u.as_str()).unwrap_or("");
                            if url.is_empty() {
                                tracing::debug!(
                                    server = %name,
                                    bundle = %bundle.name,
                                    "Skipping MCP server with empty URL"
                                );
                                continue;
                            }
                            let transport_type = config
                                .get("type")
                                .and_then(|t| t.as_str())
                                .unwrap_or("http");
                            let auth_token = config
                                .get("auth_token")
                                .and_then(|t| t.as_str())
                                .map(String::from);
                            bundle.mcp_servers.push(McpServerDef {
                                name: name.clone(),
                                transport_type: transport_type.to_string(),
                                url: url.to_string(),
                                auth_token,
                            });
                        }
                        tracing::info!(
                            bundle = %bundle.name,
                            mcp_server_count = bundle.mcp_servers.len(),
                            "Loaded MCP server definitions from .mcp.json"
                        );
                    }
                }
            }
        }

        // Discover skill files if not explicitly listed
        if bundle.skill_files.is_empty() {
            let skills_dir = dir.join("skills");
            if skills_dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&skills_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_dir() {
                            let skill_md = p.join("SKILL.md");
                            if skill_md.exists() {
                                if let Some(rel) = skill_md.strip_prefix(dir).ok() {
                                    bundle.skill_files.push(rel.to_string_lossy().to_string());
                                }
                            }
                        } else if p.extension().and_then(|e| e.to_str()) == Some("md") {
                            if let Some(rel) = p.strip_prefix(dir).ok() {
                                bundle.skill_files.push(rel.to_string_lossy().to_string());
                            }
                        }
                    }
                }
            }
        }

        Ok(bundle)
    }
}

/// Manages the bundle registry and resolution.
pub struct BundleManager {
    /// All known bundles keyed by name.
    bundles: HashMap<String, BundleDefinition>,

    /// Directories to scan for bundles.
    bundle_dirs: Vec<PathBuf>,
}

impl BundleManager {
    /// Create a new bundle manager.
    pub fn new(bundle_dirs: Vec<PathBuf>) -> Self {
        Self {
            bundles: HashMap::new(),
            bundle_dirs,
        }
    }

    /// Scan directories and load all bundles.
    pub fn discover(&mut self) -> usize {
        let loaded = BundleLoader::discover(&self.bundle_dirs);
        self.bundles.clear();
        for bundle in loaded {
            self.bundles.insert(bundle.name.clone(), bundle);
        }
        self.bundles.len()
    }

    /// Reload bundles from directories.
    pub fn reload(&mut self) -> usize {
        self.discover()
    }

    /// Get a bundle by name.
    pub fn get(&self, name: &str) -> Option<&BundleDefinition> {
        self.bundles.get(name)
    }

    /// List all available bundles.
    pub fn list_all(&self) -> Vec<&BundleDefinition> {
        let mut bundles: Vec<_> = self.bundles.values().collect();
        bundles.sort_by(|a, b| a.name.cmp(&b.name));
        bundles
    }

    /// Count of loaded bundles.
    pub fn count(&self) -> usize {
        self.bundles.len()
    }

    /// Resolve a bundle's connector requirements against active connectors.
    pub fn resolve_bundle(
        &self,
        bundle_name: &str,
        resolver: &CategoryResolver,
        active_connectors: &HashSet<String>,
    ) -> Result<BundleResolution, BundleError> {
        let bundle = self
            .bundles
            .get(bundle_name)
            .ok_or_else(|| BundleError::NotFound(bundle_name.to_string()))?;

        resolve_bundle_connectors(bundle, resolver, active_connectors)
    }

    /// Resolve multiple bundles and merge their requirements.
    pub fn resolve_bundles(
        &self,
        bundle_names: &[String],
        resolver: &CategoryResolver,
        active_connectors: &HashSet<String>,
    ) -> Result<MergedBundleResolution, BundleError> {
        let mut all_namespaces = Vec::new();
        let mut all_warnings = Vec::new();
        let mut all_prompts = Vec::new();
        let mut merged_config = BundleConfig::default();

        for name in bundle_names {
            let resolution = self.resolve_bundle(name, resolver, active_connectors)?;
            all_namespaces.extend(resolution.enabled_namespaces);
            all_warnings.extend(resolution.warnings);
            if let Some(prompt) = resolution.system_prompt {
                all_prompts.push(prompt);
            }

            // Merge config: take max timeout, max tokens
            if let Some(t) = resolution.config.timeout_ms {
                merged_config.timeout_ms = Some(
                    merged_config
                        .timeout_ms
                        .map_or(t, |existing| existing.max(t)),
                );
            }
            if let Some(t) = resolution.config.max_tokens {
                merged_config.max_tokens = Some(
                    merged_config
                        .max_tokens
                        .map_or(t, |existing| existing.max(t)),
                );
            }
            // Merge custom config (later bundles override earlier)
            for (k, v) in resolution.config.custom {
                merged_config.custom.insert(k, v);
            }
        }

        // Deduplicate namespaces
        all_namespaces.sort();
        all_namespaces.dedup();

        Ok(MergedBundleResolution {
            enabled_namespaces: all_namespaces,
            warnings: all_warnings,
            system_prompt: if all_prompts.is_empty() {
                None
            } else {
                Some(all_prompts.join("\n---\n"))
            },
            config: merged_config,
        })
    }
}

/// Merged resolution from multiple bundles.
#[derive(Debug, Clone)]
pub struct MergedBundleResolution {
    /// All resolved namespaces (deduplicated).
    pub enabled_namespaces: Vec<String>,

    /// All warnings from all bundles.
    pub warnings: Vec<String>,

    /// Concatenated system prompts.
    pub system_prompt: Option<String>,

    /// Merged config (max of timeouts, etc.).
    pub config: BundleConfig,
}

/// Resolve a single bundle's connector requirements.
fn resolve_bundle_connectors(
    bundle: &BundleDefinition,
    resolver: &CategoryResolver,
    active_connectors: &HashSet<String>,
) -> Result<BundleResolution, BundleError> {
    let mut namespaces = Vec::new();
    let mut warnings = Vec::new();

    // Required: must have at least one active connector
    for cat in &bundle.required_categories {
        let resolved = resolver.resolve(cat, active_connectors);
        if resolved.is_empty() {
            return Err(BundleError::MissingRequired {
                bundle: bundle.name.clone(),
                category: cat.clone(),
                suggestion: resolver.get_default_connectors(cat),
            });
        }
        namespaces.extend(resolved);
    }

    // Optional: add if available, warn if not
    for cat in &bundle.optional_categories {
        let resolved = resolver.resolve(cat, active_connectors);
        if resolved.is_empty() {
            warnings.push(format!(
                "Optional connector for {} not available. Some features may be limited.",
                cat
            ));
        } else {
            namespaces.extend(resolved);
        }
    }

    // Deduplicate
    namespaces.sort();
    namespaces.dedup();

    Ok(BundleResolution {
        name: bundle.name.clone(),
        enabled_namespaces: namespaces,
        warnings,
        system_prompt: bundle.system_prompt.clone(),
        config: bundle.config.clone(),
    })
}

/// Errors from bundle operations.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// Bundle not found.
    #[error("bundle not found: {0}")]
    NotFound(String),

    /// Required connector category not satisfied.
    #[error("bundle '{bundle}' requires '{category}' but no active connector satisfies it. Try installing: {suggestion:?}")]
    MissingRequired {
        /// Bundle name.
        bundle: String,
        /// Missing category.
        category: String,
        /// Suggested connectors to install.
        suggestion: Vec<String>,
    },

    /// Bundle is in degraded state.
    #[error("bundle '{0}' is degraded: some optional connectors are missing")]
    Degraded(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_bundle(dir: &Path, name: &str, required: &[&str], optional: &[&str]) {
        let bundle_dir = dir.join(name);
        std::fs::create_dir_all(&bundle_dir).unwrap();

        let def = serde_json::json!({
            "name": name,
            "version": "1.0.0",
            "description": format!("{} bundle", name),
            "required_categories": required,
            "optional_categories": optional,
            "system_prompt": format!("You are a {} assistant.", name),
        });

        std::fs::write(
            bundle_dir.join("plugin.json"),
            serde_json::to_string_pretty(&def).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn test_bundle_loader_discover() {
        let tmp = TempDir::new().unwrap();
        create_test_bundle(
            tmp.path(),
            "code-assistance",
            &["~~file-system", "~~code-runner"],
            &["~~web-browser"],
        );
        create_test_bundle(
            tmp.path(),
            "office-suite",
            &["~~presentation", "~~spreadsheet"],
            &[],
        );

        let bundles = BundleLoader::discover(&[tmp.path().to_path_buf()]);
        assert_eq!(bundles.len(), 2);

        let names: Vec<&str> = bundles.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"code-assistance"));
        assert!(names.contains(&"office-suite"));
    }

    #[test]
    fn test_bundle_manager_resolve() {
        let tmp = TempDir::new().unwrap();
        create_test_bundle(
            tmp.path(),
            "code-assistance",
            &["~~file-system", "~~code-runner"],
            &["~~web-browser"],
        );

        let mut manager = BundleManager::new(vec![tmp.path().to_path_buf()]);
        manager.discover();

        let mut resolver = CategoryResolver::new();
        resolver.register("~~file-system", "filesystem", 0);
        resolver.register("~~code-runner", "executor", 0);
        resolver.register("~~web-browser", "browser", 0);

        let active: HashSet<String> = ["filesystem".into(), "executor".into(), "browser".into()]
            .into_iter()
            .collect();

        let resolution = manager
            .resolve_bundle("code-assistance", &resolver, &active)
            .unwrap();
        assert!(resolution
            .enabled_namespaces
            .contains(&"filesystem".to_string()));
        assert!(resolution
            .enabled_namespaces
            .contains(&"executor".to_string()));
        assert!(resolution
            .enabled_namespaces
            .contains(&"browser".to_string()));
        assert!(resolution.warnings.is_empty());
        assert!(resolution.system_prompt.is_some());
    }

    #[test]
    fn test_bundle_resolve_missing_required() {
        let tmp = TempDir::new().unwrap();
        create_test_bundle(
            tmp.path(),
            "code-assistance",
            &["~~file-system", "~~code-runner"],
            &[],
        );

        let mut manager = BundleManager::new(vec![tmp.path().to_path_buf()]);
        manager.discover();

        let resolver = CategoryResolver::with_defaults();
        let active: HashSet<String> = HashSet::new(); // No active connectors

        let result = manager.resolve_bundle("code-assistance", &resolver, &active);
        assert!(matches!(result, Err(BundleError::MissingRequired { .. })));
    }

    #[test]
    fn test_bundle_resolve_optional_missing() {
        let tmp = TempDir::new().unwrap();
        create_test_bundle(
            tmp.path(),
            "code-assistance",
            &["~~file-system"],
            &["~~web-browser"],
        );

        let mut manager = BundleManager::new(vec![tmp.path().to_path_buf()]);
        manager.discover();

        let mut resolver = CategoryResolver::new();
        resolver.register("~~file-system", "filesystem", 0);

        let active: HashSet<String> = ["filesystem".into()].into_iter().collect();

        let resolution = manager
            .resolve_bundle("code-assistance", &resolver, &active)
            .unwrap();
        assert!(!resolution.warnings.is_empty());
        assert!(resolution.warnings[0].contains("~~web-browser"));
    }

    #[test]
    fn test_bundle_resolve_multiple_merge() {
        let tmp = TempDir::new().unwrap();
        create_test_bundle(
            tmp.path(),
            "code-assistance",
            &["~~file-system", "~~code-runner"],
            &[],
        );
        create_test_bundle(
            tmp.path(),
            "data-science",
            &["~~file-system", "~~spreadsheet"],
            &[],
        );

        let mut manager = BundleManager::new(vec![tmp.path().to_path_buf()]);
        manager.discover();

        let mut resolver = CategoryResolver::new();
        resolver.register("~~file-system", "filesystem", 0);
        resolver.register("~~code-runner", "executor", 0);
        resolver.register("~~spreadsheet", "xlsx", 0);

        let active: HashSet<String> = ["filesystem".into(), "executor".into(), "xlsx".into()]
            .into_iter()
            .collect();

        let merged = manager
            .resolve_bundles(
                &["code-assistance".into(), "data-science".into()],
                &resolver,
                &active,
            )
            .unwrap();

        // Should have deduplicated namespaces
        assert!(merged
            .enabled_namespaces
            .contains(&"filesystem".to_string()));
        assert!(merged.enabled_namespaces.contains(&"executor".to_string()));
        assert!(merged.enabled_namespaces.contains(&"xlsx".to_string()));

        // System prompts should be merged
        assert!(merged.system_prompt.is_some());
        assert!(merged.system_prompt.as_ref().unwrap().contains("---"));
    }

    #[test]
    fn test_bundle_not_found() {
        let tmp = TempDir::new().unwrap();
        let manager = BundleManager::new(vec![tmp.path().to_path_buf()]);
        let resolver = CategoryResolver::new();
        let active = HashSet::new();

        let result = manager.resolve_bundle("nonexistent", &resolver, &active);
        assert!(matches!(result, Err(BundleError::NotFound(_))));
    }

    #[test]
    fn test_bundle_prompt_from_file() {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("custom");
        std::fs::create_dir_all(&bundle_dir).unwrap();

        // plugin.json without inline system_prompt
        let def = serde_json::json!({
            "name": "custom",
            "version": "1.0.0",
            "description": "Custom bundle",
            "required_categories": [],
        });
        std::fs::write(
            bundle_dir.join("plugin.json"),
            serde_json::to_string_pretty(&def).unwrap(),
        )
        .unwrap();

        // PROMPT.md file
        std::fs::write(
            bundle_dir.join("PROMPT.md"),
            "You are a custom assistant.\n\nBe helpful.",
        )
        .unwrap();

        let bundles = BundleLoader::discover(&[tmp.path().to_path_buf()]);
        assert_eq!(bundles.len(), 1);
        assert!(bundles[0].system_prompt.is_some());
        assert!(bundles[0]
            .system_prompt
            .as_ref()
            .unwrap()
            .contains("custom assistant"));
    }

    #[test]
    fn test_bundle_mcp_json_parsing() {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("productivity");
        std::fs::create_dir_all(&bundle_dir).unwrap();

        let def = serde_json::json!({
            "name": "productivity",
            "version": "1.0.0",
            "description": "Productivity bundle",
            "required_categories": [],
        });
        std::fs::write(
            bundle_dir.join("plugin.json"),
            serde_json::to_string_pretty(&def).unwrap(),
        )
        .unwrap();

        let mcp = serde_json::json!({
            "mcpServers": {
                "slack": { "type": "http", "url": "https://mcp.slack.com/mcp" },
                "notion": { "type": "http", "url": "https://mcp.notion.com/mcp", "auth_token": "test-token" }
            }
        });
        std::fs::write(
            bundle_dir.join(".mcp.json"),
            serde_json::to_string_pretty(&mcp).unwrap(),
        )
        .unwrap();

        let bundles = BundleLoader::discover(&[tmp.path().to_path_buf()]);
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].mcp_servers.len(), 2);

        let mut servers: Vec<_> = bundles[0]
            .mcp_servers
            .iter()
            .map(|s| s.name.clone())
            .collect();
        servers.sort();
        assert_eq!(servers, vec!["notion", "slack"]);

        let notion = bundles[0]
            .mcp_servers
            .iter()
            .find(|s| s.name == "notion")
            .unwrap();
        assert_eq!(notion.auth_token.as_deref(), Some("test-token"));
    }

    #[test]
    fn test_bundle_mcp_json_skips_empty_url() {
        let tmp = TempDir::new().unwrap();
        let bundle_dir = tmp.path().join("test-bundle");
        std::fs::create_dir_all(&bundle_dir).unwrap();

        let def = serde_json::json!({
            "name": "test-bundle",
            "version": "1.0.0",
            "description": "Test",
            "required_categories": [],
        });
        std::fs::write(
            bundle_dir.join("plugin.json"),
            serde_json::to_string_pretty(&def).unwrap(),
        )
        .unwrap();

        let mcp = serde_json::json!({
            "mcpServers": {
                "valid": { "type": "http", "url": "https://example.com/mcp" },
                "empty": { "type": "http", "url": "" },
                "missing_url": { "type": "http" }
            }
        });
        std::fs::write(
            bundle_dir.join(".mcp.json"),
            serde_json::to_string_pretty(&mcp).unwrap(),
        )
        .unwrap();

        let bundles = BundleLoader::discover(&[tmp.path().to_path_buf()]);
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].mcp_servers.len(), 1);
        assert_eq!(bundles[0].mcp_servers[0].name, "valid");
    }

    #[test]
    fn test_bundle_config_merge() {
        let tmp = TempDir::new().unwrap();

        // Bundle 1 with timeout 60s
        let dir1 = tmp.path().join("b1");
        std::fs::create_dir_all(&dir1).unwrap();
        std::fs::write(
            dir1.join("plugin.json"),
            serde_json::to_string(&serde_json::json!({
                "name": "b1",
                "version": "1.0.0",
                "description": "B1",
                "required_categories": [],
                "config": { "timeout_ms": 60000 }
            }))
            .unwrap(),
        )
        .unwrap();

        // Bundle 2 with timeout 120s
        let dir2 = tmp.path().join("b2");
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(
            dir2.join("plugin.json"),
            serde_json::to_string(&serde_json::json!({
                "name": "b2",
                "version": "1.0.0",
                "description": "B2",
                "required_categories": [],
                "config": { "timeout_ms": 120000 }
            }))
            .unwrap(),
        )
        .unwrap();

        let mut manager = BundleManager::new(vec![tmp.path().to_path_buf()]);
        manager.discover();

        let resolver = CategoryResolver::new();
        let active = HashSet::new();

        let merged = manager
            .resolve_bundles(&["b1".into(), "b2".into()], &resolver, &active)
            .unwrap();
        assert_eq!(merged.config.timeout_ms, Some(120000)); // Max wins
    }
}
