//! Search Tools - Glob and Grep

use super::{AgentTool, ToolContext, ToolError, ToolResult};
use async_trait::async_trait;
use glob::glob;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

// ============================================================================
// Glob Tool
// ============================================================================

/// Glob tool input
#[derive(Debug, Clone, Deserialize)]
pub struct GlobInput {
    /// The glob pattern to match
    pub pattern: String,
    /// The directory to search in (defaults to cwd)
    #[serde(default)]
    pub path: Option<String>,
}

/// Glob tool output
#[derive(Debug, Clone, Serialize)]
pub struct GlobOutput {
    /// Matched file paths
    pub files: Vec<String>,
    /// Number of files found
    pub count: u32,
    /// Whether results were truncated
    pub truncated: bool,
}

/// Glob file search tool
pub struct GlobTool {
    /// Maximum number of results
    pub max_results: usize,
}

impl Default for GlobTool {
    fn default() -> Self {
        Self { max_results: 1000 }
    }
}

#[async_trait]
impl AgentTool for GlobTool {
    type Input = GlobInput;
    type Output = GlobOutput;

    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self) -> &str {
        r#"Fast file pattern matching tool that works with any codebase size.

Usage:
- Supports glob patterns like "**/*.js" or "src/**/*.ts"
- Returns matching file paths sorted by modification time
- Use this tool when you need to find files by name patterns

When to use Glob vs Grep:
- Use Glob when you know the file name pattern (e.g., "*.config.ts")
- Use Grep when you need to search file contents

When NOT to use:
- For open-ended searches that may require multiple rounds, use the Task tool with subagent_type=Explore instead

Examples:
- "**/*.rs" - All Rust files recursively
- "src/**/*.ts" - All TypeScript files under src
- "**/test_*.py" - All Python test files"#
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match files against"
                },
                "path": {
                    "type": "string",
                    "description": "The directory to search in (defaults to cwd)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "filesystem"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        let base_path = input
            .path
            .map(|p| context.resolve_path(&p))
            .unwrap_or_else(|| context.cwd.clone());

        // Construct full pattern
        let full_pattern = base_path.join(&input.pattern);
        let pattern_str = full_pattern.to_string_lossy();

        // Execute glob
        let mut files = Vec::new();
        let mut truncated = false;

        match glob(&pattern_str) {
            Ok(paths) => {
                for entry in paths.flatten() {
                    let entry: PathBuf = entry;
                    // Check if path is allowed
                    if !context.is_path_allowed(&entry) {
                        continue;
                    }

                    if files.len() >= self.max_results {
                        truncated = true;
                        break;
                    }

                    files.push(entry.to_string_lossy().to_string());
                }
            }
            Err(e) => {
                return Err(ToolError::InvalidInput(format!(
                    "Invalid glob pattern: {}",
                    e
                )));
            }
        }

        // Sort by modification time (most recent first)
        files.sort_by(|a, b| {
            let mtime = |p: &str| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            };
            mtime(b).cmp(&mtime(a))
        });

        let count = files.len() as u32;

        Ok(GlobOutput {
            files,
            count,
            truncated,
        })
    }
}

// ============================================================================
// Grep Tool
// ============================================================================

/// Grep output mode
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GrepOutputMode {
    /// Show matching lines with content
    Content,
    /// Show only file paths
    #[default]
    FilesWithMatches,
    /// Show match counts
    Count,
}

/// Grep tool input
#[derive(Debug, Clone, Deserialize)]
pub struct GrepInput {
    /// The regex pattern to search for
    pub pattern: String,
    /// The file or directory to search in
    #[serde(default)]
    pub path: Option<String>,
    /// Glob pattern to filter files
    #[serde(default)]
    pub glob: Option<String>,
    /// File type to search
    #[serde(default, rename = "type")]
    pub file_type: Option<String>,
    /// Output mode
    #[serde(default)]
    pub output_mode: GrepOutputMode,
    /// Case insensitive search
    #[serde(default, rename = "-i")]
    pub case_insensitive: bool,
    /// Show line numbers
    #[serde(default = "default_true", rename = "-n")]
    pub line_numbers: bool,
    /// Lines after match
    #[serde(default, rename = "-A")]
    pub after_context: Option<u32>,
    /// Lines before match
    #[serde(default, rename = "-B")]
    pub before_context: Option<u32>,
    /// Lines around match
    #[serde(default, rename = "-C")]
    pub context: Option<u32>,
    /// Limit results
    #[serde(default)]
    pub head_limit: Option<u32>,
    /// Enable multiline matching
    #[serde(default)]
    pub multiline: bool,
}

fn default_true() -> bool {
    true
}

/// Grep match result
#[derive(Debug, Clone, Serialize)]
pub struct GrepMatch {
    /// File path
    pub file: String,
    /// Line number (1-indexed)
    pub line: u32,
    /// Matched content
    pub content: String,
}

