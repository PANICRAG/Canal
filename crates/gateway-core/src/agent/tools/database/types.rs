//! Database Tool Types — Input/Output types for all database agent tools
//!
//! These types define the contract between the AI agent and the Platform Service
//! database API. All operations target a specific app's database via `app_id`.

use serde::{Deserialize, Serialize};

// =============================================================================
// Helpers
// =============================================================================

fn default_public() -> String {
    "public".to_string()
}

fn default_true() -> bool {
    true
}

// =============================================================================
// List Tables
// =============================================================================

/// Input for listing tables in an app's database.
#[derive(Debug, Deserialize)]
pub struct ListTablesInput {
    /// App ID identifying the target database
    pub app_id: String,
    /// Schema to list tables from (default: "public")
    #[serde(default = "default_public")]
    pub schema: String,
}

/// Output containing the list of tables.
#[derive(Debug, Serialize)]
pub struct ListTablesOutput {
    /// Array of table metadata objects
    pub tables: Vec<serde_json::Value>,
}

// =============================================================================
// Get Table Info
// =============================================================================

/// Input for getting detailed information about a specific table.
#[derive(Debug, Deserialize)]
pub struct GetTableInfoInput {
    /// App ID identifying the target database
    pub app_id: String,
    /// Table ID (numeric)
    pub table_id: i64,
}

/// Output containing table details (columns, constraints, etc.).
#[derive(Debug, Serialize)]
pub struct GetTableInfoOutput {
    /// Full table metadata including columns, indexes, constraints
    pub table: serde_json::Value,
}

// =============================================================================
// Query Rows
// =============================================================================

/// Input for querying rows from a table with optional filtering, sorting, and pagination.
#[derive(Debug, Deserialize)]
pub struct QueryRowsInput {
    /// App ID identifying the target database
    pub app_id: String,
    /// Table name to query
    pub table: String,
    /// Optional PostgREST-style filters (e.g., ["age=gt.18", "status=eq.active"])
    #[serde(default)]
    pub filters: Option<Vec<String>>,
    /// Page number for pagination (1-based)
    #[serde(default)]
    pub page: Option<u32>,
    /// Number of rows per page
    #[serde(default)]
    pub per_page: Option<u32>,
    /// Column name to order by
    #[serde(default)]
    pub order: Option<String>,
    /// Sort direction: "asc" or "desc"
    #[serde(default)]
    pub sort: Option<String>,
}

/// Output containing queried rows and optional total count.
#[derive(Debug, Serialize)]
pub struct QueryRowsOutput {
    /// Array of row objects
    pub rows: Vec<serde_json::Value>,
    /// Total count of matching rows (if available)
    pub total_count: Option<i64>,
}

// =============================================================================
// Insert Rows
// =============================================================================

/// Input for inserting one or more rows into a table.
#[derive(Debug, Deserialize)]
pub struct InsertRowsInput {
    /// App ID identifying the target database
    pub app_id: String,
    /// Table name to insert into
    pub table: String,
    /// Array of row objects to insert
    pub rows: Vec<serde_json::Value>,
}

/// Output after inserting rows.
#[derive(Debug, Serialize)]
pub struct InsertRowsOutput {
    /// The inserted rows (with server-generated fields like id, created_at)
    pub rows: Vec<serde_json::Value>,
    /// Number of rows inserted
    pub count: i64,
}

// =============================================================================
// Update Rows
// =============================================================================

/// Input for updating rows in a table.
#[derive(Debug, Deserialize)]
pub struct UpdateRowsInput {
    /// App ID identifying the target database
    pub app_id: String,
    /// Table name to update
    pub table: String,
    /// Column values to set (e.g., {"status": "active", "updated_at": "now()"})
    pub values: serde_json::Value,
    /// PostgREST-style filters to select rows to update (e.g., ["id=eq.42"])
    pub filters: Vec<String>,
}

/// Output after updating rows.
#[derive(Debug, Serialize)]
pub struct UpdateRowsOutput {
    /// The updated rows
    pub rows: Vec<serde_json::Value>,
    /// Number of rows affected
    pub count: i64,
}

