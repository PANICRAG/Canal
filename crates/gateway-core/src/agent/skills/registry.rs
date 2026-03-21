//! Skill Registry - Manage and search skills
//!
//! Provides a registry for storing, retrieving, and searching skills.
//! Uses `DashMap` for thread-safe concurrent access from shared `AppState`.

use super::builtin::get_builtin_skills;
use super::definition::Skill;
use super::parser::{SkillParseError, SkillParser};
use dashmap::DashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during registry operations
#[derive(Error, Debug)]
pub enum SkillRegistryError {
    #[error("Skill not found: {0}")]
    NotFound(String),

    #[error("Parse error: {0}")]
    Parse(#[from] SkillParseError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Duplicate skill: {0}")]
    Duplicate(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),
}

/// Result type for registry operations
pub type SkillRegistryResult<T> = Result<T, SkillRegistryError>;

/// Registry for managing skills
///
/// Thread-safe: uses `DashMap` internally so all read/write methods
/// can be called from any thread through a shared `&self` reference.
pub struct SkillRegistry {
    /// Skills indexed by name (thread-safe concurrent map)
    skills: DashMap<String, Skill>,

    /// Search paths for skill discovery
    search_paths: Vec<PathBuf>,

    /// Whether to allow overwriting existing skills
    allow_overwrite: bool,
}

impl SkillRegistry {
    /// Create a new empty skill registry
    pub fn new() -> Self {
        Self {
            skills: DashMap::new(),
            search_paths: Vec::new(),
            allow_overwrite: true,
        }
    }

    /// Create a new skill registry with builtin skills loaded
    pub fn with_builtins() -> Self {
        let registry = Self::new();
        registry.load_builtins();
        registry
    }

    /// Create a builder for configuring the registry
    pub fn builder() -> SkillRegistryBuilder {
        SkillRegistryBuilder::new()
    }

    /// Load builtin skills into the registry
    pub fn load_builtins(&self) {
        for skill in get_builtin_skills() {
            // Builtins can be overwritten by user skills
            self.skills.insert(skill.name.clone(), skill);
        }
    }

    /// Add a search path for skill discovery
    ///
    /// NOTE: This method still requires `&mut self` because `search_paths`
    /// is a plain `Vec` mutated only during registry construction.
    pub fn add_search_path(&mut self, path: impl Into<PathBuf>) {
        self.search_paths.push(path.into());
    }

    /// Load skills from a directory
    ///
    /// Returns the number of skills loaded
    pub fn load_from_directory(&self, path: &Path) -> SkillRegistryResult<usize> {
        if !path.exists() {
            return Ok(0);
        }

        if !path.is_dir() {
            return Err(SkillRegistryError::InvalidPath(format!(
                "Not a directory: {}",
                path.display()
            )));
        }

        let mut count = 0;

        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let file_path = entry.path();

            // Only process .md files
            if file_path.extension().map_or(false, |ext| ext == "md") {
                match SkillParser::parse_file(&file_path) {
                    Ok(skill) => {
                        if let Err(e) = SkillParser::validate(&skill) {
                            tracing::warn!("Invalid skill {}: {}", file_path.display(), e);
                            continue;
                        }
                        self.register(skill)?;
                        count += 1;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse skill {}: {}", file_path.display(), e);
                    }
                }
            }
        }

        Ok(count)
    }

    /// Load skills from all search paths
    pub fn load_from_search_paths(&self) -> SkillRegistryResult<usize> {
        let paths = self.search_paths.clone();
        let mut total = 0;

        for path in paths {
            total += self.load_from_directory(&path)?;
        }

        Ok(total)
    }

    /// Register a skill
    pub fn register(&self, skill: Skill) -> SkillRegistryResult<()> {
        if !self.allow_overwrite && self.skills.contains_key(&skill.name) {
            return Err(SkillRegistryError::Duplicate(skill.name));
        }

        self.skills.insert(skill.name.clone(), skill);
        Ok(())
    }

    /// Get a skill by name (returns a clone for thread safety)
    pub fn get(&self, name: &str) -> Option<Skill> {
        self.skills
            .get(name)
            .map(|r| r.value().clone())
            .or_else(|| {
                // Try with namespace prefix removed
                if let Some(pos) = name.find(':') {
                    let short_name = &name[pos + 1..];
                    self.skills.get(short_name).map(|r| r.value().clone())
                } else {
                    None
                }
            })
    }

    /// Check if a skill exists
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Remove a skill by name
    pub fn remove(&self, name: &str) -> Option<Skill> {
        self.skills.remove(name).map(|(_, v)| v)
    }

