//! Tool execution service trait.
//!
//! Defines the boundary for tool discovery and execution.
//! - Local impl wraps `ToolSystem` directly
//! - Remote impl sends requests via gRPC to tool-service

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::ServiceResult;

/// Result of executing a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallOutput {
    /// Content blocks (text, images, etc.)
    pub content: Vec<ToolContentBlock>,
    /// Whether the tool call resulted in an error
    pub is_error: bool,
}

/// A content block in a tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolContentBlock {
    /// Text content
    #[serde(rename = "text")]
    Text { text: String },
    /// Image content (base64 encoded)
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
}

/// Metadata about an available tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    /// Namespace (e.g., "filesystem", "agent", "mcp_server_name")
    pub namespace: String,
    /// Tool name within the namespace
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for the tool's input parameters
    pub input_schema: serde_json::Value,
}

/// Service boundary for tool execution.
///
/// # Example
///
/// ```rust,ignore
/// let tools: Arc<dyn ToolService> = Arc::new(LocalToolService::new(tool_system));
/// let result = tools.execute("filesystem", "read_file", json!({"path": "/tmp/foo"})).await?;
/// ```
#[async_trait]
pub trait ToolService: Send + Sync {
    /// Execute a tool by namespace and name.
    async fn execute(
        &self,
        namespace: &str,
        name: &str,
        input: serde_json::Value,
    ) -> ServiceResult<ToolCallOutput>;

    /// Execute a tool call using the LLM tool name format (namespace_name).
    /// Includes permission checking.
    async fn execute_llm_tool_call(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> ServiceResult<ToolCallOutput>;

    /// Execute a tool call with pre-approved permission (bypasses permission checks).
    async fn execute_llm_tool_call_approved(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> ServiceResult<ToolCallOutput>;

    /// List all available tools.
    async fn list_tools(&self) -> ServiceResult<Vec<ToolInfo>>;

    /// List tools filtered by enabled namespaces.
    async fn list_tools_filtered(
        &self,
        enabled_namespaces: &[String],
    ) -> ServiceResult<Vec<ToolInfo>>;

    /// Get tool JSON schemas suitable for sending to an LLM.
    async fn schemas_for_llm(&self) -> ServiceResult<Vec<serde_json::Value>>;

    /// Health check for this service.
    async fn health(&self) -> ServiceResult<bool>;
}