// =============================================================================
// Delete Rows
// =============================================================================

/// Input for deleting rows from a table.
#[derive(Debug, Deserialize)]
pub struct DeleteRowsInput {
    /// App ID identifying the target database
    pub app_id: String,
    /// Table name to delete from
    pub table: String,
    /// PostgREST-style filters to select rows to delete (e.g., ["id=eq.42"])
    pub filters: Vec<String>,
}

/// Output after deleting rows.
#[derive(Debug, Serialize)]
pub struct DeleteRowsOutput {
    /// Number of rows deleted
    pub count: i64,
}

// =============================================================================
// Create Table
// =============================================================================

/// Input for creating a new table in the database.
#[derive(Debug, Deserialize)]
pub struct CreateTableInput {
    /// App ID identifying the target database
    pub app_id: String,
    /// Table name
    pub name: String,
    /// Schema to create the table in (default: "public")
    #[serde(default = "default_public")]
    pub schema: String,
    /// Column definitions
    pub columns: Vec<CreateColumnDef>,
    /// Optional table comment
    #[serde(default)]
    pub comment: Option<String>,
}

/// Column definition for table creation.
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateColumnDef {
    /// Column name
    pub name: String,
    /// PostgreSQL data type (e.g., "text", "integer", "timestamptz", "uuid")
    pub data_type: String,
    /// Whether this column is (part of) the primary key
    #[serde(default)]
    pub is_primary_key: bool,
    /// Whether the column allows NULL values (default: true)
    #[serde(default = "default_true")]
    pub is_nullable: bool,
    /// Whether the column has a UNIQUE constraint
    #[serde(default)]
    pub is_unique: bool,
    /// Whether the column is an identity column (auto-incrementing)
    #[serde(default)]
    pub is_identity: bool,
    /// Default value expression (e.g., "now()", "gen_random_uuid()")
    #[serde(default)]
    pub default_value: Option<String>,
    /// Optional column comment
    #[serde(default)]
    pub comment: Option<String>,
    /// Foreign key reference (e.g., "other_table.id")
    #[serde(default)]
    pub foreign_key: Option<String>,
}

/// Output after creating a table.
#[derive(Debug, Serialize)]
pub struct CreateTableOutput {
    /// The created table metadata
    pub table: serde_json::Value,
}

// =============================================================================
// Drop Table
// =============================================================================

/// Input for dropping (deleting) a table.
#[derive(Debug, Deserialize)]
pub struct DropTableInput {
    /// App ID identifying the target database
    pub app_id: String,
    /// Table ID (numeric) to drop
    pub table_id: i64,
    /// Whether to CASCADE (also drop dependent objects)
    #[serde(default)]
    pub cascade: bool,
}

/// Output after dropping a table.
#[derive(Debug, Serialize)]
pub struct DropTableOutput {
    /// Whether the drop succeeded
    pub success: bool,
}

// =============================================================================
// Execute SQL
// =============================================================================

/// Input for executing raw SQL against the database.
#[derive(Debug, Deserialize)]
pub struct ExecuteSqlInput {
    /// App ID identifying the target database
    pub app_id: String,
    /// SQL statement to execute
    pub sql: String,
}

/// Output from SQL execution.
#[derive(Debug, Serialize)]
pub struct ExecuteSqlOutput {
    /// Result rows (for SELECT queries)
    pub rows: Vec<serde_json::Value>,
    /// Number of rows affected or returned
    pub row_count: i64,
    /// Execution duration in milliseconds
    pub duration_ms: f64,
    /// Error message if the query failed (partial success scenario)
    pub error: Option<String>,
}

// =============================================================================
// Explain SQL
// =============================================================================

/// Input for explaining a SQL query's execution plan.
#[derive(Debug, Deserialize)]
pub struct ExplainSqlInput {
    /// App ID identifying the target database
    pub app_id: String,
    /// SQL query to explain
    pub sql: String,
    /// Whether to run EXPLAIN ANALYZE (actually executes the query; default: true)
    #[serde(default = "default_true")]
    pub analyze: bool,
}

