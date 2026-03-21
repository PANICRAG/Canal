//! MCP Server Discovery
//!
//! This module provides automatic discovery and management of MCP servers.
//! It supports:
//! - Scanning configuration directories for server definitions
//! - Health checking and capability fetching
//! - Caching of server metadata and tool schemas

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::error::{Error, Result};

/// Server configuration that can be serialized/deserialized
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    pub transport: TransportConfig,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub namespace: String,
    #[serde(default = "default_timeout")]
    pub startup_timeout_secs: u64,
    #[serde(default)]
    pub auto_restart: bool,
}

fn default_enabled() -> bool {
    true
}
fn default_timeout() -> u64 {
    30
}

/// Transport configuration for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    Http {
        url: String,
    },
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            transport: TransportConfig::Http { url: String::new() },
            enabled: true,
            namespace: String::new(),
            startup_timeout_secs: 30,
            auto_restart: false,
        }
    }
}

/// Discovered MCP server metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredServer {
    /// Unique identifier for the server
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Server description
    pub description: String,

    /// Server version
    pub version: Option<String>,

    /// Server category (e.g., "filesystem", "database", "web")
    pub category: String,

    /// Configuration for connecting to the server
    pub config: ServerConfig,

    /// Tools provided by this server
    pub tools: Vec<DiscoveredTool>,

    /// Whether the server is currently healthy
    pub healthy: bool,

    /// Last health check timestamp
    pub last_check: Option<chrono::DateTime<chrono::Utc>>,

    /// Source of the server definition
    pub source: ServerSource,
}

/// Tool metadata discovered from server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredTool {
    /// Tool name
    pub name: String,

    /// Tool description
    pub description: String,

    /// Input schema (JSON Schema)
    pub input_schema: serde_json::Value,
}

/// Where the server definition came from
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerSource {
    /// From a configuration file
    ConfigFile(PathBuf),

    /// Built-in/bundled server
    Builtin,

    /// Manually added at runtime
    Manual,

    /// Discovered from registry
    Registry(String),
}

/// Server definition in config file
#[derive(Debug, Clone, Deserialize)]
pub struct ServerDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default = "default_category")]
    pub category: String,
    pub transport: TransportDefinition,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub enabled: bool,
}

