//! Database Migration Tools — List, apply, and rollback migrations
//!
//! Tracks applied migrations in a `_canal_migrations` table within
//! the app's managed database. All SQL execution goes through the
//! platform-service hosting API.

use super::{validate_path_id, DatabaseToolConfig};
use crate::agent::tools::{AgentTool, ToolContext, ToolError, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

// =============================================================================
// Types
// =============================================================================

/// Input for listing applied migrations.
#[derive(Debug, Deserialize)]
pub struct MigrationListInput {
    /// The app ID whose database to query.
    pub app_id: String,
}

/// Output of listing applied migrations.
#[derive(Debug, Serialize)]
pub struct MigrationListOutput {
    /// All migration records, ordered by execution time ascending.
    pub migrations: Vec<MigrationRecord>,
    /// Informational message (e.g., if table did not exist yet).
    pub message: Option<String>,
}

/// A single migration record from the tracking table.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MigrationRecord {
    /// Migration name (unique identifier, e.g., "001_create_users").
    pub name: String,
    /// SHA-256 checksum of the SQL content.
    pub checksum: String,
    /// ISO-8601 timestamp when the migration was applied.
    pub executed_at: String,
    /// ISO-8601 timestamp if/when the migration was rolled back.
    pub rolled_back_at: Option<String>,
    /// How the migration was applied: "agent", "gitops", "manual".
    pub source: String,
}

/// Input for applying a migration.
#[derive(Debug, Deserialize)]
pub struct MigrationApplyInput {
    /// The app ID whose database to modify.
    pub app_id: String,
    /// Name for this migration (e.g., "001_create_users").
    pub migration_name: String,
    /// The SQL to execute.
    pub sql: String,
}

/// Output of applying a migration.
#[derive(Debug, Serialize)]
pub struct MigrationApplyOutput {
    /// Whether the migration was applied successfully.
    pub success: bool,
    /// The migration name.
    pub migration_name: String,
    /// Number of rows affected by the migration SQL.
    pub rows_affected: i64,
    /// Time taken in milliseconds.
    pub duration_ms: f64,
    /// Error message if the migration failed.
    pub error: Option<String>,
}

/// Input for rolling back a migration.
#[derive(Debug, Deserialize)]
pub struct MigrationRollbackInput {
    /// The app ID whose database to modify.
    pub app_id: String,
    /// The name of the migration to roll back.
    pub migration_name: String,
    /// The SQL to undo the migration (e.g., DROP TABLE ...).
    pub rollback_sql: String,
}

/// Output of rolling back a migration.
#[derive(Debug, Serialize)]
pub struct MigrationRollbackOutput {
    /// Whether the rollback was applied successfully.
    pub success: bool,
    /// The migration name that was rolled back.
    pub migration_name: String,
    /// Error message if the rollback failed.
    pub error: Option<String>,
}

// =============================================================================
// Helpers
// =============================================================================

/// Compute SHA-256 checksum of SQL content.
fn compute_checksum(sql: &str) -> String {
    format!("{:x}", Sha256::digest(sql.as_bytes()))
}

// =============================================================================
// DatabaseMigrationListTool
// =============================================================================

/// List all applied migrations from the `_canal_migrations` table.
pub struct DatabaseMigrationListTool {
    config: Arc<DatabaseToolConfig>,
}