/// Output from EXPLAIN/EXPLAIN ANALYZE.
#[derive(Debug, Serialize)]
pub struct ExplainSqlOutput {
    /// Structured query plan (JSON format from PostgreSQL)
    pub plan: serde_json::Value,
    /// Human-readable plan text
    pub raw_text: String,
    /// Duration of the EXPLAIN operation in milliseconds
    pub duration_ms: f64,
    /// Error message if explain failed
    pub error: Option<String>,
}

// =============================================================================
// List Schemas
// =============================================================================

/// Input for listing database schemas.
#[derive(Debug, Deserialize)]
pub struct ListSchemasInput {
    /// App ID identifying the target database
    pub app_id: String,
}

/// Output containing the list of schemas.
#[derive(Debug, Serialize)]
pub struct ListSchemasOutput {
    /// Array of schema metadata objects
    pub schemas: Vec<serde_json::Value>,
}

// =============================================================================
// List Views
// =============================================================================

/// Input for listing database views.
#[derive(Debug, Deserialize)]
pub struct ListViewsInput {
    /// App ID identifying the target database
    pub app_id: String,
}

/// Output containing the list of views.
#[derive(Debug, Serialize)]
pub struct ListViewsOutput {
    /// Array of view metadata objects
    pub views: Vec<serde_json::Value>,
}

// =============================================================================
// List Functions
// =============================================================================

/// Input for listing database functions.
#[derive(Debug, Deserialize)]
pub struct ListFunctionsInput {
    /// App ID identifying the target database
    pub app_id: String,
}

/// Output containing the list of functions.
#[derive(Debug, Serialize)]
pub struct ListFunctionsOutput {
    /// Array of function metadata objects
    pub functions: Vec<serde_json::Value>,
}

// =============================================================================
// Schema Context (AI)
// =============================================================================

/// Input for getting AI-friendly schema context.
#[derive(Debug, Deserialize)]
pub struct SchemaContextInput {
    /// App ID identifying the target database
    pub app_id: String,
}