/// Grep tool output
#[derive(Debug, Clone, Serialize)]
pub struct GrepOutput {
    /// Matched results
    pub matches: Vec<GrepMatch>,
    /// Files with matches (for files_with_matches mode)
    pub files: Vec<String>,
    /// Match count per file (for count mode)
    pub counts: Vec<(String, u32)>,
    /// Total matches
    pub total_matches: u32,
    /// Files searched
    pub files_searched: u32,
    /// Whether results were truncated
    pub truncated: bool,
}

/// Grep search tool
pub struct GrepTool {
    /// Maximum results
    pub max_results: usize,
    /// Maximum files to search
    pub max_files: usize,
}

impl Default for GrepTool {
    fn default() -> Self {
        Self {
            max_results: 1000,
            max_files: 10000,
        }
    }
}

impl GrepTool {
    /// Get file extension for a file type
    fn type_to_extensions(file_type: &str) -> Vec<&'static str> {
        match file_type.to_lowercase().as_str() {
            "js" | "javascript" => vec!["js", "mjs", "cjs"],
            "ts" | "typescript" => vec!["ts", "tsx", "mts", "cts"],
            "py" | "python" => vec!["py", "pyi"],
            "rs" | "rust" => vec!["rs"],
            "go" | "golang" => vec!["go"],
            "java" => vec!["java"],
            "c" => vec!["c", "h"],
            "cpp" | "c++" => vec!["cpp", "hpp", "cc", "hh", "cxx", "hxx"],
            "rb" | "ruby" => vec!["rb"],
            "php" => vec!["php"],
            "swift" => vec!["swift"],
            "kt" | "kotlin" => vec!["kt", "kts"],
            "md" | "markdown" => vec!["md", "markdown"],
            "json" => vec!["json"],
            "yaml" | "yml" => vec!["yaml", "yml"],
            "toml" => vec!["toml"],
            "html" => vec!["html", "htm"],
            "css" => vec!["css"],
            "sql" => vec!["sql"],
            _ => vec![],
        }
    }
}

#[async_trait]
impl AgentTool for GrepTool {
    type Input = GrepInput;
    type Output = GrepOutput;

    fn name(&self) -> &str {
        "Grep"
    }

    fn description(&self) -> &str {
        r#"A powerful search tool built on ripgrep for searching file contents.

Usage:
- Supports full regex syntax (e.g., "log.*Error", "function\s+\w+")
- Filter files with glob parameter (e.g., "*.js", "**/*.tsx") or type parameter (e.g., "js", "py", "rust")
- Output modes: "content" shows matching lines, "files_with_matches" shows only file paths (default), "count" shows match counts

Pattern syntax:
- Uses ripgrep (not grep) - literal braces need escaping (use `interface\{\}` to find `interface{}` in Go code)
- Multiline matching: By default patterns match within single lines only. For cross-line patterns like `struct \{[\s\S]*?field`, use multiline: true

When NOT to use:
- For open-ended searches requiring multiple rounds, use the Task tool with subagent_type=Explore instead
- For finding files by name, use Glob instead"#
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in"
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files"
                },
                "type": {
                    "type": "string",
                    "description": "File type to search (js, py, rs, etc.)"
                },
                "output_mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count"],
                    "description": "Output mode"
                },
                "-i": {
                    "type": "boolean",
                    "description": "Case insensitive search"
                },
                "-n": {
                    "type": "boolean",
                    "description": "Show line numbers"
                },
                "head_limit": {
                    "type": "integer",
                    "description": "Limit number of results"
                }
            },
            "required": ["pattern"]
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn is_mutating(&self) -> bool {
        false
    }

    fn namespace(&self) -> &str {
        "filesystem"
    }

    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output> {
        // Compile regex — R1-H17: apply multiline flag when requested
        let flags = match (input.case_insensitive, input.multiline) {
            (true, true) => "(?is)",
            (true, false) => "(?i)",
            (false, true) => "(?s)",
            (false, false) => "",
        };
        let regex = Regex::new(&format!("{}{}", flags, input.pattern))
            .map_err(|e| ToolError::InvalidInput(format!("Invalid regex: {}", e)))?;

        let base_path = input
            .path
            .as_ref()
            .map(|p| context.resolve_path(p))
            .unwrap_or_else(|| context.cwd.clone());

        // Collect files to search
        let files = self.collect_files(&base_path, &input, context).await?;

        let limit = input.head_limit.unwrap_or(self.max_results as u32) as usize;
        let mut matches = Vec::new();
        let mut file_matches: Vec<String> = Vec::new();
        let mut counts: Vec<(String, u32)> = Vec::new();
        let mut total_matches = 0u32;
        let mut files_searched = 0u32;
        let mut truncated = false;

        for file_path in files {
            if files_searched >= self.max_files as u32 {
                truncated = true;
                break;
            }

            files_searched += 1;

            // R1-H25: Skip files larger than 10MB to prevent excessive memory usage
            const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;
            if let Ok(metadata) = fs::metadata(&file_path).await {
                if metadata.len() > MAX_FILE_SIZE {
                    continue;
                }
            }

            // Read file content
            let content = match fs::read_to_string(&file_path).await {
                Ok(c) => c,
                Err(_) => continue, // Skip binary or unreadable files
            };

            let file_str = file_path.to_string_lossy().to_string();
            let mut file_match_count = 0u32;

            for (line_num, line) in content.lines().enumerate() {
                if regex.is_match(line) {
                    file_match_count += 1;
                    total_matches += 1;

                    if matches.len() < limit {
                        matches.push(GrepMatch {
                            file: file_str.clone(),
                            line: (line_num + 1) as u32,
                            content: line.to_string(),
                        });
                    } else {
                        truncated = true;
                    }
                }
            }

            if file_match_count > 0 {
                file_matches.push(file_str.clone());
                counts.push((file_str, file_match_count));
            }
        }

        Ok(GrepOutput {
            matches,
            files: file_matches,
            counts,
            total_matches,
            files_searched,
            truncated,
        })
    }
}

