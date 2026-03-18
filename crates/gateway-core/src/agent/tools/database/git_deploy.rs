//! GitOps Database Deployment Tool
//!
//! Deploys database schema from a Git repository by:
//! 1. Fetching migration files from the repo (via platform-service)
//! 2. Sorting them by filename (numbered: 001_, 002_, etc.)
//! 3. Checking which are already applied
//! 4. Applying pending migrations in order
//! 5. Returning a deployment report

use super::{validate_path_id, DatabaseToolConfig};
use crate::agent::tools::{AgentTool, ToolContext, ToolError, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;

// =============================================================================
// Types
// =============================================================================

/// Input for deploying database schema from a Git repository.
#[derive(Debug, Deserialize)]
pub struct GitDeployInput {
    /// The app ID whose database to deploy to.
    pub app_id: String,
    /// GitHub repository URL (e.g., "https://github.com/user/repo").
    pub git_repo_url: String,
    /// Branch to use (default: "main").
    #[serde(default = "default_main")]
    pub git_branch: String,
    /// Path within the repo where migration SQL files live (default: "migrations/").
    #[serde(default = "default_migrations_path")]
    pub migrations_path: String,
    /// Git token for private repos (GitHub PAT). Optional for public repos.
    #[serde(default)]
    pub git_token: Option<String>,
}

fn default_main() -> String {
    "main".to_string()
}

fn default_migrations_path() -> String {
    "migrations/".to_string()
}

/// Output of a GitOps database deployment.
#[derive(Debug, Serialize)]
pub struct GitDeployOutput {
    /// Names of migrations that were successfully applied.
    pub applied: Vec<String>,
    /// Names of migrations that were already applied (skipped).
    pub skipped: Vec<String>,
    /// Migrations that failed to apply.
    pub failed: Vec<GitDeployFailure>,
    /// Total count of applied migrations.
    pub total_applied: usize,
    /// Total count of skipped migrations.
    pub total_skipped: usize,
    /// Total count of failed migrations.
    pub total_failed: usize,
}

/// Details about a failed migration during GitOps deploy.
#[derive(Debug, Serialize)]
pub struct GitDeployFailure {
    /// The migration file name that failed.
    pub migration_name: String,
    /// The error that occurred.
    pub error: String,
}

/// A migration file parsed from the repository listing.
#[derive(Debug, Clone)]
struct MigrationFile {
    /// File name (e.g., "001_create_users.sql")
    name: String,
    /// SQL content of the file
    sql: String,
}

// =============================================================================
// DatabaseGitDeployTool
// =============================================================================

/// Deploy database schema from a GitHub repository by applying pending
/// migration SQL files in order.
pub struct DatabaseGitDeployTool {
    config: Arc<DatabaseToolConfig>,
}

impl DatabaseGitDeployTool {
    pub fn new(config: Arc<DatabaseToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DatabaseGitDeployTool {
    type Input = GitDeployInput;
    type Output = GitDeployOutput;

    fn name(&self) -> &str {
        "database_git_deploy"
    }

    fn description(&self) -> &str {
        "Deploy database schema from a GitHub repository. Clones the repo, finds SQL migration \
         files in the specified directory (default: 'migrations/'), sorts them by filename \
         (numbered: 001_, 002_, etc.), checks which are already applied, and applies pending \
         migrations in order. Each migration is recorded in the tracking table. If a migration \
         fails, the error is reported but remaining migrations are still attempted. \
         For private repos, provide a git_token (GitHub PAT). \
         \n\nThe migrations directory should contain .sql files named with a numeric prefix: \
         001_create_users.sql, 002_add_email_index.sql, etc."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": {
                    "type": "string",
                    "description": "The app ID (UUID) whose database to deploy schema to"
                },
                "git_repo_url": {
                    "type": "string",
                    "description": "GitHub repository URL (e.g., https://github.com/user/repo)"
                },
                "git_branch": {
                    "type": "string",
                    "description": "Branch to deploy from (default: main)"
                },
                "migrations_path": {
                    "type": "string",
                    "description": "Path within the repo where SQL migration files are stored (default: migrations/)"
                },
                "git_token": {
                    "type": "string",
                    "description": "GitHub Personal Access Token for private repos. Optional for public repos."
                }
            },
            "required": ["app_id", "git_repo_url"]
        })
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn is_mutating(&self) -> bool {
        true
    }

    fn namespace(&self) -> &str {
        "database"
    }

    async fn execute(
        &self,
        input: Self::Input,
        _context: &ToolContext,
    ) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;

        if input.git_repo_url.is_empty() {
            return Err(ToolError::InvalidInput(
                "git_repo_url cannot be empty".to_string(),
            ));
        }

        // Step 1: Ensure migrations tracking table exists
        super::ensure_migrations_table(&self.config, &input.app_id).await?;

        // Step 2: Fetch migration files from the repository via platform-service
        let migration_files = self.fetch_migration_files(&input).await?;

        if migration_files.is_empty() {
            return Ok(GitDeployOutput {
                applied: vec![],
                skipped: vec![],
                failed: vec![GitDeployFailure {
                    migration_name: "(none)".to_string(),
                    error: format!(
                        "No .sql files found in '{}' directory of {}@{}",
                        input.migrations_path, input.git_repo_url, input.git_branch
                    ),
                }],
                total_applied: 0,
                total_skipped: 0,
                total_failed: 1,
            });
        }

        // Step 3: Get already-applied migration names
        let applied_names = self.get_applied_migration_names(&input.app_id).await?;

        // Step 4: Apply pending migrations in order
        let mut applied = Vec::new();
        let mut skipped = Vec::new();
        let mut failed = Vec::new();

        for migration in &migration_files {
            // Strip .sql extension for the migration name
            let migration_name = migration
                .name
                .strip_suffix(".sql")
                .unwrap_or(&migration.name)
                .to_string();

            // Skip if already applied (and not rolled back)
            if applied_names.contains(&migration_name) {
                skipped.push(migration_name);
                continue;
            }

            // Apply the migration
            match self
                .apply_single_migration(&input.app_id, &migration_name, &migration.sql)
                .await
            {
                Ok(()) => {
                    applied.push(migration_name);
                }
                Err(e) => {
                    failed.push(GitDeployFailure {
                        migration_name,
                        error: e.to_string(),
                    });
                    // Continue with remaining migrations — don't stop on failure
                }
            }
        }

        let total_applied = applied.len();
        let total_skipped = skipped.len();
        let total_failed = failed.len();

        Ok(GitDeployOutput {
            applied,
            skipped,
            failed,
            total_applied,
            total_skipped,
            total_failed,
        })
    }
}