/// Output containing DDL text and summary statistics for AI consumption.
#[derive(Debug, Serialize)]
pub struct SchemaContextOutput {
    /// Full DDL text of the database schema
    pub ddl_text: String,
    /// Number of tables in the schema
    pub table_count: i64,
    /// Total number of columns across all tables
    pub column_count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_tables_input_defaults() {
        let input: ListTablesInput =
            serde_json::from_str(r#"{"app_id": "app-123"}"#).expect("deserialize");
        assert_eq!(input.app_id, "app-123");
        assert_eq!(input.schema, "public");
    }

    #[test]
    fn test_list_tables_input_custom_schema() {
        let input: ListTablesInput =
            serde_json::from_str(r#"{"app_id": "app-123", "schema": "private"}"#)
                .expect("deserialize");
        assert_eq!(input.schema, "private");
    }

    #[test]
    fn test_query_rows_input_minimal() {
        let input: QueryRowsInput =
            serde_json::from_str(r#"{"app_id": "app-1", "table": "users"}"#).expect("deserialize");
        assert_eq!(input.table, "users");
        assert!(input.filters.is_none());
        assert!(input.page.is_none());
        assert!(input.per_page.is_none());
        assert!(input.order.is_none());
        assert!(input.sort.is_none());
    }

    #[test]
    fn test_query_rows_input_full() {
        let input: QueryRowsInput = serde_json::from_str(
            r#"{"app_id": "a", "table": "t", "filters": ["age=gt.18"], "page": 2, "per_page": 25, "order": "name", "sort": "asc"}"#,
        )
        .expect("deserialize");
        assert_eq!(input.filters.as_ref().unwrap().len(), 1);
        assert_eq!(input.page, Some(2));
        assert_eq!(input.per_page, Some(25));
        assert_eq!(input.order.as_deref(), Some("name"));
        assert_eq!(input.sort.as_deref(), Some("asc"));
    }

    #[test]
    fn test_create_column_def_defaults() {
        let col: CreateColumnDef =
            serde_json::from_str(r#"{"name": "id", "data_type": "uuid"}"#).expect("deserialize");
        assert_eq!(col.name, "id");
        assert!(!col.is_primary_key);
        assert!(col.is_nullable); // default true
        assert!(!col.is_unique);
        assert!(!col.is_identity);
        assert!(col.default_value.is_none());
        assert!(col.comment.is_none());
        assert!(col.foreign_key.is_none());
    }

    #[test]
    fn test_create_table_input() {
        let input: CreateTableInput = serde_json::from_str(
            r#"{
                "app_id": "app-1",
                "name": "users",
                "columns": [
                    {"name": "id", "data_type": "uuid", "is_primary_key": true, "is_nullable": false, "default_value": "gen_random_uuid()"},
                    {"name": "email", "data_type": "text", "is_unique": true}
                ]
            }"#,
        )
        .expect("deserialize");
        assert_eq!(input.name, "users");
        assert_eq!(input.schema, "public");
        assert_eq!(input.columns.len(), 2);
        assert!(input.columns[0].is_primary_key);
        assert!(!input.columns[0].is_nullable);
        assert!(input.columns[1].is_unique);
    }

    #[test]
    fn test_drop_table_input() {
        let input: DropTableInput =
            serde_json::from_str(r#"{"app_id": "app-1", "table_id": 42}"#).expect("deserialize");
        assert_eq!(input.table_id, 42);
        assert!(!input.cascade);
    }

    #[test]
    fn test_execute_sql_input() {
        let input: ExecuteSqlInput =
            serde_json::from_str(r#"{"app_id": "app-1", "sql": "SELECT 1"}"#)
                .expect("deserialize");
        assert_eq!(input.sql, "SELECT 1");
    }

    #[test]
    fn test_explain_sql_input_defaults() {
        let input: ExplainSqlInput =
            serde_json::from_str(r#"{"app_id": "app-1", "sql": "SELECT * FROM users"}"#)
                .expect("deserialize");
        assert!(input.analyze); // default true
    }

    #[test]
    fn test_insert_rows_input() {
        let input: InsertRowsInput = serde_json::from_str(
            r#"{"app_id": "app-1", "table": "users", "rows": [{"name": "Alice"}, {"name": "Bob"}]}"#,
        )
        .expect("deserialize");
        assert_eq!(input.rows.len(), 2);
    }

    #[test]
    fn test_update_rows_input() {
        let input: UpdateRowsInput = serde_json::from_str(
            r#"{"app_id": "app-1", "table": "users", "values": {"status": "active"}, "filters": ["id=eq.42"]}"#,
        )
        .expect("deserialize");
        assert_eq!(input.filters.len(), 1);
    }

    #[test]
    fn test_delete_rows_input() {
        let input: DeleteRowsInput = serde_json::from_str(
            r#"{"app_id": "app-1", "table": "users", "filters": ["id=eq.42"]}"#,
        )
        .expect("deserialize");
        assert_eq!(input.filters.len(), 1);
    }

    #[test]
    fn test_schema_context_input() {
        let input: SchemaContextInput =
            serde_json::from_str(r#"{"app_id": "app-1"}"#).expect("deserialize");
        assert_eq!(input.app_id, "app-1");
    }

    #[test]
    fn test_output_serialization() {
        let output = ListTablesOutput {
            tables: vec![serde_json::json!({"name": "users", "schema": "public"})],
        };
        let json = serde_json::to_value(&output).expect("serialize");
        assert_eq!(json["tables"][0]["name"], "users");
    }

    #[test]
    fn test_execute_sql_output_serialization() {
        let output = ExecuteSqlOutput {
            rows: vec![serde_json::json!({"count": 5})],
            row_count: 1,
            duration_ms: 12.5,
            error: None,
        };
        let json = serde_json::to_value(&output).expect("serialize");
        assert_eq!(json["row_count"], 1);
        assert!(json["error"].is_null());
    }

    #[test]
    fn test_schema_context_output_serialization() {
        let output = SchemaContextOutput {
            ddl_text: "CREATE TABLE users (id uuid PRIMARY KEY);".to_string(),
            table_count: 1,
            column_count: 1,
        };
        let json = serde_json::to_value(&output).expect("serialize");
        assert_eq!(json["table_count"], 1);
    }
}