impl DatabaseMigrationListTool {
    pub fn new(config: Arc<DatabaseToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DatabaseMigrationListTool {
    type Input = MigrationListInput;
    type Output = MigrationListOutput;

    fn name(&self) -> &str {
        "database_migration_list"
    }

    fn description(&self) -> &str {
        "List all applied database migrations for an app, ordered by execution time. \
         Shows each migration's name, checksum, when it was applied, and whether it \
         has been rolled back. Use this to understand the current schema state before \
         applying new migrations or performing a rollback."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": {
                    "type": "string",
                    "description": "The app ID (UUID) whose database migrations to list"
                }
            },
            "required": ["app_id"]
        })
    }

    fn requires_permission(&self) -> bool {
        false
    }

    fn is_mutating(&self) -> bool {
        false
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

        let sql = "SELECT name, checksum, executed_at::text, rolled_back_at::text, source \
                   FROM _canal_migrations ORDER BY executed_at ASC";

        match super::execute_sql(&self.config, &input.app_id, sql).await {
            Ok(result) => {
                let migrations = parse_migration_rows(&result);
                Ok(MigrationListOutput {
                    migrations,
                    message: None,
                })
            }
            Err(e) => {
                let err_str = e.to_string();
                // If the table doesn't exist, return empty list with informational message
                if err_str.contains("does not exist")
                    || err_str.contains("relation")
                    || err_str.contains("_canal_migrations")
                {
                    Ok(MigrationListOutput {
                        migrations: vec![],
                        message: Some(
                            "No migrations table found. It will be created automatically \
                             when you apply the first migration."
                                .to_string(),
                        ),
                    })
                } else {
                    Err(e)
                }
            }
        }
    }
}

/// Parse migration records from the SQL execution response JSON.
fn parse_migration_rows(result: &serde_json::Value) -> Vec<MigrationRecord> {
    // The SQL execute endpoint returns rows in various formats;
    // handle both array-of-objects and array-of-arrays patterns.
    let rows = result
        .get("rows")
        .or_else(|| result.get("data"))
        .or_else(|| result.get("result"));

    let Some(rows) = rows else {
        // Try interpreting the entire result as an array
        if let Some(arr) = result.as_array() {
            return arr.iter().filter_map(parse_migration_row).collect();
        }
        return vec![];
    };

    if let Some(arr) = rows.as_array() {
        arr.iter().filter_map(parse_migration_row).collect()
    } else {
        vec![]
    }
}

fn parse_migration_row(row: &serde_json::Value) -> Option<MigrationRecord> {
    // Object format: {"name": "...", "checksum": "...", ...}
    if let Some(obj) = row.as_object() {
        Some(MigrationRecord {
            name: obj.get("name")?.as_str()?.to_string(),
            checksum: obj
                .get("checksum")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            executed_at: obj
                .get("executed_at")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            rolled_back_at: obj
                .get("rolled_back_at")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            source: obj
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("agent")
                .to_string(),
        })
    } else if let Some(arr) = row.as_array() {
        // Array format: ["name", "checksum", "executed_at", "rolled_back_at", "source"]
        if arr.len() >= 3 {
            Some(MigrationRecord {
                name: arr[0].as_str().unwrap_or("").to_string(),
                checksum: arr[1].as_str().unwrap_or("").to_string(),
                executed_at: arr[2].as_str().unwrap_or("").to_string(),
                rolled_back_at: arr.get(3).and_then(|v| v.as_str()).map(|s| s.to_string()),
                source: arr
                    .get(4)
                    .and_then(|v| v.as_str())
                    .unwrap_or("agent")
                    .to_string(),
            })
        } else {
            None
        }
    } else {
        None
    }
}

// =============================================================================
// DatabaseMigrationApplyTool
// =============================================================================

/// Apply a named migration (SQL) and record it in the tracking table.
pub struct DatabaseMigrationApplyTool {
    config: Arc<DatabaseToolConfig>,
}

