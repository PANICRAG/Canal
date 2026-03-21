//! Plugin manifest types.
//!
//! Defines the core data structures for plugin metadata, format detection,
//! and MCP server configuration.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Plugin format type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginFormat {
    /// Simple format: `SKILL.md` + optional scripts/references.
    ClaudeSkills,
    /// Complex format: `.claude-plugin/plugin.json` + `.mcp.json` + commands + skills.
    Cowork,
}

impl std::fmt::Display for PluginFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginFormat::ClaudeSkills => write!(f, "ClaudeSkills"),
            PluginFormat::Cowork => write!(f, "Cowork"),
        }
    }
}

/// Plugin manifest containing core metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin unique name (directory name).
    pub name: String,

    /// Semantic version string (e.g., "1.0.0").
    pub version: String,

    /// Human-readable description.
    pub description: String,

    /// Plugin author.
    #[serde(default)]
    pub author: Option<String>,

    /// License identifier or file reference.
    #[serde(default)]
    pub license: Option<String>,

    /// Homepage URL.
    #[serde(default)]
    pub homepage: Option<String>,

    /// Detected plugin format.
    pub format: PluginFormat,
}

/// MCP server configuration from `.mcp.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMcpConfig {
    /// Named MCP servers provided by this plugin.
    #[serde(flatten)]
    pub servers: HashMap<String, McpServerEntry>,
}

/// A single MCP server entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    /// Command to run the MCP server.
    pub command: String,

    /// Arguments for the command.
    #[serde(default)]
    pub args: Vec<String>,

    /// Environment variables for the server process.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Cowork plugin.json manifest structure.
#[derive(Debug, Clone, Deserialize)]
pub struct CoworkPluginJson {
    /// Plugin display name.
    #[serde(default)]
    pub name: Option<String>,

    /// Plugin description.
    #[serde(default)]
    pub description: Option<String>,

    /// Plugin version.
    #[serde(default)]
    pub version: Option<String>,

    /// Plugin author.
    #[serde(default)]
    pub author: Option<String>,

    /// Homepage URL.
    #[serde(default)]
    pub homepage: Option<String>,
}

/// API response type for catalog entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Plugin name.
    pub name: String,

    /// Plugin description.
    pub description: String,

    /// Version string.
    pub version: String,

    /// Format type as string.
    pub format: String,

    /// Author name.
    pub author: Option<String>,

    /// Number of skills in this plugin.
    pub skills_count: usize,

    /// Reference document names.
    pub references: Vec<String>,

    /// Whether the plugin has Python scripts.
    pub has_scripts: bool,

    /// Whether the plugin provides MCP servers.
    pub has_mcp: bool,

    /// Whether the current user has installed this plugin.
    pub installed: bool,
}

/// Unified API response wrapper for plugin endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginApiResponse<T> {
    /// Whether the operation succeeded.
    pub success: bool,

    /// Response data (None on error).
    pub data: Option<T>,

    /// Error message (None on success).
    pub error: Option<String>,
}

impl<T> PluginApiResponse<T> {
    /// Create a success response with data.
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    /// Create a success response without data.
    pub fn ok_empty() -> Self
    where
        T: Default,
    {
        Self {
            success: true,
            data: None,
            error: None,
        }
    }

    /// Create an error response.
    pub fn err(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_serialize_roundtrip() {
        let manifest = PluginManifest {
            name: "pdf".to_string(),
            version: "1.0.0".to_string(),
            description: "PDF processing plugin".to_string(),
            author: Some("Canal".to_string()),
            license: Some("Proprietary".to_string()),
            homepage: None,
            format: PluginFormat::ClaudeSkills,
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: PluginManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, "pdf");
        assert_eq!(deserialized.version, "1.0.0");
        assert_eq!(deserialized.format, PluginFormat::ClaudeSkills);
        assert_eq!(deserialized.author, Some("Canal".to_string()));
    }

    #[test]
    fn test_manifest_claude_skills_format() {
        let manifest = PluginManifest {
            name: "docx".to_string(),
            version: "1.0.0".to_string(),
            description: "Word doc processing".to_string(),
            author: None,
            license: None,
            homepage: None,
            format: PluginFormat::ClaudeSkills,
        };
        assert_eq!(manifest.format, PluginFormat::ClaudeSkills);
        assert_eq!(manifest.format.to_string(), "ClaudeSkills");
    }

    #[test]
    fn test_manifest_cowork_format() {
        let manifest = PluginManifest {
            name: "productivity".to_string(),
            version: "2.0.0".to_string(),
            description: "Enterprise productivity".to_string(),
            author: Some("Cowork".to_string()),
            license: None,
            homepage: Some("https://cowork.example.com".to_string()),
            format: PluginFormat::Cowork,
        };
        assert_eq!(manifest.format, PluginFormat::Cowork);
        assert!(manifest.homepage.is_some());
    }

    #[test]
    fn test_mcp_config_deserialize() {
        let json = r#"{
            "task-manager": {
                "command": "npx",
                "args": ["-y", "@task-manager/server"],
                "env": {"API_KEY": "test"}
            }
        }"#;

        let config: PluginMcpConfig = serde_json::from_str(json).unwrap();
        assert!(config.servers.contains_key("task-manager"));

        let server = &config.servers["task-manager"];
        assert_eq!(server.command, "npx");
        assert_eq!(server.args, vec!["-y", "@task-manager/server"]);
        assert_eq!(server.env.get("API_KEY"), Some(&"test".to_string()));
    }

    #[test]
    fn test_plugin_api_response_ok() {
        let resp = PluginApiResponse::ok(vec!["pdf", "docx"]);
        assert!(resp.success);
        assert!(resp.data.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_plugin_api_response_err() {
        let resp: PluginApiResponse<String> = PluginApiResponse::err("not found");
        assert!(!resp.success);
        assert!(resp.data.is_none());
        assert_eq!(resp.error, Some("not found".to_string()));
    }
}
