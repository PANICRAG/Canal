//! Database Agent Tools — CRUD and SQL operations on user databases
//!
//! 7 agent tools for managing per-user databases via the Platform Service
//! database API under `/api/hosting/apps/{app_id}/db/`.

use super::types::*;
use super::{database_request, validate_path_id, validate_sql, DatabaseToolConfig};
use crate::agent::tools::traits::{AgentTool, ToolResult};
use async_trait::async_trait;
use gateway_tool_types::{ToolContext, ToolError};
use std::sync::Arc;

// =============================================================================
// 1. DatabaseListTablesTool
// =============================================================================

pub struct DatabaseListTablesTool {
    config: Arc<DatabaseToolConfig>,
}

impl DatabaseListTablesTool {
    pub fn new(config: Arc<DatabaseToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DatabaseListTablesTool {
    type Input = ListTablesInput;
    type Output = ListTablesOutput;

    fn name(&self) -> &str {
        "database_list_tables"
    }

    fn description(&self) -> &str {
        "List all tables in the user's database. Returns table names, column definitions, \
         primary keys, and foreign key relationships. Use this when the user asks about \
         their database structure or before writing queries to understand the schema."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": {
                    "type": "string",
                    "description": "The application ID (UUID) that owns the database."
                },
                "schema": {
                    "type": "string",
                    "description": "Database schema name (default: 'public')."
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

    async fn execute(&self, input: Self::Input, _context: &ToolContext) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        let path = format!(
            "/api/hosting/apps/{}/db/tables?schema={}",
            input.app_id, input.schema
        );
        let result = database_request(&self.config, "GET", &path, None).await?;
        let tables = match result {
            serde_json::Value::Array(arr) => arr,
            other => vec![other],
        };
        Ok(ListTablesOutput { tables })
    }
}

// =============================================================================
// 2. DatabaseQueryRowsTool
// =============================================================================

pub struct DatabaseQueryRowsTool {
    config: Arc<DatabaseToolConfig>,
}

impl DatabaseQueryRowsTool {
    pub fn new(config: Arc<DatabaseToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DatabaseQueryRowsTool {
    type Input = QueryRowsInput;
    type Output = QueryRowsOutput;

    fn name(&self) -> &str {
        "database_query_rows"
    }

    fn description(&self) -> &str {
        "Query rows from a database table with pagination, sorting, and filtering. \
         For complex queries with JOINs or aggregations, use database_execute_sql instead."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": { "type": "string", "description": "App ID (UUID)" },
                "table": { "type": "string", "description": "Table name" },
                "page": { "type": "integer", "description": "Page number (1-based, default: 1)" },
                "per_page": { "type": "integer", "description": "Rows per page (default: 50, max: 1000)" },
                "order": { "type": "string", "description": "Column to order by" },
                "sort": { "type": "string", "enum": ["asc", "desc"], "description": "Sort direction" },
                "filters": { "type": "array", "items": { "type": "string" }, "description": "PostgREST filters (e.g. ['age=gt.18'])" }
            },
            "required": ["app_id", "table"]
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

    async fn execute(&self, input: Self::Input, _context: &ToolContext) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        validate_path_id(&input.table, "table")?;

        let page = input.page.unwrap_or(1).max(1);
        let per_page = input.per_page.unwrap_or(50).min(1000).max(1);

        let mut path = format!(
            "/api/hosting/apps/{}/db/data/{}?page={}&per_page={}",
            input.app_id, input.table, page, per_page
        );
        if let Some(ref order) = input.order {
            path.push_str(&format!("&order={}", order));
        }
        if let Some(ref sort) = input.sort {
            path.push_str(&format!("&sort={}", sort));
        }

        let result = database_request(&self.config, "GET", &path, None).await?;

        let rows = result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_else(|| match &result {
                serde_json::Value::Array(a) => a.clone(),
                _ => vec![result.clone()],
            });
        let total_count = result
            .get("total_count")
            .or_else(|| result.get("total"))
            .and_then(|v| v.as_i64());

        Ok(QueryRowsOutput { rows, total_count })
    }
}

// =============================================================================
// 3. DatabaseExecuteSqlTool
// =============================================================================

pub struct DatabaseExecuteSqlTool {
    config: Arc<DatabaseToolConfig>,
}

impl DatabaseExecuteSqlTool {
    pub fn new(config: Arc<DatabaseToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DatabaseExecuteSqlTool {
    type Input = ExecuteSqlInput;
    type Output = ExecuteSqlOutput;

    fn name(&self) -> &str {
        "database_execute_sql"
    }

    fn description(&self) -> &str {
        "Execute raw SQL against the user's database. Supports SELECT, DDL (CREATE/ALTER/DROP), \
         and DML (INSERT/UPDATE/DELETE). WARNING: Can modify or destroy data — confirm destructive \
         operations with the user first."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": { "type": "string", "description": "App ID (UUID)" },
                "sql": { "type": "string", "description": "SQL statement to execute" }
            },
            "required": ["app_id", "sql"]
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

    async fn execute(&self, input: Self::Input, _context: &ToolContext) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        validate_sql(&input.sql)?;

        let path = format!("/api/hosting/apps/{}/db/sql/execute", input.app_id);
        let body = serde_json::json!({ "sql": input.sql.trim() });
        let result = database_request(&self.config, "POST", &path, Some(body)).await?;

        let rows = result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let row_count = result.get("row_count").and_then(|v| v.as_i64()).unwrap_or(rows.len() as i64);
        let duration_ms = result.get("duration_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let error = result.get("error").and_then(|v| v.as_str()).map(String::from);

        Ok(ExecuteSqlOutput {
            rows,
            row_count,
            duration_ms,
            error,
        })
    }
}

// =============================================================================
// 4. DatabaseCreateTableTool
// =============================================================================

pub struct DatabaseCreateTableTool {
    config: Arc<DatabaseToolConfig>,
}

impl DatabaseCreateTableTool {
    pub fn new(config: Arc<DatabaseToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DatabaseCreateTableTool {
    type Input = CreateTableInput;
    type Output = CreateTableOutput;

    fn name(&self) -> &str {
        "database_create_table"
    }

    fn description(&self) -> &str {
        "Create a new table with structured column definitions. For complex DDL (constraints, \
         indexes, triggers), use database_execute_sql instead."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": { "type": "string", "description": "App ID (UUID)" },
                "name": { "type": "string", "description": "Table name" },
                "schema": { "type": "string", "description": "Schema (default: 'public')" },
                "columns": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "data_type": { "type": "string", "description": "PostgreSQL type (text, integer, uuid, etc.)" },
                            "is_primary_key": { "type": "boolean" },
                            "is_nullable": { "type": "boolean" },
                            "is_unique": { "type": "boolean" },
                            "default_value": { "type": "string" },
                            "foreign_key": { "type": "string", "description": "e.g. 'other_table.id'" }
                        },
                        "required": ["name", "data_type"]
                    },
                    "minItems": 1
                },
                "comment": { "type": "string" }
            },
            "required": ["app_id", "name", "columns"]
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

    async fn execute(&self, input: Self::Input, _context: &ToolContext) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        if input.name.is_empty() {
            return Err(ToolError::InvalidInput("Table name cannot be empty".into()));
        }
        if input.columns.is_empty() {
            return Err(ToolError::InvalidInput("At least one column required".into()));
        }

        let path = format!("/api/hosting/apps/{}/db/tables", input.app_id);
        let body = serde_json::json!({
            "name": input.name,
            "schema": input.schema,
            "columns": input.columns,
            "comment": input.comment,
        });

        let result = database_request(&self.config, "POST", &path, Some(body)).await?;
        Ok(CreateTableOutput { table: result })
    }
}

// =============================================================================
// 5. DatabaseDropTableTool
// =============================================================================

pub struct DatabaseDropTableTool {
    config: Arc<DatabaseToolConfig>,
}

impl DatabaseDropTableTool {
    pub fn new(config: Arc<DatabaseToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DatabaseDropTableTool {
    type Input = DropTableInput;
    type Output = DropTableOutput;

    fn name(&self) -> &str {
        "database_drop_table"
    }

    fn description(&self) -> &str {
        "Drop (permanently delete) a table. This is destructive and cannot be undone. \
         Always confirm with the user before dropping."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": { "type": "string", "description": "App ID (UUID)" },
                "table_id": { "type": "integer", "description": "Table ID (numeric) to drop" },
                "cascade": { "type": "boolean", "description": "Also drop dependent objects (default: false)" }
            },
            "required": ["app_id", "table_id"]
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

    async fn execute(&self, input: Self::Input, _context: &ToolContext) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        let path = format!(
            "/api/hosting/apps/{}/db/tables/{}?cascade={}",
            input.app_id, input.table_id, input.cascade
        );
        database_request(&self.config, "DELETE", &path, None).await?;
        Ok(DropTableOutput { success: true })
    }
}

// =============================================================================
// 6. DatabaseSchemaContextTool
// =============================================================================

pub struct DatabaseSchemaContextTool {
    config: Arc<DatabaseToolConfig>,
}

impl DatabaseSchemaContextTool {
    pub fn new(config: Arc<DatabaseToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DatabaseSchemaContextTool {
    type Input = SchemaContextInput;
    type Output = SchemaContextOutput;

    fn name(&self) -> &str {
        "database_schema_context"
    }

    fn description(&self) -> &str {
        "Get a comprehensive schema context optimized for AI understanding. Returns DDL text, \
         table count, and column count. Use at the start of a database conversation."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": { "type": "string", "description": "App ID (UUID)" }
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

    async fn execute(&self, input: Self::Input, _context: &ToolContext) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        let path = format!("/api/hosting/apps/{}/db/ai/schema-context", input.app_id);
        let result = database_request(&self.config, "GET", &path, None).await?;

        Ok(SchemaContextOutput {
            ddl_text: result.get("ddl_text").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            table_count: result.get("table_count").and_then(|v| v.as_i64()).unwrap_or(0),
            column_count: result.get("column_count").and_then(|v| v.as_i64()).unwrap_or(0),
        })
    }
}

// =============================================================================
// 7. DatabaseExplainSqlTool
// =============================================================================

pub struct DatabaseExplainSqlTool {
    config: Arc<DatabaseToolConfig>,
}

impl DatabaseExplainSqlTool {
    pub fn new(config: Arc<DatabaseToolConfig>) -> Self {
        Self { config }
    }
}

#[async_trait]
impl AgentTool for DatabaseExplainSqlTool {
    type Input = ExplainSqlInput;
    type Output = ExplainSqlOutput;

    fn name(&self) -> &str {
        "database_explain_sql"
    }

    fn description(&self) -> &str {
        "Explain a SQL query's execution plan. Use to debug slow queries or verify index usage."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "app_id": { "type": "string", "description": "App ID (UUID)" },
                "sql": { "type": "string", "description": "SQL query to explain" },
                "analyze": { "type": "boolean", "description": "Run EXPLAIN ANALYZE (default: true)" }
            },
            "required": ["app_id", "sql"]
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

    async fn execute(&self, input: Self::Input, _context: &ToolContext) -> ToolResult<Self::Output> {
        validate_path_id(&input.app_id, "app_id")?;
        validate_sql(&input.sql)?;

        let path = format!("/api/hosting/apps/{}/db/sql/explain", input.app_id);
        let body = serde_json::json!({
            "sql": input.sql.trim(),
            "analyze": input.analyze,
        });
        let result = database_request(&self.config, "POST", &path, Some(body)).await?;

        Ok(ExplainSqlOutput {
            plan: result.get("plan").cloned().unwrap_or(serde_json::Value::Null),
            raw_text: result.get("raw_text").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            duration_ms: result.get("duration_ms").and_then(|v| v.as_f64()).unwrap_or(0.0),
            error: result.get("error").and_then(|v| v.as_str()).map(String::from),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Arc<DatabaseToolConfig> {
        Arc::new(DatabaseToolConfig::new("http://localhost:8080", "t"))
    }

    #[test]
    fn test_tool_metadata() {
        let tools: Vec<(Box<dyn std::any::Any>, &str, bool, bool)> = vec![
            (Box::new(DatabaseListTablesTool::new(cfg())), "database_list_tables", false, false),
            (Box::new(DatabaseQueryRowsTool::new(cfg())), "database_query_rows", false, false),
            (Box::new(DatabaseExecuteSqlTool::new(cfg())), "database_execute_sql", true, true),
            (Box::new(DatabaseCreateTableTool::new(cfg())), "database_create_table", true, true),
            (Box::new(DatabaseDropTableTool::new(cfg())), "database_drop_table", true, true),
            (Box::new(DatabaseSchemaContextTool::new(cfg())), "database_schema_context", false, false),
            (Box::new(DatabaseExplainSqlTool::new(cfg())), "database_explain_sql", false, false),
        ];
        assert_eq!(tools.len(), 7);
    }
}