impl DatabaseMigrationApplyTool {
    pub fn new(config: Arc<DatabaseToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DatabaseMigrationApplyTool {
    type Input = MigrationApplyInput;
    type Output = MigrationApplyOutput;

    fn name(&self) -> &str {
        "database_migration_apply"
    }

    fn description(&self) -> &str {
        "Apply a SQL migration to an app's database and record it in the migration tracking table. \
         The migration is identified by a unique name (e.g., '001_create_users'). Provide the \
         full SQL to execute. The tool auto-creates the tracking table if it doesn't exist yet. \
         Each migration name can only be applied once — use rollback first if you need to re-apply."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": {
                    "type": "string",
                    "description": "The app ID (UUID) whose database to modify"
                },
                "migration_name": {
                    "type": "string",
                    "description": "Unique name for this migration (e.g., '001_create_users', '002_add_email_index')"
                },
                "sql": {
                    "type": "string",
                    "description": "The SQL statements to execute for this migration"
                }
            },
            "required": ["app_id", "migration_name", "sql"]
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

        if input.migration_name.is_empty() {
            return Err(ToolError::InvalidInput(
                "migration_name cannot be empty".to_string(),
            ));
        }
        if input.sql.trim().is_empty() {
            return Err(ToolError::InvalidInput(
                "sql cannot be empty".to_string(),
            ));
        }

        let start = std::time::Instant::now();

        // Step 1: Ensure migrations table exists
        super::ensure_migrations_table(&self.config, &input.app_id).await?;

        // Step 2: Execute the migration SQL
        let result = match super::execute_sql(&self.config, &input.app_id, &input.sql).await {
            Ok(r) => r,
            Err(e) => {
                return Ok(MigrationApplyOutput {
                    success: false,
                    migration_name: input.migration_name,
                    rows_affected: 0,
                    duration_ms: start.elapsed().as_secs_f64() * 1000.0,
                    error: Some(format!("Migration SQL failed: {}", e)),
                });
            }
        };

        // Step 3: Record in migrations table
        let checksum = compute_checksum(&input.sql);
        let record_sql = format!(
            "INSERT INTO _canal_migrations (name, checksum, source) VALUES ('{}', '{}', 'agent')",
            input.migration_name.replace('\'', "''"),
            checksum
        );

        if let Err(e) = super::execute_sql(&self.config, &input.app_id, &record_sql).await {
            return Ok(MigrationApplyOutput {
                success: false,
                migration_name: input.migration_name,
                rows_affected: 0,
                duration_ms: start.elapsed().as_secs_f64() * 1000.0,
                error: Some(format!(
                    "Migration SQL succeeded but failed to record in tracking table: {}",
                    e
                )),
            });
        }

        let rows_affected = result
            .get("rows_affected")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        Ok(MigrationApplyOutput {
            success: true,
            migration_name: input.migration_name,
            rows_affected,
            duration_ms: start.elapsed().as_secs_f64() * 1000.0,
            error: None,
        })
    }
}

// =============================================================================
// DatabaseMigrationRollbackTool
// =============================================================================

/// Roll back a previously applied migration by executing rollback SQL
/// and marking the migration as rolled back in the tracking table.
pub struct DatabaseMigrationRollbackTool {
    config: Arc<DatabaseToolConfig>,
}