fn default_category() -> String {
    "general".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TransportDefinition {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Http {
        url: String,
    },
}

/// MCP Server Discovery Service
pub struct McpDiscovery {
    /// Directory to scan for server configs
    config_dirs: Vec<PathBuf>,

    /// Cache of discovered servers
    servers: Arc<RwLock<HashMap<String, DiscoveredServer>>>,
}

impl McpDiscovery {
    /// Create a new discovery service
    pub fn new() -> Self {
        Self {
            config_dirs: vec![],
            servers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a configuration directory to scan
    pub fn with_config_dir(mut self, dir: impl AsRef<Path>) -> Self {
        self.config_dirs.push(dir.as_ref().to_path_buf());
        self
    }

    /// Scan all configured directories for server definitions
    pub async fn scan(&self) -> Result<Vec<DiscoveredServer>> {
        let mut discovered = Vec::new();

        for dir in &self.config_dirs {
            if !dir.exists() {
                debug!("Config directory does not exist: {:?}", dir);
                continue;
            }

            match self.scan_directory(dir).await {
                Ok(servers) => {
                    info!("Discovered {} servers in {:?}", servers.len(), dir);
                    discovered.extend(servers);
                }
                Err(e) => {
                    warn!("Failed to scan directory {:?}: {}", dir, e);
                }
            }
        }

        // Update cache
        let mut cache = self.servers.write().await;
        for server in &discovered {
            cache.insert(server.id.clone(), server.clone());
        }

        Ok(discovered)
    }

    /// Scan a single directory for server definitions
    async fn scan_directory(&self, dir: &Path) -> Result<Vec<DiscoveredServer>> {
        let mut servers = Vec::new();

        let mut entries = tokio::fs::read_dir(dir)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read directory: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| Error::Internal(format!("Failed to read entry: {}", e)))?
        {
            let path = entry.path();

            // Only process .yaml and .json files
            let ext = path.extension().and_then(|s| s.to_str());
            if !matches!(ext, Some("yaml") | Some("yml") | Some("json")) {
                continue;
            }

            match self.load_server_definition(&path).await {
                Ok(server) => {
                    if server.config.enabled {
                        servers.push(server);
                    }
                }
                Err(e) => {
                    warn!("Failed to load server definition {:?}: {}", path, e);
                }
            }
        }

        Ok(servers)
    }

    /// Load a server definition from a file
    async fn load_server_definition(&self, path: &Path) -> Result<DiscoveredServer> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read file: {}", e)))?;

        let ext = path.extension().and_then(|s| s.to_str());
        let def: ServerDefinition = match ext {
            Some("yaml") | Some("yml") => serde_yaml::from_str(&content)
                .map_err(|e| Error::Internal(format!("Failed to parse YAML: {}", e)))?,
            Some("json") => serde_json::from_str(&content)
                .map_err(|e| Error::Internal(format!("Failed to parse JSON: {}", e)))?,
            _ => return Err(Error::Internal("Unsupported file format".to_string())),
        };

        // Convert to ServerConfig
        let transport = match def.transport {
            TransportDefinition::Stdio { command, args } => TransportConfig::Stdio {
                command,
                args,
                env: def.env.clone(),
            },
            TransportDefinition::Http { url } => TransportConfig::Http { url },
        };

        let config = ServerConfig {
            name: def.name.clone(),
            transport,
            enabled: def.enabled,
            ..Default::default()
        };

        // Generate ID from filename
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&def.name)
            .to_string();

        Ok(DiscoveredServer {
            id,
            name: def.name,
            description: def.description,
            version: def.version,
            category: def.category,
            config,
            tools: vec![], // Will be populated on connect
            healthy: false,
            last_check: None,
            source: ServerSource::ConfigFile(path.to_path_buf()),
        })
    }

    /// Get all cached servers
    pub async fn list_servers(&self) -> Vec<DiscoveredServer> {
        let cache = self.servers.read().await;
        cache.values().cloned().collect()
    }

    /// Get a server by ID
    pub async fn get_server(&self, id: &str) -> Option<DiscoveredServer> {
        let cache = self.servers.read().await;
        cache.get(id).cloned()
    }

    /// Add a server manually
    pub async fn add_server(&self, server: DiscoveredServer) {
        let mut cache = self.servers.write().await;
        cache.insert(server.id.clone(), server);
    }

    /// Remove a server
    pub async fn remove_server(&self, id: &str) -> Option<DiscoveredServer> {
        let mut cache = self.servers.write().await;
        cache.remove(id)
    }

    /// Update server health status
    pub async fn update_health(&self, id: &str, healthy: bool, tools: Vec<DiscoveredTool>) {
        let mut cache = self.servers.write().await;
        if let Some(server) = cache.get_mut(id) {
            server.healthy = healthy;
            server.last_check = Some(chrono::Utc::now());
            if healthy && !tools.is_empty() {
                server.tools = tools;
            }
        }
    }

    /// Get servers by category
    pub async fn list_by_category(&self, category: &str) -> Vec<DiscoveredServer> {
        let cache = self.servers.read().await;
        cache
            .values()
            .filter(|s| s.category == category)
            .cloned()
            .collect()
    }

    /// Get healthy servers only
    pub async fn list_healthy(&self) -> Vec<DiscoveredServer> {
        let cache = self.servers.read().await;
        cache.values().filter(|s| s.healthy).cloned().collect()
    }

    /// Get all unique categories
    pub async fn list_categories(&self) -> Vec<String> {
        let cache = self.servers.read().await;
        let mut categories: Vec<_> = cache.values().map(|s| s.category.clone()).collect();
        categories.sort();
        categories.dedup();
        categories
    }
}

impl Default for McpDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to create a stdio server config
fn stdio_server(
    id: &str,
    name: &str,
    description: &str,
    category: &str,
    command: &str,
    args: Vec<&str>,
    env: Vec<(&str, &str)>,
    enabled: bool,
) -> DiscoveredServer {
    DiscoveredServer {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        version: Some("1.0.0".to_string()),
        category: category.to_string(),
        config: ServerConfig {
            name: name.to_string(),
            transport: TransportConfig::Stdio {
                command: command.to_string(),
                args: args.into_iter().map(|s| s.to_string()).collect(),
                env: env
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            },
            enabled,
            ..Default::default()
        },
        tools: vec![],
        healthy: false,
        last_check: None,
        source: ServerSource::Builtin,
    }
}