impl GrepTool {
    async fn collect_files(
        &self,
        base_path: &PathBuf,
        input: &GrepInput,
        context: &ToolContext,
    ) -> ToolResult<Vec<PathBuf>> {
        let mut files = Vec::new();

        if base_path.is_file() {
            if context.is_path_allowed(base_path) {
                files.push(base_path.clone());
            }
            return Ok(files);
        }

        // Build glob pattern
        let pattern = if let Some(ref glob_pattern) = input.glob {
            base_path.join(glob_pattern)
        } else if let Some(ref file_type) = input.file_type {
            let extensions = Self::type_to_extensions(file_type);
            if extensions.is_empty() {
                return Err(ToolError::InvalidInput(format!(
                    "Unknown file type: {}",
                    file_type
                )));
            }
            // Use first extension with recursive glob
            base_path.join(format!("**/*.{}", extensions[0]))
        } else {
            base_path.join("**/*")
        };

        let pattern_str = pattern.to_string_lossy();

        match glob(&pattern_str) {
            Ok(paths) => {
                for entry in paths.flatten() {
                    let entry: PathBuf = entry;
                    if entry.is_file() && context.is_path_allowed(&entry) {
                        // Check file type filter
                        if let Some(ref file_type) = input.file_type {
                            let extensions = Self::type_to_extensions(file_type);
                            let ext = entry.extension().and_then(|e| e.to_str()).unwrap_or("");
                            if !extensions.contains(&ext) {
                                continue;
                            }
                        }

                        files.push(entry);

                        if files.len() >= self.max_files {
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                return Err(ToolError::InvalidInput(format!(
                    "Invalid glob pattern: {}",
                    e
                )));
            }
        }

        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_glob_tool() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(temp_dir.path().join("test1.txt"), "content").unwrap();
        std::fs::write(temp_dir.path().join("test2.txt"), "content").unwrap();
        std::fs::create_dir(temp_dir.path().join("subdir")).unwrap();
        std::fs::write(temp_dir.path().join("subdir/test3.txt"), "content").unwrap();

        let tool = GlobTool::default();
        let context =
            ToolContext::new("s1", temp_dir.path()).with_allowed_directory(temp_dir.path());

        let input = GlobInput {
            pattern: "**/*.txt".to_string(),
            path: None,
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert_eq!(output.count, 3);
    }

    #[tokio::test]
    async fn test_grep_tool() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(
            temp_dir.path().join("test.txt"),
            "hello world\nfoo bar\nhello again",
        )
        .unwrap();

        let tool = GrepTool::default();
        let context =
            ToolContext::new("s1", temp_dir.path()).with_allowed_directory(temp_dir.path());

        let input = GrepInput {
            pattern: "hello".to_string(),
            path: None,
            glob: Some("*.txt".to_string()),
            file_type: None,
            output_mode: GrepOutputMode::Content,
            case_insensitive: false,
            line_numbers: true,
            after_context: None,
            before_context: None,
            context: None,
            head_limit: None,
            multiline: false,
        };

        let output = tool.execute(input, &context).await.unwrap();
        assert_eq!(output.total_matches, 2);
        assert_eq!(output.matches.len(), 2);
    }

    #[test]
    fn test_type_to_extensions() {
        assert_eq!(GrepTool::type_to_extensions("rs"), vec!["rs"]);
        assert_eq!(GrepTool::type_to_extensions("js"), vec!["js", "mjs", "cjs"]);
        assert_eq!(GrepTool::type_to_extensions("python"), vec!["py", "pyi"]);
    }
}