impl DatabaseGitDeployTool {
    /// Fetch migration SQL files from the repository via the platform-service API.
    ///
    /// Calls the hosting API's repo file listing and content retrieval endpoints.
    /// Returns migration files sorted by filename.
    async fn fetch_migration_files(
        &self,
        input: &GitDeployInput,
    ) -> Result<Vec<MigrationFile>, ToolError> {
        // Call platform-service to list and fetch migration files from the repo
        let mut body = serde_json::json!({
            "git_repo": input.git_repo_url,
            "git_branch": input.git_branch,
            "path": input.migrations_path,
            "file_pattern": "*.sql",
        });
        if let Some(token) = &input.git_token {
            body["git_token"] = serde_json::json!(token);
        }

        let result = super::database_request(
            &self.config,
            "POST",
            "/api/hosting/repo/files",
            Some(body),
        )
        .await?;

        // Parse the response — expected format:
        // { "files": [{ "name": "001_create_users.sql", "content": "CREATE TABLE ..." }, ...] }
        let files = result
            .get("files")
            .and_then(|f| f.as_array())
            .ok_or_else(|| {
                ToolError::ExecutionError(
                    "Unexpected response format from repo files endpoint".to_string(),
                )
            })?;

        let mut migration_files: Vec<MigrationFile> = files
            .iter()
            .filter_map(|f| {
                let name = f.get("name")?.as_str()?.to_string();
                let content = f.get("content")?.as_str()?.to_string();
                if name.ends_with(".sql") && !content.trim().is_empty() {
                    Some(MigrationFile {
                        name,
                        sql: content,
                    })
                } else {
                    None
                }
            })
            .collect();

        // Sort by filename (lexicographic — works with numbered prefixes like 001_, 002_)
        migration_files.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(migration_files)
    }