/// Pre-configured MCP servers that come bundled
pub fn builtin_servers() -> Vec<DiscoveredServer> {
    vec![
        stdio_server(
            "filesystem",
            "Filesystem",
            "Read, write, and manage files on the local filesystem",
            "filesystem",
            "npx",
            vec!["-y", "@anthropic/mcp-server-filesystem"],
            vec![],
            true,
        ),
        stdio_server(
            "memory",
            "Memory",
            "Persistent key-value memory storage",
            "storage",
            "npx",
            vec!["-y", "@anthropic/mcp-server-memory"],
            vec![],
            true,
        ),
        stdio_server(
            "fetch",
            "Web Fetch",
            "Fetch content from URLs with markdown conversion",
            "web",
            "npx",
            vec!["-y", "@anthropic/mcp-server-fetch"],
            vec![],
            true,
        ),
        stdio_server(
            "git",
            "Git",
            "Git repository operations",
            "development",
            "npx",
            vec!["-y", "@anthropic/mcp-server-git"],
            vec![],
            true,
        ),
        stdio_server(
            "github",
            "GitHub",
            "GitHub API operations (issues, PRs, repos)",
            "development",
            "npx",
            vec!["-y", "@modelcontextprotocol/server-github"],
            vec![("GITHUB_TOKEN", "${GITHUB_TOKEN}")],
            false, // Requires token
        ),
        stdio_server(
            "postgres",
            "PostgreSQL",
            "PostgreSQL database operations",
            "database",
            "npx",
            vec!["-y", "@modelcontextprotocol/server-postgres"],
            vec![("DATABASE_URL", "${DATABASE_URL}")],
            false, // Requires connection string
        ),
        stdio_server(
            "sqlite",
            "SQLite",
            "SQLite database operations",
            "database",
            "npx",
            vec!["-y", "@modelcontextprotocol/server-sqlite"],
            vec![],
            true,
        ),
        stdio_server(
            "brave-search",
            "Brave Search",
            "Web search using Brave Search API",
            "web",
            "npx",
            vec!["-y", "@modelcontextprotocol/server-brave-search"],
            vec![("BRAVE_API_KEY", "${BRAVE_API_KEY}")],
            false, // Requires API key
        ),
        stdio_server(
            "puppeteer",
            "Puppeteer",
            "Browser automation with Puppeteer",
            "web",
            "npx",
            vec!["-y", "@modelcontextprotocol/server-puppeteer"],
            vec![],
            true,
        ),
        stdio_server(
            "slack",
            "Slack",
            "Slack workspace integration",
            "communication",
            "npx",
            vec!["-y", "@modelcontextprotocol/server-slack"],
            vec![("SLACK_TOKEN", "${SLACK_TOKEN}")],
            false, // Requires token
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_servers() {
        let servers = builtin_servers();
        assert!(!servers.is_empty());
        assert_eq!(servers.len(), 10);

        // Check that we have expected categories
        let categories: Vec<_> = servers.iter().map(|s| &s.category).collect();
        assert!(categories.contains(&&"filesystem".to_string()));
        assert!(categories.contains(&&"database".to_string()));
        assert!(categories.contains(&&"web".to_string()));
    }

    #[tokio::test]
    async fn test_discovery_add_remove() {
        let discovery = McpDiscovery::new();

        let server = DiscoveredServer {
            id: "test".to_string(),
            name: "Test Server".to_string(),
            description: "A test server".to_string(),
            version: Some("1.0.0".to_string()),
            category: "test".to_string(),
            config: ServerConfig::default(),
            tools: vec![],
            healthy: false,
            last_check: None,
            source: ServerSource::Manual,
        };

        discovery.add_server(server).await;

        let fetched = discovery.get_server("test").await;
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().name, "Test Server");

        let removed = discovery.remove_server("test").await;
        assert!(removed.is_some());

        let fetched = discovery.get_server("test").await;
        assert!(fetched.is_none());
    }
}
