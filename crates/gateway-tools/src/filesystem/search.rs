//! File Search
//!
//! Provides file content search capabilities using ripgrep for performance.

use crate::error::{ServiceError as Error, ServiceResult as Result};
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::permissions::PermissionGuard;

/// A single search match
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    /// File path containing the match
    pub path: String,
    /// Line number (1-indexed)
    pub line_number: usize,
    /// Line content
    pub line_content: String,
    /// Start index of match within line
    pub match_start: usize,
    /// End index of match within line
    pub match_end: usize,
}

/// Search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Matches found
    pub matches: Vec<SearchMatch>,
    /// Total number of matches (may be more than returned if limited)
    pub total_matches: usize,
    /// Number of files searched
    pub files_searched: usize,
    /// Whether results were truncated
    pub truncated: bool,
}

/// File searcher using ripgrep
pub struct FileSearcher {
    permission_guard: Arc<PermissionGuard>,
}

impl FileSearcher {
    /// Create a new file searcher
    pub fn new(permission_guard: Arc<PermissionGuard>) -> Self {
        Self { permission_guard }
    }

    /// Search for a pattern in files
    pub async fn search(
        &self,
        path: &str,
        pattern: &str,
        file_pattern: Option<&str>,
        max_results: usize,
    ) -> Result<SearchResult> {
        // Check permission
        if !self.permission_guard.can_read(path) {
            return Err(Error::NotFound(format!("Path not accessible: {}", path)));
        }

        let canonical = self.permission_guard.resolve_path(path)?;

        // Check if ripgrep is available
        let rg_available = Command::new("rg")
            .arg("--version")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        if rg_available {
            self.search_with_ripgrep(&canonical, pattern, file_pattern, max_results)
                .await
        } else {
            // Fall back to basic grep-like search
            self.search_basic(&canonical, pattern, file_pattern, max_results)
                .await
        }
    }

