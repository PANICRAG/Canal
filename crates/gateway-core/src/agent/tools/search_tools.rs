//! search_tools Meta-Tool (A46 Role Constraint System)
//!
//! Enables Claude Code-style on-demand tool discovery. Instead of sending
//! all tool schemas to the LLM upfront, the LLM starts with core tools
//! plus `search_tools`, and discovers additional tools as needed.
//!
//! This reduces per-turn token consumption by ~65% for roles with many
//! available tools.

use super::traits::{AgentTool, ToolError, ToolMetadata};
use super::ToolContext;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Input for the search_tools meta-tool.
#[derive(Debug, Serialize, Deserialize)]
pub struct SearchToolsInput {
    /// Search keywords (e.g., "deploy", "database", "monitoring")
    pub query: String,
    /// Optional: filter by namespace (e.g., "hosting", "platform")
    #[serde(default)]
    pub namespace: Option<String>,
}

/// Output from the search_tools meta-tool.
#[derive(Debug, Serialize, Deserialize)]
pub struct SearchToolsOutput {
    /// Matching tools with their schemas
    pub tools: Vec<DiscoveredTool>,
    /// Total matches found
    pub total_matches: usize,
}

/// A discovered tool returned by search_tools.
#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoveredTool {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// Tool namespace
    pub namespace: String,
    /// Full input schema (so LLM can call it immediately)
    pub input_schema: serde_json::Value,
}

/// Searchable tool catalog for the search_tools meta-tool.
///
/// This is a snapshot of available tools that can be searched by keyword.
/// It's separate from ToolRegistry to avoid circular dependencies.
pub struct SearchableToolCatalog {
    tools: Vec<ToolMetadata>,
}

impl SearchableToolCatalog {
    /// Create from a list of tool metadata.
    pub fn new(tools: Vec<ToolMetadata>) -> Self {
        Self { tools }
    }

    /// Search tools by query string, optionally filtered by namespace.
    ///
    /// Matches against tool name (case-insensitive) and description.
    /// Returns up to `max_results` matches.
    pub fn search(
        &self,
        query: &str,
        namespace: Option<&str>,
        max_results: usize,
    ) -> Vec<&ToolMetadata> {
        let query_lower = query.to_lowercase();
        let keywords: Vec<&str> = query_lower.split_whitespace().collect();

        let mut scored: Vec<(usize, &ToolMetadata)> = self
            .tools
            .iter()
            .filter(|tool| {
                // Namespace filter
                if let Some(ns) = namespace {
                    if !tool.namespace.eq_ignore_ascii_case(ns)
                        && !tool.name.starts_with(&format!("{}_", ns))
                    {
                        return false;
                    }
                }
                true
            })
            .filter_map(|tool| {
                let name_lower = tool.name.to_lowercase();
                let desc_lower = tool.description.to_lowercase();

                // Score: count matching keywords (name matches score higher)
                let mut score = 0usize;
                for kw in &keywords {
                    if name_lower.contains(kw) {
                        score += 3; // Name match is worth more
                    }
                    if desc_lower.contains(kw) {
                        score += 1;
                    }
                }

                if score > 0 {
                    Some((score, tool))
                } else {
                    None
                }
            })
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.0.cmp(&a.0));

        scored
            .into_iter()
            .take(max_results)
            .map(|(_, tool)| tool)
            .collect()
    }
}

/// The search_tools meta-tool.
///
/// When tool discovery is enabled for a role, this tool is added to the
/// initial tool set. The LLM calls it to discover additional tools.
pub struct SearchToolsTool {
    catalog: Arc<SearchableToolCatalog>,
}

impl SearchToolsTool {
    /// Create a new search_tools tool with access to the tool catalog.
    pub fn new(catalog: Arc<SearchableToolCatalog>) -> Self {
        Self { catalog }
    }
}

#[async_trait]
impl AgentTool for SearchToolsTool {
    type Input = SearchToolsInput;
    type Output = SearchToolsOutput;

    fn name(&self) -> &str {
        "search_tools"
    }