    /// Get the set of migration names that have been applied and not rolled back.
    async fn get_applied_migration_names(
        &self,
        app_id: &str,
    ) -> Result<HashSet<String>, ToolError> {
        let sql = "SELECT name FROM _canal_migrations WHERE rolled_back_at IS NULL";

        match super::execute_sql(&self.config, app_id, sql).await {
            Ok(result) => {
                let mut names = HashSet::new();
                let rows = result
                    .get("rows")
                    .or_else(|| result.get("data"))
                    .or_else(|| result.get("result"))
                    .and_then(|v| v.as_array());

                if let Some(rows) = rows {
                    for row in rows {
                        if let Some(name) = row
                            .get("name")
                            .and_then(|v| v.as_str())
                            .or_else(|| row.as_array().and_then(|a| a.first()?.as_str()))
                        {
                            names.insert(name.to_string());
                        }
                    }
                }
                Ok(names)
            }
            Err(_) => {
                // Table might not exist yet — treat as empty
                Ok(HashSet::new())
            }
        }
    }

    /// Apply a single migration and record it in the tracking table.
    async fn apply_single_migration(
        &self,
        app_id: &str,
        migration_name: &str,
        sql: &str,
    ) -> Result<(), ToolError> {
        // Execute the migration SQL
        super::execute_sql(&self.config, app_id, sql).await?;

        // Record in tracking table
        let checksum = format!("{:x}", Sha256::digest(sql.as_bytes()));
        let record_sql = format!(
            "INSERT INTO _canal_migrations (name, checksum, source) VALUES ('{}', '{}', 'gitops')",
            migration_name.replace('\'', "''"),
            checksum
        );

        super::execute_sql(&self.config, app_id, &record_sql).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_deploy_tool_metadata() {
        let config = Arc::new(DatabaseToolConfig::new("http://localhost:8080", "token"));
        let tool = DatabaseGitDeployTool::new(config);
        assert_eq!(tool.name(), "database_git_deploy");
        assert!(tool.requires_permission());
        assert!(tool.is_mutating());
        assert_eq!(tool.namespace(), "database");
    }

    #[test]
    fn test_default_values() {
        assert_eq!(default_main(), "main");
        assert_eq!(default_migrations_path(), "migrations/");
    }

    #[test]
    fn test_git_deploy_input_defaults() {
        let json = serde_json::json!({
            "app_id": "test-app",
            "git_repo_url": "https://github.com/user/repo"
        });
        let input: GitDeployInput = serde_json::from_value(json).expect("should deserialize");
        assert_eq!(input.app_id, "test-app");
        assert_eq!(input.git_branch, "main");
        assert_eq!(input.migrations_path, "migrations/");
        assert!(input.git_token.is_none());
    }

    #[test]
    fn test_git_deploy_output_serialization() {
        let output = GitDeployOutput {
            applied: vec!["001_create_users".to_string()],
            skipped: vec!["000_init".to_string()],
            failed: vec![GitDeployFailure {
                migration_name: "002_bad".to_string(),
                error: "syntax error".to_string(),
            }],
            total_applied: 1,
            total_skipped: 1,
            total_failed: 1,
        };
        let json = serde_json::to_value(&output).expect("should serialize");
        assert_eq!(json["total_applied"], 1);
        assert_eq!(json["total_skipped"], 1);
        assert_eq!(json["total_failed"], 1);
        assert_eq!(json["failed"][0]["error"], "syntax error");
    }

    #[test]
    fn test_input_schema_has_required_fields() {
        let config = Arc::new(DatabaseToolConfig::new("http://localhost:8080", "token"));
        let tool = DatabaseGitDeployTool::new(config);
        let schema = tool.input_schema();
        let required = schema["required"].as_array().expect("required should be array");
        let required_strs: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(required_strs.contains(&"app_id"));
        assert!(required_strs.contains(&"git_repo_url"));
    }
}