    /// Search using ripgrep
    async fn search_with_ripgrep(
        &self,
        path: &std::path::PathBuf,
        pattern: &str,
        file_pattern: Option<&str>,
        max_results: usize,
    ) -> Result<SearchResult> {
        let mut cmd = Command::new("rg");

        // Set options
        cmd.arg("--line-number") // Show line numbers
            .arg("--column") // Show column numbers
            .arg("--no-heading") // One line per match
            .arg("--with-filename") // Show filenames
            .arg("--max-count")
            .arg(max_results.to_string()); // Limit per-file results

        // Add file pattern filter if specified
        if let Some(glob) = file_pattern {
            cmd.arg("--glob").arg(glob);
        }

        // Use -- to separate options from the pattern, preventing argument injection
        // (e.g., pattern starting with "--" being interpreted as a flag)
        cmd.arg("--").arg(pattern).arg(path);

        // Run ripgrep
        let output = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("Failed to spawn ripgrep: {}", e)))?;

        let stdout = output
            .stdout
            .ok_or_else(|| Error::Internal("Failed to capture ripgrep stdout".into()))?;

        let mut matches = Vec::new();
        let mut reader = BufReader::new(stdout).lines();

        while let Some(line) = reader
            .next_line()
            .await
            .map_err(|e| Error::Internal(format!("Failed to read ripgrep output: {}", e)))?
        {
            if matches.len() >= max_results {
                break;
            }

            // Parse ripgrep output format: file:line:column:content
            if let Some(m) = self.parse_rg_line(&line) {
                // Verify we can read this file
                if self.permission_guard.can_read(&m.path) {
                    matches.push(m);
                }
            }
        }

        let truncated = matches.len() >= max_results;

        Ok(SearchResult {
            total_matches: matches.len(),
            files_searched: matches
                .iter()
                .map(|m| m.path.as_str())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            matches,
            truncated,
        })
    }

    /// Parse a ripgrep output line
    fn parse_rg_line(&self, line: &str) -> Option<SearchMatch> {
        // Format: file:line:column:content
        let parts: Vec<&str> = line.splitn(4, ':').collect();

        if parts.len() >= 4 {
            let path = parts[0].to_string();
            let line_number = parts[1].parse().ok()?;
            let column: usize = parts[2].parse().ok()?;
            let line_content = parts[3].to_string();

            Some(SearchMatch {
                path,
                line_number,
                line_content: line_content.clone(),
                match_start: column.saturating_sub(1),
                match_end: column.saturating_sub(1) + 1, // Approximate
            })
        } else if parts.len() == 3 {
            // Sometimes column is missing
            let path = parts[0].to_string();
            let line_number = parts[1].parse().ok()?;
            let line_content = parts[2].to_string();

            Some(SearchMatch {
                path,
                line_number,
                line_content,
                match_start: 0,
                match_end: 0,
            })
        } else {
            None
        }
    }

    /// Basic search fallback when ripgrep is not available
    async fn search_basic(
        &self,
        path: &std::path::PathBuf,
        pattern: &str,
        file_pattern: Option<&str>,
        max_results: usize,
    ) -> Result<SearchResult> {
        let mut matches = Vec::new();
        let mut files_searched = 0;

        self.search_directory(
            path,
            pattern,
            file_pattern,
            max_results,
            &mut matches,
            &mut files_searched,
        )
        .await?;

        let truncated = matches.len() >= max_results;

        Ok(SearchResult {
            total_matches: matches.len(),
            files_searched,
            matches,
            truncated,
        })
    }

    /// Recursively search a directory
    #[async_recursion::async_recursion]
    async fn search_directory(
        &self,
        path: &std::path::PathBuf,
        pattern: &str,
        file_pattern: Option<&str>,
        max_results: usize,
        matches: &mut Vec<SearchMatch>,
        files_searched: &mut usize,
    ) -> Result<()> {
        if matches.len() >= max_results {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(path)
            .await
            .map_err(|e| Error::Internal(format!("Failed to read directory: {}", e)))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| Error::Internal(format!("Failed to read directory entry: {}", e)))?
        {
            if matches.len() >= max_results {
                break;
            }

            let entry_path = entry.path();
            let metadata = entry.metadata().await.ok();

            if let Some(meta) = metadata {
                if meta.is_file() {
                    // Check file pattern
                    if let Some(glob) = file_pattern {
                        let file_name = entry_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("");
                        if !self.matches_glob(file_name, glob) {
                            continue;
                        }
                    }

                    // Search file
                    self.search_file(&entry_path, pattern, max_results - matches.len(), matches)
                        .await?;
                    *files_searched += 1;
                } else if meta.is_dir() {
                    // Recursively search subdirectory
                    let path_str = entry_path.to_string_lossy().to_string();
                    if self.permission_guard.can_read(&path_str) {
                        self.search_directory(
                            &entry_path,
                            pattern,
                            file_pattern,
                            max_results,
                            matches,
                            files_searched,
                        )
                        .await?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Search within a single file
    async fn search_file(
        &self,
        path: &std::path::PathBuf,
        pattern: &str,
        max_results: usize,
        matches: &mut Vec<SearchMatch>,
    ) -> Result<()> {
        let content = tokio::fs::read_to_string(path).await.ok();

        if let Some(content) = content {
            for (line_num, line) in content.lines().enumerate() {
                if matches.len() >= max_results {
                    break;
                }

                if let Some(pos) = line.find(pattern) {
                    matches.push(SearchMatch {
                        path: path.to_string_lossy().to_string(),
                        line_number: line_num + 1,
                        line_content: line.to_string(),
                        match_start: pos,
                        match_end: pos + pattern.len(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Simple glob pattern matching
    fn matches_glob(&self, text: &str, pattern: &str) -> bool {
        if pattern == "*" {
            return true;
        }

        if pattern.starts_with("*.") {
            let ext = &pattern[2..];
            return text.ends_with(&format!(".{}", ext));
        }

        if pattern.ends_with('*') {
            let prefix = &pattern[..pattern.len() - 1];
            return text.starts_with(prefix);
        }

        text == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_match_serialization() {
        let m = SearchMatch {
            path: "/tmp/test.rs".to_string(),
            line_number: 42,
            line_content: "fn main() {".to_string(),
            match_start: 0,
            match_end: 2,
        };

        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"line_number\":42"));
    }

    #[test]
    fn test_matches_glob() {
        use crate::filesystem::config::FilesystemConfig;

        let guard = Arc::new(PermissionGuard::new(FilesystemConfig::default()));
        let searcher = FileSearcher::new(guard);

        assert!(searcher.matches_glob("test.rs", "*.rs"));
        assert!(searcher.matches_glob("main.rs", "*.rs"));
        assert!(!searcher.matches_glob("test.py", "*.rs"));
        assert!(searcher.matches_glob("anything", "*"));
    }
}