    fn description(&self) -> &str {
        "Search for available tools by keyword or namespace. Use this to discover tools you need for the current task. Returns matching tool names, descriptions, and input schemas so you can call them immediately."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search keywords (e.g., 'deploy', 'database', 'monitoring', 'git')"
                },
                "namespace": {
                    "type": "string",
                    "description": "Optional: filter by tool namespace (e.g., 'hosting', 'platform', 'devtools')"
                }
            },
            "required": ["query"]
        })
    }

    fn namespace(&self) -> &str {
        "system"
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn requires_permission(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> Result<Self::Output, ToolError> {
        let matches = self.catalog.search(
            &input.query,
            input.namespace.as_deref(),
            5, // Return top 5 matches
        );

        let total_matches = matches.len();
        let tools: Vec<DiscoveredTool> = matches
            .into_iter()
            .map(|m| DiscoveredTool {
                name: m.name.clone(),
                description: m.description.clone(),
                namespace: m.namespace.clone(),
                input_schema: m.input_schema.clone(),
            })
            .collect();

        Ok(SearchToolsOutput {
            tools,
            total_matches,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_catalog() -> SearchableToolCatalog {
        SearchableToolCatalog::new(vec![
            ToolMetadata {
                name: "hosting_deploy_app".to_string(),
                description: "Deploy an application to the hosting platform".to_string(),
                namespace: "hosting".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                requires_permission: false,
                is_mutating: true,
            },
            ToolMetadata {
                name: "hosting_analyze_repo".to_string(),
                description: "Analyze a git repository for deployment configuration".to_string(),
                namespace: "hosting".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                requires_permission: false,
                is_mutating: false,
            },
            ToolMetadata {
                name: "platform_create_instance".to_string(),
                description: "Create a new platform instance for a tenant".to_string(),
                namespace: "platform".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                requires_permission: true,
                is_mutating: true,
            },
            ToolMetadata {
                name: "Read".to_string(),
                description: "Read a file from the filesystem".to_string(),
                namespace: "filesystem".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                requires_permission: false,
                is_mutating: false,
            },
            ToolMetadata {
                name: "devtools_get_logs".to_string(),
                description: "Get application logs from the monitoring system".to_string(),
                namespace: "devtools".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                requires_permission: false,
                is_mutating: false,
            },
            ToolMetadata {
                name: "devtools_get_metrics".to_string(),
                description: "Get performance metrics and monitoring data".to_string(),
                namespace: "devtools".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                requires_permission: false,
                is_mutating: false,
            },
        ])
    }

    #[test]
    fn test_search_by_keyword() {
        let catalog = sample_catalog();

        let results = catalog.search("deploy", None, 5);
        assert_eq!(results.len(), 2); // hosting_deploy_app + hosting_analyze_repo (description match)
        assert_eq!(results[0].name, "hosting_deploy_app"); // Name match scores higher
    }

    #[test]
    fn test_search_by_namespace() {
        let catalog = sample_catalog();

        let results = catalog.search("get", Some("devtools"), 5);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.namespace == "devtools"));
    }

    #[test]
    fn test_search_no_results() {
        let catalog = sample_catalog();

        let results = catalog.search("nonexistent_xyzzy", None, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_max_results() {
        let catalog = sample_catalog();

        // "a" appears in many tool names/descriptions
        let results = catalog.search("a", None, 2);
        assert!(results.len() <= 2);
    }

    #[test]
    fn test_search_case_insensitive() {
        let catalog = sample_catalog();

        let results_lower = catalog.search("deploy", None, 5);
        let results_upper = catalog.search("DEPLOY", None, 5);
        assert_eq!(results_lower.len(), results_upper.len());
    }

    #[test]
    fn test_search_multi_keyword() {
        let catalog = sample_catalog();

        // "git repository" should match hosting_analyze_repo strongly
        let results = catalog.search("git repository", None, 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "hosting_analyze_repo");
    }

    #[tokio::test]
    async fn test_search_tools_execution() {
        let catalog = Arc::new(sample_catalog());
        let tool = SearchToolsTool::new(catalog);

        let input = SearchToolsInput {
            query: "deploy".to_string(),
            namespace: None,
        };

        let context = ToolContext::default();
        let result = tool.execute(input, &context).await.unwrap();
        assert!(!result.tools.is_empty());
        assert_eq!(result.tools[0].name, "hosting_deploy_app");
    }

    #[tokio::test]
    async fn test_search_tools_with_namespace() {
        let catalog = Arc::new(sample_catalog());
        let tool = SearchToolsTool::new(catalog);

        let input = SearchToolsInput {
            query: "logs metrics".to_string(),
            namespace: Some("devtools".to_string()),
        };

        let context = ToolContext::default();
        let result = tool.execute(input, &context).await.unwrap();
        assert_eq!(result.total_matches, 2);
        assert!(result.tools.iter().all(|t| t.namespace == "devtools"));
    }
}