impl DatabaseMigrationRollbackTool {
    pub fn new(config: Arc<DatabaseToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DatabaseMigrationRollbackTool {
    type Input = MigrationRollbackInput;
    type Output = MigrationRollbackOutput;

    fn name(&self) -> &str {
        "database_migration_rollback"
    }

    fn description(&self) -> &str {
        "Roll back a previously applied database migration. Executes the provided rollback SQL \
         (e.g., DROP TABLE, ALTER TABLE DROP COLUMN) and marks the migration as rolled back \
         in the tracking table. You must provide the rollback SQL — this tool does not store \
         or auto-generate rollback scripts. Use database_migration_list first to see which \
         migrations have been applied."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": {
                    "type": "string",
                    "description": "The app ID (UUID) whose database to modify"
                },
                "migration_name": {
                    "type": "string",
                    "description": "The name of the migration to roll back (must match an applied migration)"
                },
                "rollback_sql": {
                    "type": "string",
                    "description": "The SQL statements to undo the migration (e.g., 'DROP TABLE users')"
                }
            },
            "required": ["app_id", "migration_name", "rollback_sql"]
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

        if input.migration_name.is_empty() {
            return Err(ToolError::InvalidInput(
                "migration_name cannot be empty".to_string(),
            ));
        }
        if input.rollback_sql.trim().is_empty() {
            return Err(ToolError::InvalidInput(
                "rollback_sql cannot be empty".to_string(),
            ));
        }

        // Step 1: Execute the rollback SQL
        if let Err(e) =
            super::execute_sql(&self.config, &input.app_id, &input.rollback_sql).await
        {
            return Ok(MigrationRollbackOutput {
                success: false,
                migration_name: input.migration_name,
                error: Some(format!("Rollback SQL failed: {}", e)),
            });
        }

        // Step 2: Mark migration as rolled back
        let update_sql = format!(
            "UPDATE _canal_migrations SET rolled_back_at = NOW() WHERE name = '{}'",
            input.migration_name.replace('\'', "''")
        );

        if let Err(e) = super::execute_sql(&self.config, &input.app_id, &update_sql).await {
            return Ok(MigrationRollbackOutput {
                success: false,
                migration_name: input.migration_name,
                error: Some(format!(
                    "Rollback SQL succeeded but failed to update tracking table: {}",
                    e
                )),
            });
        }

        Ok(MigrationRollbackOutput {
            success: true,
            migration_name: input.migration_name,
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_checksum() {
        let checksum = compute_checksum("CREATE TABLE users (id SERIAL PRIMARY KEY)");
        assert!(!checksum.is_empty());
        assert_eq!(checksum.len(), 64); // SHA-256 hex is 64 chars

        // Same input → same checksum
        let checksum2 = compute_checksum("CREATE TABLE users (id SERIAL PRIMARY KEY)");
        assert_eq!(checksum, checksum2);

        // Different input → different checksum
        let checksum3 = compute_checksum("DROP TABLE users");
        assert_ne!(checksum, checksum3);
    }

    #[test]
    fn test_parse_migration_row_object() {
        let row = serde_json::json!({
            "name": "001_create_users",
            "checksum": "abc123",
            "executed_at": "2026-03-14T10:00:00Z",
            "rolled_back_at": null,
            "source": "agent"
        });
        let record = parse_migration_row(&row).expect("should parse");
        assert_eq!(record.name, "001_create_users");
        assert_eq!(record.checksum, "abc123");
        assert!(record.rolled_back_at.is_none());
    }

    #[test]
    fn test_parse_migration_row_array() {
        let row = serde_json::json!([
            "002_add_index",
            "def456",
            "2026-03-14T11:00:00Z",
            null,
            "gitops"
        ]);
        let record = parse_migration_row(&row).expect("should parse");
        assert_eq!(record.name, "002_add_index");
        assert_eq!(record.source, "gitops");
    }

    #[test]
    fn test_parse_migration_rows_empty() {
        let result = serde_json::json!({"rows": []});
        let records = parse_migration_rows(&result);
        assert!(records.is_empty());
    }

    #[test]
    fn test_list_tool_metadata() {
        let config = Arc::new(DatabaseToolConfig::new("http://localhost:8080", "token"));
        let tool = DatabaseMigrationListTool::new(config);
        assert_eq!(tool.name(), "database_migration_list");
        assert!(!tool.requires_permission());
        assert!(!tool.is_mutating());
        assert_eq!(tool.namespace(), "database");
    }

    #[test]
    fn test_apply_tool_metadata() {
        let config = Arc::new(DatabaseToolConfig::new("http://localhost:8080", "token"));
        let tool = DatabaseMigrationApplyTool::new(config);
        assert_eq!(tool.name(), "database_migration_apply");
        assert!(tool.requires_permission());
        assert!(tool.is_mutating());
        assert_eq!(tool.namespace(), "database");
    }

    #[test]
    fn test_rollback_tool_metadata() {
        let config = Arc::new(DatabaseToolConfig::new("http://localhost:8080", "token"));
        let tool = DatabaseMigrationRollbackTool::new(config);
        assert_eq!(tool.name(), "database_migration_rollback");
        assert!(tool.requires_permission());
        assert!(tool.is_mutating());
        assert_eq!(tool.namespace(), "database");
    }
}
