//! Plugin loader — format detection, parsing, and directory scanning.
//!
//! Supports two formats:
//! - **Claude Skills**: `SKILL.md` + optional `scripts/` + reference `.md` files
//! - **Cowork**: `.claude-plugin/plugin.json` + `.mcp.json` + `commands/` + `skills/`

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::skills::definition::Skill;
use crate::skills::parser::SkillParser;

use serde::Deserialize;

use super::error::{PluginError, PluginResult};
use super::manifest::{
    CoworkPluginJson, McpServerEntry, PluginFormat, PluginManifest, PluginMcpConfig,
};

/// A loaded plugin with all parsed data.
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    /// Plugin manifest (name, version, format, etc.).
    pub manifest: PluginManifest,

    /// Parsed skill definitions.
    pub skills: Vec<Skill>,

    /// Reference document paths: (display_name, file_path).
    pub reference_paths: Vec<(String, PathBuf)>,

    /// Path to the scripts directory (if present).
    pub scripts_dir: Option<PathBuf>,

    /// MCP server configuration (Cowork format only).
    pub mcp_config: Option<PluginMcpConfig>,

    /// Root directory of the plugin source.
    pub source_path: PathBuf,
}

/// Plugin loader for format detection and parsing.
pub struct PluginLoader;

impl PluginLoader {
    /// Detect the plugin format of a directory.
    ///
    /// Priority: Cowork > Claude Skills. If both markers exist, Cowork wins.
    ///
    /// # Errors
    /// - `NotFound` if path doesn't exist or is not a directory.
    /// - `UnknownFormat` if no format markers are found.
    pub fn detect_format(path: &Path) -> PluginResult<PluginFormat> {
        if !path.exists() || !path.is_dir() {
            return Err(PluginError::NotFound(path.display().to_string()));
        }

        let has_cowork = path.join(".claude-plugin/plugin.json").exists();
        let has_skill_md = path.join("SKILL.md").exists();

        match (has_cowork, has_skill_md) {
            (true, _) => Ok(PluginFormat::Cowork),
            (false, true) => Ok(PluginFormat::ClaudeSkills),
            (false, false) => Err(PluginError::UnknownFormat {
                path: path.display().to_string(),
                hint: "Expected .claude-plugin/plugin.json or SKILL.md".to_string(),
            }),
        }
    }

    /// Parse a Claude Skills format plugin.
    ///
    /// Reads `SKILL.md` using `SkillParser`, discovers reference `.md` files
    /// and scripts directory.
    pub fn parse_claude_skills(path: &Path) -> PluginResult<LoadedPlugin> {
        let skill_md_path = path.join("SKILL.md");
        if !skill_md_path.exists() {
            return Err(PluginError::InvalidManifest(format!(
                "SKILL.md not found in {}",
                path.display()
            )));
        }

        // Parse SKILL.md using existing SkillParser
        let skill = SkillParser::parse_file(&skill_md_path)
            .map_err(|e| PluginError::Parse(e.to_string()))?;

        // Extract plugin name from skill or directory name
        let plugin_name = if !skill.name.is_empty() {
            skill.name.clone()
        } else {
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        };

        // Extract license from skill metadata custom fields
        let license = skill
            .metadata
            .custom
            .get("license")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Build manifest from SKILL.md frontmatter
        let manifest = PluginManifest {
            name: plugin_name.clone(),
            version: skill.metadata.version.clone(),
            description: skill.description.clone(),
            author: skill.metadata.author.clone(),
            license,
            homepage: None,
            format: PluginFormat::ClaudeSkills,
        };

        // Discover reference .md files (excluding SKILL.md itself)
        let reference_paths = Self::discover_references(path, &skill_md_path);

        // Check for scripts directory
        let scripts_dir = {
            let sd = path.join("scripts");
            if sd.is_dir() {
                Some(sd)
            } else {
                None
            }
        };

        Ok(LoadedPlugin {
            manifest,
            skills: vec![skill],
            reference_paths,
            scripts_dir,
            mcp_config: None,
            source_path: path.to_path_buf(),
        })
    }