    /// List all skills (returns owned clones for thread safety)
    pub fn list(&self) -> Vec<Skill> {
        let mut skills: Vec<_> = self.skills.iter().map(|r| r.value().clone()).collect();
        // Sort by priority (descending) then by name
        skills.sort_by(|a, b| {
            b.metadata
                .priority
                .cmp(&a.metadata.priority)
                .then_with(|| a.name.cmp(&b.name))
        });
        skills
    }

    /// List visible skills (non-hidden)
    pub fn list_visible(&self) -> Vec<Skill> {
        self.list().into_iter().filter(|s| !s.is_hidden()).collect()
    }

    /// List builtin skills
    pub fn list_builtins(&self) -> Vec<Skill> {
        self.list().into_iter().filter(|s| s.is_builtin()).collect()
    }

    /// List user-defined skills (non-builtin)
    pub fn list_user_defined(&self) -> Vec<Skill> {
        self.list()
            .into_iter()
            .filter(|s| !s.is_builtin())
            .collect()
    }

    /// Search for skills matching a query
    pub fn search(&self, query: &str) -> Vec<Skill> {
        let query_lower = query.to_lowercase();
        let mut results: Vec<_> = self
            .skills
            .iter()
            .filter(|entry| {
                let skill = entry.value();
                skill.name.to_lowercase().contains(&query_lower)
                    || skill.description.to_lowercase().contains(&query_lower)
                    || skill
                        .metadata
                        .tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&query_lower))
            })
            .map(|entry| entry.value().clone())
            .collect();

        // Sort by relevance (name match > description match > tag match)
        results.sort_by(|a, b| {
            let a_name_match = a.name.to_lowercase().contains(&query_lower);
            let b_name_match = b.name.to_lowercase().contains(&query_lower);

            match (a_name_match, b_name_match) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.cmp(&b.name),
            }
        });

        results
    }

    /// Find skills by tag
    pub fn find_by_tag(&self, tag: &str) -> Vec<Skill> {
        let tag_lower = tag.to_lowercase();
        self.skills
            .iter()
            .filter(|entry| {
                entry
                    .value()
                    .metadata
                    .tags
                    .iter()
                    .any(|t| t.to_lowercase() == tag_lower)
            })
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Find skills by namespace
    pub fn find_by_namespace(&self, namespace: &str) -> Vec<Skill> {
        self.skills
            .iter()
            .filter(|entry| {
                entry
                    .value()
                    .metadata
                    .namespace
                    .as_ref()
                    .map_or(false, |ns| ns == namespace)
            })
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get skill count
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Get all skill names
    pub fn names(&self) -> Vec<String> {
        self.skills.iter().map(|r| r.key().clone()).collect()
    }

    /// Clear all skills
    pub fn clear(&self) {
        self.skills.clear();
    }

    /// Set whether to allow overwriting existing skills
    pub fn set_allow_overwrite(&mut self, allow: bool) {
        self.allow_overwrite = allow;
    }

    /// Get search paths
    pub fn search_paths(&self) -> &[PathBuf] {
        &self.search_paths
    }

    // ============================================================
    // Two-Layer Loading Methods (for system prompt integration)
    // ============================================================

    /// Generate skill descriptions for system prompt (Layer 1)
    /// Returns a formatted string listing all available skills with descriptions
    /// Budget: ~15K characters
    pub fn generate_descriptions(&self, max_chars: usize) -> String {
        let mut output = String::new();
        output.push_str("## Available Skill Patches\n\n");
        output.push_str("When a skill matches your task, invoke it to load full content.\n\n");

        let mut total_chars = output.len();

        // Collect and sort skills by priority (descending) then by name
        let mut skills: Vec<(String, Skill)> = self
            .skills
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect();
        skills.sort_by(|(_, a), (_, b)| {
            b.metadata
                .priority
                .cmp(&a.metadata.priority)
                .then_with(|| a.name.cmp(&b.name))
        });

        for (name, skill) in &skills {
            // Skip hidden skills
            if skill.is_hidden() {
                continue;
            }

            let skill_desc = format!(
                "- **{}**: {}\n",
                name,
                skill.description.lines().next().unwrap_or("")
            );

            if total_chars + skill_desc.len() > max_chars {
                output.push_str("... (more skills available)\n");
                break;
            }

            output.push_str(&skill_desc);
            total_chars += skill_desc.len();
        }

        output
    }

    /// Generate skill descriptions with extra plugin skills.
    ///
    /// Merges builtin registry skills with the given extra skills for system prompt.
    /// Does NOT modify the registry -- extra skills are read-only borrowed.
    pub fn generate_descriptions_with_extras(
        &self,
        max_chars: usize,
        extra_skills: &[Skill],
    ) -> String {
        let mut output = String::new();
        output.push_str("## Available Skill Patches\n\n");
        output.push_str("When a skill matches your task, invoke it to load full content.\n\n");

        let mut total_chars = output.len();

        // Collect all skills: registry (cloned) + extras (cloned)
        let mut all_skills: Vec<(String, Skill)> = self
            .skills
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect();

        for skill in extra_skills {
            all_skills.push((skill.name.clone(), skill.clone()));
        }

        // Sort by priority (descending) then by name
        all_skills.sort_by(|(_, a), (_, b)| {
            b.metadata
                .priority
                .cmp(&a.metadata.priority)
                .then_with(|| a.name.cmp(&b.name))
        });

        for (name, skill) in &all_skills {
            if skill.is_hidden() {
                continue;
            }

            let skill_desc = format!(
                "- **{}**: {}\n",
                name,
                skill.description.lines().next().unwrap_or("")
            );

            if total_chars + skill_desc.len() > max_chars {
                output.push_str("... (more skills available)\n");
                break;
            }

            output.push_str(&skill_desc);
            total_chars += skill_desc.len();
        }

        output
    }

    /// Get detailed description for a specific skill (for matching)
    pub fn get_skill_summary(&self, name: &str) -> Option<SkillSummary> {
        self.get(name).map(|skill| SkillSummary {
            name: skill.name.clone(),
            description: skill.description.clone(),
            keywords: skill
                .metadata
                .custom
                .get("keywords")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            requires_browser: skill
                .metadata
                .custom
                .get("requires_browser")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    }

    /// Get all skill names for quick lookup
    pub fn skill_names(&self) -> Vec<String> {
        self.skills.iter().map(|r| r.key().clone()).collect()
    }

    // ============================================================
    // Verified Plan Methods (A39 Resilient Execution Loop)
    // ============================================================

    /// Find verified plans relevant to a task query.
    ///
    /// Searches skills in the "verified-plan" namespace and ranks them
    /// by match score against the query. Returns up to `limit` results.
    pub fn find_verified_plans(&self, query: &str, limit: usize) -> Vec<Skill> {
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        let mut scored: Vec<(Skill, usize)> = self
            .skills
            .iter()
            .filter(|entry| {
                entry
                    .value()
                    .metadata
                    .namespace
                    .as_ref()
                    .map_or(false, |ns| ns == "verified-plan")
            })
            .filter_map(|entry| {
                let skill = entry.value();
                let mut score = 0usize;
                let desc_lower = skill.description.to_lowercase();
                for word in &query_words {
                    if desc_lower.contains(word) {
                        score += 2;
                    }
                }
                // Check tags
                for tag in &skill.metadata.tags {
                    if query_lower.contains(&tag.to_lowercase()) {
                        score += 3;
                    }
                }
                if score > 0 {
                    Some((skill.clone(), score))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.into_iter().take(limit).map(|(s, _)| s).collect()
    }

    /// Store a verified plan as a skill in the "verified-plan" namespace.
    ///
    /// Overwrites any existing plan with the same name.
    pub fn store_verified_plan(&self, skill: Skill) -> SkillRegistryResult<()> {
        // Force namespace to verified-plan
        let mut skill = skill;
        skill.metadata.namespace = Some("verified-plan".into());
        self.skills.insert(skill.name.clone(), skill);
        Ok(())
    }

    /// Update the outcome of a previously stored verified plan.
    ///
    /// Adjusts the `success_rate` and `usage_count` in the plan's custom metadata.
    /// Uses exponential moving average: `new_rate = 0.7 * old_rate + 0.3 * outcome`.
    pub fn update_plan_outcome(&self, plan_name: &str, success: bool) {
        // Try direct key first, then namespace-stripped key
        let key = if self.skills.contains_key(plan_name) {
            Some(plan_name.to_string())
        } else if let Some(pos) = plan_name.find(':') {
            let short = &plan_name[pos + 1..];
            if self.skills.contains_key(short) {
                Some(short.to_string())
            } else {
                None
            }
        } else {
            None
        };

        if let Some(key) = key {
            if let Some(mut entry) = self.skills.get_mut(&key) {
                let skill = entry.value_mut();
                let old_rate = skill
                    .metadata
                    .custom
                    .get("success_rate")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0);
                let usage_count = skill
                    .metadata
                    .custom
                    .get("usage_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);

                let outcome_val = if success { 1.0 } else { 0.0 };
                let new_rate = 0.7 * old_rate + 0.3 * outcome_val;

                skill
                    .metadata
                    .custom
                    .insert("success_rate".into(), serde_json::json!(new_rate));
                skill
                    .metadata
                    .custom
                    .insert("usage_count".into(), serde_json::json!(usage_count + 1));
            }
        }
    }

    /// Check if any skill matches the given keywords
    /// Returns skill names sorted by match score (highest first)
    pub fn match_skills(&self, query: &str) -> Vec<String> {
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        let mut matches: Vec<(String, usize)> = self
            .skills
            .iter()
            .filter_map(|entry| {
                let name = entry.key();
                let skill = entry.value();
                let mut score = 0;

                // Check name match
                if name.to_lowercase().contains(&query_lower) {
                    score += 10;
                }

                // Check description match
                let desc_lower = skill.description.to_lowercase();
                for word in &query_words {
                    if desc_lower.contains(word) {
                        score += 2;
                    }
                }

                // Check keywords match
                if let Some(keywords) = skill.metadata.custom.get("keywords") {
                    if let Some(arr) = keywords.as_array() {
                        for kw in arr {
                            if let Some(kw_str) = kw.as_str() {
                                if query_lower.contains(&kw_str.to_lowercase()) {
                                    score += 5;
                                }
                            }
                        }
                    }
                }

                // Check tags match
                for tag in &skill.metadata.tags {
                    if query_lower.contains(&tag.to_lowercase()) {
                        score += 3;
                    }
                }

                if score > 0 {
                    Some((name.clone(), score))
                } else {
                    None
                }
            })
            .collect();

        // Sort by score descending
        matches.sort_by(|a, b| b.1.cmp(&a.1));

        matches.into_iter().map(|(name, _)| name).collect()
    }
}

/// Summary of a skill for matching/display (Layer 1 metadata)
#[derive(Debug, Clone)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub requires_browser: bool,
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for creating a configured SkillRegistry
pub struct SkillRegistryBuilder {
    registry: SkillRegistry,
    load_builtins: bool,
    search_paths: Vec<PathBuf>,
}

impl SkillRegistryBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            registry: SkillRegistry::new(),
            load_builtins: false,
            search_paths: Vec::new(),
        }
    }

    /// Enable loading builtin skills
    pub fn with_builtins(mut self) -> Self {
        self.load_builtins = true;
        self
    }

    /// Add a search path
    pub fn search_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.search_paths.push(path.into());
        self
    }

    /// Set whether to allow overwriting
    pub fn allow_overwrite(mut self, allow: bool) -> Self {
        self.registry.allow_overwrite = allow;
        self
    }

    /// Register a skill
    pub fn skill(self, skill: Skill) -> Self {
        let _ = self.registry.register(skill);
        self
    }

    /// Build the registry
    pub fn build(mut self) -> SkillRegistryResult<SkillRegistry> {
        // Load builtins first (so they can be overwritten)
        if self.load_builtins {
            self.registry.load_builtins();
        }

        // Add search paths
        for path in self.search_paths {
            self.registry.add_search_path(path);
        }

        // Load from search paths
        self.registry.load_from_search_paths()?;

        Ok(self.registry)
    }
}

impl Default for SkillRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_skill(name: &str, description: &str) -> Skill {
        Skill::builder(name)
            .description(description)
            .prompt_template(format!("Execute {}: $ARGUMENTS", name))
            .build()
    }

    #[test]
    fn test_registry_new() {
        let registry = SkillRegistry::new();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_registry_with_builtins() {
        let registry = SkillRegistry::with_builtins();
        assert!(!registry.is_empty());
        assert!(registry.contains("commit"));
        assert!(registry.contains("plan"));
        assert!(registry.contains("bug-fix"));
    }

    #[test]
    fn test_register_and_get() {
        let registry = SkillRegistry::new();
        let skill = create_test_skill("test", "A test skill");

        registry.register(skill).unwrap();
        assert_eq!(registry.len(), 1);

        let retrieved = registry.get("test").unwrap();
        assert_eq!(retrieved.name, "test");
        assert_eq!(retrieved.description, "A test skill");
    }

    #[test]
    fn test_register_duplicate_allowed() {
        let registry = SkillRegistry::new();

        registry
            .register(create_test_skill("test", "First"))
            .unwrap();
        registry
            .register(create_test_skill("test", "Second"))
            .unwrap();

        let skill = registry.get("test").unwrap();
        assert_eq!(skill.description, "Second");
    }

    #[test]
    fn test_register_duplicate_not_allowed() {
        let mut registry = SkillRegistry::new();
        registry.set_allow_overwrite(false);

        registry
            .register(create_test_skill("test", "First"))
            .unwrap();
        let result = registry.register(create_test_skill("test", "Second"));

        assert!(matches!(
            result.unwrap_err(),
            SkillRegistryError::Duplicate(_)
        ));
    }

    #[test]
    fn test_remove() {
        let registry = SkillRegistry::new();
        registry
            .register(create_test_skill("test", "Test"))
            .unwrap();

        let removed = registry.remove("test");
        assert!(removed.is_some());
        assert!(registry.is_empty());
    }

    #[test]
    fn test_list() {
        let registry = SkillRegistry::new();
        registry
            .register(create_test_skill("alpha", "Alpha"))
            .unwrap();
        registry
            .register(create_test_skill("beta", "Beta"))
            .unwrap();
        registry
            .register(create_test_skill("gamma", "Gamma"))
            .unwrap();

        let list = registry.list();
        assert_eq!(list.len(), 3);
        // Should be sorted by name
        assert_eq!(list[0].name, "alpha");
        assert_eq!(list[1].name, "beta");
        assert_eq!(list[2].name, "gamma");
    }

    #[test]
    fn test_list_visible() {
        let registry = SkillRegistry::new();
        registry
            .register(Skill::builder("visible").build())
            .unwrap();
        registry
            .register(Skill::builder("hidden").hidden(true).build())
            .unwrap();

        let visible = registry.list_visible();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].name, "visible");
    }

    #[test]
    fn test_search() {
        let registry = SkillRegistry::new();
        registry
            .register(
                Skill::builder("git-commit")
                    .description("Create a git commit")
                    .tag("git")
                    .build(),
            )
            .unwrap();
        registry
            .register(
                Skill::builder("git-push")
                    .description("Push to remote")
                    .tag("git")
                    .build(),
            )
            .unwrap();
        registry
            .register(
                Skill::builder("docker-build")
                    .description("Build docker image")
                    .tag("docker")
                    .build(),
            )
            .unwrap();

        let results = registry.search("git");
        assert_eq!(results.len(), 2);

        let results = registry.search("commit");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "git-commit");

        let results = registry.search("docker");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_find_by_tag() {
        let registry = SkillRegistry::new();
        registry
            .register(Skill::builder("a").tag("git").build())
            .unwrap();
        registry
            .register(Skill::builder("b").tag("git").tag("vcs").build())
            .unwrap();
        registry
            .register(Skill::builder("c").tag("docker").build())
            .unwrap();

        let git_skills = registry.find_by_tag("git");
        assert_eq!(git_skills.len(), 2);

        let vcs_skills = registry.find_by_tag("vcs");
        assert_eq!(vcs_skills.len(), 1);
    }

    #[test]
    fn test_find_by_namespace() {
        let registry = SkillRegistry::new();
        registry
            .register(Skill::builder("a").namespace("git").build())
            .unwrap();
        registry
            .register(Skill::builder("b").namespace("git").build())
            .unwrap();
        registry
            .register(Skill::builder("c").namespace("docker").build())
            .unwrap();

        let git_skills = registry.find_by_namespace("git");
        assert_eq!(git_skills.len(), 2);

        let docker_skills = registry.find_by_namespace("docker");
        assert_eq!(docker_skills.len(), 1);
    }

    #[test]
    fn test_load_from_directory() {
        let temp_dir = TempDir::new().unwrap();

        // Create skill files
        fs::write(
            temp_dir.path().join("skill1.md"),
            r#"---
name: skill1
description: First skill
---
Content 1
"#,
        )
        .unwrap();

        fs::write(
            temp_dir.path().join("skill2.md"),
            r#"---
name: skill2
description: Second skill
---
Content 2
"#,
        )
        .unwrap();

        // Create a non-md file (should be ignored)
        fs::write(temp_dir.path().join("readme.txt"), "Not a skill").unwrap();

        let registry = SkillRegistry::new();
        let count = registry.load_from_directory(temp_dir.path()).unwrap();

        assert_eq!(count, 2);
        assert!(registry.contains("skill1"));
        assert!(registry.contains("skill2"));
    }

    #[test]
    fn test_load_from_nonexistent_directory() {
        let registry = SkillRegistry::new();
        let count = registry
            .load_from_directory(Path::new("/nonexistent"))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_builder() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(
            temp_dir.path().join("custom.md"),
            r#"---
name: custom
---
Custom skill
"#,
        )
        .unwrap();

        let registry = SkillRegistryBuilder::new()
            .with_builtins()
            .search_path(temp_dir.path())
            .skill(create_test_skill("inline", "Inline skill"))
            .build()
            .unwrap();

        assert!(registry.contains("commit")); // builtin
        assert!(registry.contains("custom")); // from directory
        assert!(registry.contains("inline")); // inline
    }

    #[test]
    fn test_get_with_namespace() {
        let registry = SkillRegistry::new();
        registry
            .register(Skill::builder("commit").namespace("git").build())
            .unwrap();

        // Should find by short name
        assert!(registry.get("commit").is_some());

        // Should find by qualified name
        assert!(registry.get("git:commit").is_some());
    }

    #[test]
    fn test_names() {
        let registry = SkillRegistry::new();
        registry.register(create_test_skill("a", "A")).unwrap();
        registry.register(create_test_skill("b", "B")).unwrap();

        let names = registry.names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"a".to_string()));
        assert!(names.contains(&"b".to_string()));
    }

    #[test]
    fn test_clear() {
        let registry = SkillRegistry::with_builtins();
        assert!(!registry.is_empty());

        registry.clear();
        assert!(registry.is_empty());
    }

    #[test]
    fn test_search_paths() {
        let mut registry = SkillRegistry::new();
        registry.add_search_path("/path/one");
        registry.add_search_path("/path/two");

        let paths = registry.search_paths();
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn test_list_with_priority() {
        let registry = SkillRegistry::new();
        registry
            .register(Skill::builder("low").priority(0).build())
            .unwrap();
        registry
            .register(Skill::builder("high").priority(100).build())
            .unwrap();
        registry
            .register(Skill::builder("medium").priority(50).build())
            .unwrap();

        let list = registry.list();
        assert_eq!(list[0].name, "high");
        assert_eq!(list[1].name, "medium");
        assert_eq!(list[2].name, "low");
    }

    #[test]
    fn test_list_user_defined() {
        let registry = SkillRegistry::with_builtins();
        registry.register(Skill::builder("custom").build()).unwrap();

        let user_skills = registry.list_user_defined();
        assert_eq!(user_skills.len(), 1);
        assert_eq!(user_skills[0].name, "custom");
    }

    #[test]
    fn test_list_builtins() {
        let registry = SkillRegistry::with_builtins();

        let builtins = registry.list_builtins();
        assert!(!builtins.is_empty());
        assert!(builtins.iter().all(|s| s.is_builtin()));
    }

    // ============================================================
    // Two-Layer Loading Tests
    // ============================================================

    #[test]
    fn test_generate_descriptions() {
        let registry = SkillRegistry::new();
        registry
            .register(
                Skill::builder("test-skill")
                    .description("A test skill for testing purposes")
                    .prompt_template("test")
                    .build(),
            )
            .unwrap();

        let desc = registry.generate_descriptions(1000);
        assert!(desc.contains("test-skill"));
        assert!(desc.contains("A test skill"));
        assert!(desc.contains("Available Skill Patches"));
    }

    #[test]
    fn test_generate_descriptions_respects_max_chars() {
        let registry = SkillRegistry::new();
        for i in 0..100 {
            registry
                .register(
                    Skill::builder(format!("skill-{}", i))
                        .description(format!("Description for skill number {}", i))
                        .prompt_template("test")
                        .build(),
                )
                .unwrap();
        }

        let desc = registry.generate_descriptions(500);
        assert!(desc.len() <= 550); // Allow some slack for the truncation message
        assert!(desc.contains("... (more skills available)"));
    }

    #[test]
    fn test_generate_descriptions_skips_hidden() {
        let registry = SkillRegistry::new();
        registry
            .register(
                Skill::builder("visible-skill")
                    .description("This skill is visible")
                    .prompt_template("test")
                    .build(),
            )
            .unwrap();
        registry
            .register(
                Skill::builder("hidden-skill")
                    .description("This skill is hidden")
                    .hidden(true)
                    .prompt_template("test")
                    .build(),
            )
            .unwrap();

        let desc = registry.generate_descriptions(1000);
        assert!(desc.contains("visible-skill"));
        assert!(!desc.contains("hidden-skill"));
    }

    #[test]
    fn test_get_skill_summary() {
        let registry = SkillRegistry::new();
        let mut skill = Skill::builder("gmail-automation")
            .description("Gmail email automation for sending and reading emails")
            .prompt_template("test")
            .build();
        skill.metadata.custom.insert(
            "keywords".to_string(),
            serde_json::json!(["gmail", "email", "inbox"]),
        );
        skill
            .metadata
            .custom
            .insert("requires_browser".to_string(), serde_json::json!(true));
        registry.register(skill).unwrap();

        let summary = registry.get_skill_summary("gmail-automation").unwrap();
        assert_eq!(summary.name, "gmail-automation");
        assert!(summary.description.contains("Gmail"));
        assert_eq!(summary.keywords, vec!["gmail", "email", "inbox"]);
        assert!(summary.requires_browser);
    }

    #[test]
    fn test_get_skill_summary_not_found() {
        let registry = SkillRegistry::new();
        assert!(registry.get_skill_summary("nonexistent").is_none());
    }

    #[test]
    fn test_skill_names() {
        let registry = SkillRegistry::new();
        registry
            .register(create_test_skill("skill-a", "A"))
            .unwrap();
        registry
            .register(create_test_skill("skill-b", "B"))
            .unwrap();
        registry
            .register(create_test_skill("skill-c", "C"))
            .unwrap();

        let names = registry.skill_names();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"skill-a".to_string()));
        assert!(names.contains(&"skill-b".to_string()));
        assert!(names.contains(&"skill-c".to_string()));
    }

    #[test]
    fn test_match_skills() {
        let registry = SkillRegistry::new();

        let mut skill = Skill::builder("gmail-automation")
            .description("Gmail email automation for sending and reading emails")
            .prompt_template("test")
            .build();
        skill.metadata.custom.insert(
            "keywords".to_string(),
            serde_json::json!(["gmail", "email", "inbox"]),
        );
        registry.register(skill).unwrap();

        let matches = registry.match_skills("send email");
        assert!(!matches.is_empty());
        assert_eq!(matches[0], "gmail-automation");
    }

    #[test]
    fn test_match_skills_by_name() {
        let registry = SkillRegistry::new();
        registry
            .register(
                Skill::builder("github-pr-review")
                    .description("Review pull requests on GitHub")
                    .prompt_template("test")
                    .build(),
            )
            .unwrap();

        let matches = registry.match_skills("github");
        assert!(!matches.is_empty());
        assert_eq!(matches[0], "github-pr-review");
    }

    #[test]
    fn test_match_skills_by_tags() {
        let registry = SkillRegistry::new();
        registry
            .register(
                Skill::builder("deploy-k8s")
                    .description("Deploy to Kubernetes cluster")
                    .tag("kubernetes")
                    .tag("deployment")
                    .prompt_template("test")
                    .build(),
            )
            .unwrap();

        let matches = registry.match_skills("kubernetes");
        assert!(!matches.is_empty());
        assert_eq!(matches[0], "deploy-k8s");
    }

    #[test]
    fn test_match_skills_sorted_by_score() {
        let registry = SkillRegistry::new();

        // Lower score - only description match (no name/keyword bonus)
        registry
            .register(
                Skill::builder("task-handler")
                    .description("Handle sending messages and notifications")
                    .prompt_template("test")
                    .build(),
            )
            .unwrap();

        // Higher score - name match + description + keywords
        let mut gmail_skill = Skill::builder("gmail-sender")
            .description("Gmail specific automation for sending messages")
            .prompt_template("test")
            .build();
        gmail_skill.metadata.custom.insert(
            "keywords".to_string(),
            serde_json::json!(["gmail", "send", "messages"]),
        );
        registry.register(gmail_skill).unwrap();

        // Query "send messages" - gmail-sender should score higher
        // gmail-sender: description match for "send" (+2) and "messages" (+2), keyword "send" (+5), keyword "messages" (+5) = 14
        // task-handler: description match for "sending" (+2) and "messages" (+2) = 4
        let matches = registry.match_skills("send messages");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0], "gmail-sender"); // Higher score due to keywords
        assert_eq!(matches[1], "task-handler");
    }

    // ============================================================
    // Verified Plan Tests (A39)
    // ============================================================

    #[test]
    fn test_find_verified_plans() {
        let registry = SkillRegistry::new();
        registry
            .store_verified_plan(
                Skill::builder("verified-plan-abc")
                    .description("Send email via Gmail browser automation")
                    .namespace("verified-plan")
                    .tag("email")
                    .tag("gmail")
                    .build(),
            )
            .unwrap();
        registry
            .store_verified_plan(
                Skill::builder("verified-plan-def")
                    .description("Deploy code to production server")
                    .namespace("verified-plan")
                    .tag("deploy")
                    .build(),
            )
            .unwrap();

        let results = registry.find_verified_plans("send email gmail", 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "verified-plan-abc");

        let results = registry.find_verified_plans("deploy server", 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "verified-plan-def");

        let results = registry.find_verified_plans("unrelated query", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_store_verified_plan_forces_namespace() {
        let registry = SkillRegistry::new();
        let skill = Skill::builder("my-plan")
            .description("A plan")
            .namespace("wrong-namespace")
            .build();
        registry.store_verified_plan(skill).unwrap();

        let stored = registry.get("my-plan").unwrap();
        assert_eq!(stored.metadata.namespace.as_deref(), Some("verified-plan"));
    }

    #[test]
    fn test_update_plan_outcome() {
        let registry = SkillRegistry::new();
        let mut skill = Skill::builder("verified-plan-test")
            .description("Test plan")
            .namespace("verified-plan")
            .build();
        skill
            .metadata
            .custom
            .insert("success_rate".into(), serde_json::json!(1.0));
        skill
            .metadata
            .custom
            .insert("usage_count".into(), serde_json::json!(1));
        registry.store_verified_plan(skill).unwrap();

        // Record a failure
        registry.update_plan_outcome("verified-plan-test", false);
        let s = registry.get("verified-plan-test").unwrap();
        let rate = s
            .metadata
            .custom
            .get("success_rate")
            .unwrap()
            .as_f64()
            .unwrap();
        assert!((rate - 0.7).abs() < 0.01); // 0.7 * 1.0 + 0.3 * 0.0 = 0.7
        let count = s
            .metadata
            .custom
            .get("usage_count")
            .unwrap()
            .as_u64()
            .unwrap();
        assert_eq!(count, 2);

        // Record a success
        registry.update_plan_outcome("verified-plan-test", true);
        let s = registry.get("verified-plan-test").unwrap();
        let rate = s
            .metadata
            .custom
            .get("success_rate")
            .unwrap()
            .as_f64()
            .unwrap();
        assert!((rate - 0.79).abs() < 0.01); // 0.7 * 0.7 + 0.3 * 1.0 = 0.79
    }

    #[test]
    fn test_match_skills_no_match() {
        let registry = SkillRegistry::new();
        registry
            .register(
                Skill::builder("git-commit")
                    .description("Create git commits")
                    .prompt_template("test")
                    .build(),
            )
            .unwrap();

        let matches = registry.match_skills("kubernetes deployment");
        assert!(matches.is_empty());
    }

    // ============================================================
    // generate_descriptions_with_extras Tests
    // ============================================================

    #[test]
    fn test_registry_descriptions_with_extras() {
        let registry = SkillRegistry::new();
        registry
            .register(
                Skill::builder("commit")
                    .description("Create a git commit")
                    .prompt_template("test")
                    .build(),
            )
            .unwrap();

        let extras = vec![
            Skill::builder("pdf")
                .description("PDF processing plugin")
                .prompt_template("test")
                .build(),
            Skill::builder("docx")
                .description("Word document processing")
                .prompt_template("test")
                .build(),
        ];

        let desc = registry.generate_descriptions_with_extras(5000, &extras);
        assert!(desc.contains("commit"));
        assert!(desc.contains("pdf"));
        assert!(desc.contains("docx"));
        assert!(desc.contains("PDF processing"));
        assert!(desc.contains("Word document"));
    }

    #[test]
    fn test_registry_extras_within_budget() {
        let registry = SkillRegistry::new();
        for i in 0..50 {
            registry
                .register(
                    Skill::builder(format!("builtin-{}", i))
                        .description(format!("Builtin skill number {}", i))
                        .prompt_template("test")
                        .build(),
                )
                .unwrap();
        }

        let extras: Vec<Skill> = (0..50)
            .map(|i| {
                Skill::builder(format!("plugin-{}", i))
                    .description(format!("Plugin skill number {}", i))
                    .prompt_template("test")
                    .build()
            })
            .collect();

        let desc = registry.generate_descriptions_with_extras(500, &extras);
        assert!(desc.len() <= 550); // Some slack for truncation message
        assert!(desc.contains("... (more skills available)"));
    }

    #[test]
    fn test_registry_extras_empty() {
        let registry = SkillRegistry::new();
        registry
            .register(
                Skill::builder("commit")
                    .description("Create a git commit")
                    .prompt_template("test")
                    .build(),
            )
            .unwrap();

        let no_extras: &[Skill] = &[];
        let with_extras = registry.generate_descriptions_with_extras(5000, no_extras);
        let without_extras = registry.generate_descriptions(5000);

        // Should be identical when no extras
        assert_eq!(with_extras, without_extras);
    }
}