    /// Parse a Cowork format plugin.
    ///
    /// Reads `.claude-plugin/plugin.json`, optional `.mcp.json`,
    /// `commands/*.md`, and `skills/*/SKILL.md`.
    pub fn parse_cowork(path: &Path) -> PluginResult<LoadedPlugin> {
        let plugin_json_path = path.join(".claude-plugin/plugin.json");
        if !plugin_json_path.exists() {
            return Err(PluginError::InvalidManifest(format!(
                ".claude-plugin/plugin.json not found in {}",
                path.display()
            )));
        }

        // Parse plugin.json
        let plugin_json_content =
            std::fs::read_to_string(&plugin_json_path).map_err(|e| PluginError::Io(e))?;
        let plugin_json: CoworkPluginJson = serde_json::from_str(&plugin_json_content)
            .map_err(|e| PluginError::Parse(format!("plugin.json: {}", e)))?;

        let dir_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let manifest = PluginManifest {
            name: plugin_json.name.unwrap_or_else(|| dir_name.to_string()),
            version: plugin_json.version.unwrap_or_else(|| "1.0.0".to_string()),
            description: plugin_json
                .description
                .unwrap_or_else(|| format!("{} plugin", dir_name)),
            author: plugin_json.author,
            license: None,
            homepage: plugin_json.homepage,
            format: PluginFormat::Cowork,
        };

        // Parse .mcp.json if present
        let mcp_config = Self::parse_mcp_config(path);

        // Parse commands/*.md as skills
        let mut skills = Vec::new();

        let commands_dir = path.join("commands");
        if commands_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&commands_dir) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if entry_path.extension().and_then(|e| e.to_str()) == Some("md") {
                        if let Ok(skill) = SkillParser::parse_file(&entry_path) {
                            skills.push(skill);
                        }
                    }
                }
            }
        }

        // Parse skills/*/SKILL.md
        let skills_dir = path.join("skills");
        if skills_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&skills_dir) {
                for entry in entries.flatten() {
                    let skill_md = entry.path().join("SKILL.md");
                    if skill_md.exists() {
                        if let Ok(skill) = SkillParser::parse_file(&skill_md) {
                            skills.push(skill);
                        }
                    }
                }
            }
        }

        // Discover reference files
        let reference_paths = Self::discover_cowork_references(path);

        // Check for scripts directory
        let scripts_dir = {
            let sd = path.join("scripts");
            if sd.is_dir() {
                Some(sd)
            } else {
                None
            }
        };

        Ok(LoadedPlugin {
            manifest,
            skills,
            reference_paths,
            scripts_dir,
            mcp_config,
            source_path: path.to_path_buf(),
        })
    }

    /// Scan directories for plugins.
    ///
    /// Iterates subdirectories, detects format, and parses each plugin.
    /// Unrecognized directories are skipped with a warning log.
    pub fn discover(dirs: &[PathBuf]) -> Vec<LoadedPlugin> {
        let mut plugins = Vec::new();

        for dir in dirs {
            if !dir.is_dir() {
                tracing::warn!("Plugin catalog directory does not exist: {}", dir.display());
                continue;
            }

            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) => {
                    tracing::warn!("Failed to read plugin directory {}: {}", dir.display(), e);
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

                match Self::detect_format(&entry_path) {
                    Ok(PluginFormat::ClaudeSkills) => {
                        match Self::parse_claude_skills(&entry_path) {
                            Ok(plugin) => {
                                tracing::info!(
                                    "Loaded Claude Skills plugin: {} ({})",
                                    plugin.manifest.name,
                                    entry_path.display()
                                );
                                plugins.push(plugin);
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to parse Claude Skills plugin at {}: {}",
                                    entry_path.display(),
                                    e
                                );
                            }
                        }
                    }
                    Ok(PluginFormat::Cowork) => match Self::parse_cowork(&entry_path) {
                        Ok(plugin) => {
                            tracing::info!(
                                "Loaded Cowork plugin: {} ({})",
                                plugin.manifest.name,
                                entry_path.display()
                            );
                            plugins.push(plugin);
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to parse Cowork plugin at {}: {}",
                                entry_path.display(),
                                e
                            );
                        }
                    },
                    Err(_) => {
                        tracing::debug!(
                            "Skipping unrecognized directory: {}",
                            entry_path.display()
                        );
                    }
                }
            }
        }

        plugins
    }

    // --- Private helpers ---

    /// Discover reference .md files alongside SKILL.md.
    fn discover_references(dir: &Path, skill_md: &Path) -> Vec<(String, PathBuf)> {
        let mut refs = Vec::new();

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path == *skill_md {
                    continue; // Skip SKILL.md itself
                }
                if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown.md")
                        .to_string();
                    refs.push((name, path));
                }
            }
        }

        refs.sort_by(|a, b| a.0.cmp(&b.0));
        refs
    }

    /// Discover reference files in Cowork skills directories.
    fn discover_cowork_references(dir: &Path) -> Vec<(String, PathBuf)> {
        let mut refs = Vec::new();

        // Check skills/*/SKILL.md sibling .md files
        let skills_dir = dir.join("skills");
        if skills_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&skills_dir) {
                for entry in entries.flatten() {
                    let skill_dir = entry.path();
                    if skill_dir.is_dir() {
                        if let Ok(files) = std::fs::read_dir(&skill_dir) {
                            for file in files.flatten() {
                                let file_path = file.path();
                                if file_path.extension().and_then(|e| e.to_str()) == Some("md")
                                    && file_path.file_name().and_then(|n| n.to_str())
                                        != Some("SKILL.md")
                                {
                                    let name = file_path
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("unknown.md")
                                        .to_string();
                                    refs.push((name, file_path));
                                }
                            }
                        }
                    }
                }
            }
        }

        refs.sort_by(|a, b| a.0.cmp(&b.0));
        refs
    }

    /// Parse .mcp.json if present.
    fn parse_mcp_config(dir: &Path) -> Option<PluginMcpConfig> {
        let mcp_path = dir.join(".mcp.json");
        if !mcp_path.exists() {
            return None;
        }

        let content = std::fs::read_to_string(&mcp_path).ok()?;

        // .mcp.json has a "mcpServers" wrapper
        #[derive(Deserialize)]
        struct McpJsonWrapper {
            #[serde(rename = "mcpServers", default)]
            mcp_servers: HashMap<String, McpServerEntry>,
        }

        // Try wrapped format first, then flat format
        if let Ok(wrapper) = serde_json::from_str::<McpJsonWrapper>(&content) {
            if !wrapper.mcp_servers.is_empty() {
                return Some(PluginMcpConfig {
                    servers: wrapper.mcp_servers,
                });
            }
        }

        // Try flat format (servers at top level)
        if let Ok(config) = serde_json::from_str::<PluginMcpConfig>(&content) {
            if !config.servers.is_empty() {
                return Some(config);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_temp_claude_skills(dir: &Path, name: &str) -> PathBuf {
        let plugin_dir = dir.join(name);
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(
            plugin_dir.join("SKILL.md"),
            format!(
                "---\nname: {}\ndescription: {} processing plugin\n---\n\n# {} Guide\n\nContent here.",
                name, name, name
            ),
        )
        .unwrap();
        plugin_dir
    }

    fn create_temp_cowork(dir: &Path, name: &str) -> PathBuf {
        let plugin_dir = dir.join(name);
        let claude_dir = plugin_dir.join(".claude-plugin");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(
            claude_dir.join("plugin.json"),
            format!(
                r#"{{"name": "{}", "description": "{} enterprise plugin", "version": "2.0.0"}}"#,
                name, name
            ),
        )
        .unwrap();
        plugin_dir
    }

    #[test]
    fn test_detect_format_claude_skills() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = create_temp_claude_skills(tmp.path(), "test-plugin");

        let format = PluginLoader::detect_format(&plugin_dir).unwrap();
        assert_eq!(format, PluginFormat::ClaudeSkills);
    }

    #[test]
    fn test_detect_format_cowork() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = create_temp_cowork(tmp.path(), "test-cowork");

        let format = PluginLoader::detect_format(&plugin_dir).unwrap();
        assert_eq!(format, PluginFormat::Cowork);
    }

    #[test]
    fn test_detect_format_both_files() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = create_temp_cowork(tmp.path(), "hybrid");
        // Also add SKILL.md
        fs::write(
            plugin_dir.join("SKILL.md"),
            "---\nname: hybrid\n---\nContent",
        )
        .unwrap();

        // Cowork should take priority
        let format = PluginLoader::detect_format(&plugin_dir).unwrap();
        assert_eq!(format, PluginFormat::Cowork);
    }

    #[test]
    fn test_detect_format_not_found() {
        let result = PluginLoader::detect_format(Path::new("/nonexistent/path"));
        assert!(matches!(result.unwrap_err(), PluginError::NotFound(_)));
    }

    #[test]
    fn test_detect_format_no_marker() {
        let tmp = TempDir::new().unwrap();
        let empty_dir = tmp.path().join("empty");
        fs::create_dir_all(&empty_dir).unwrap();

        let result = PluginLoader::detect_format(&empty_dir);
        match result.unwrap_err() {
            PluginError::UnknownFormat { path, hint } => {
                assert!(path.contains("empty"));
                assert!(hint.contains("SKILL.md"));
            }
            other => panic!("Expected UnknownFormat, got: {:?}", other),
        }
    }

    #[test]
    fn test_detect_format_file_not_dir() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("not-a-dir.txt");
        fs::write(&file_path, "hello").unwrap();

        let result = PluginLoader::detect_format(&file_path);
        assert!(matches!(result.unwrap_err(), PluginError::NotFound(_)));
    }

    #[test]
    fn test_parse_claude_skills_basic() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = create_temp_claude_skills(tmp.path(), "pdf");

        let plugin = PluginLoader::parse_claude_skills(&plugin_dir).unwrap();
        assert_eq!(plugin.manifest.name, "pdf");
        assert_eq!(plugin.manifest.format, PluginFormat::ClaudeSkills);
        assert_eq!(plugin.skills.len(), 1);
        assert_eq!(plugin.skills[0].name, "pdf");
        assert!(plugin.mcp_config.is_none());
    }

    #[test]
    fn test_parse_claude_skills_references() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = create_temp_claude_skills(tmp.path(), "pdf");

        // Add reference files
        fs::write(plugin_dir.join("REFERENCE.md"), "# Reference").unwrap();
        fs::write(plugin_dir.join("FORMS.md"), "# Forms Guide").unwrap();

        let plugin = PluginLoader::parse_claude_skills(&plugin_dir).unwrap();
        assert_eq!(plugin.reference_paths.len(), 2);
        let ref_names: Vec<&str> = plugin
            .reference_paths
            .iter()
            .map(|r| r.0.as_str())
            .collect();
        assert!(ref_names.contains(&"FORMS.md"));
        assert!(ref_names.contains(&"REFERENCE.md"));
    }

    #[test]
    fn test_discover_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let plugins = PluginLoader::discover(&[tmp.path().to_path_buf()]);
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_discover_multiple_plugins() {
        let tmp = TempDir::new().unwrap();
        create_temp_claude_skills(tmp.path(), "pdf");
        create_temp_claude_skills(tmp.path(), "docx");

        let plugins = PluginLoader::discover(&[tmp.path().to_path_buf()]);
        assert_eq!(plugins.len(), 2);

        let names: Vec<&str> = plugins.iter().map(|p| p.manifest.name.as_str()).collect();
        assert!(names.contains(&"pdf"));
        assert!(names.contains(&"docx"));
    }
}
