//! Tool traits — standard interface for agent tools.

use crate::context::ToolContext;
use crate::error::{ToolError, ToolResult};
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};

/// Agent tool trait — standard interface for all tools.
///
/// Tools implement this with concrete `Input` and `Output` types.
/// Use [`ToolWrapper`] to bridge to [`DynamicTool`] for type-erased storage.
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// Input type for the tool
    type Input: DeserializeOwned + Send;
    /// Output type for the tool
    type Output: Serialize + Send;

    /// Get the tool name
    fn name(&self) -> &str;

    /// Get the tool description
    fn description(&self) -> &str;

    /// Get the JSON schema for the input
    fn input_schema(&self) -> serde_json::Value;

    /// Check if this tool requires permission
    fn requires_permission(&self) -> bool {
        true
    }

    /// Check if this tool modifies state
    fn is_mutating(&self) -> bool {
        false
    }

    /// Get the tool namespace
    fn namespace(&self) -> &str {
        "builtin"
    }

    /// Execute the tool
    async fn execute(&self, input: Self::Input, context: &ToolContext) -> ToolResult<Self::Output>;
}

/// Tool metadata for registration
#[derive(Debug, Clone)]
pub struct ToolMetadata {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// Tool namespace
    pub namespace: String,
    /// Input JSON schema
    pub input_schema: serde_json::Value,
    /// Whether the tool requires permission
    pub requires_permission: bool,
    /// Whether the tool mutates state
    pub is_mutating: bool,
}

impl ToolMetadata {
    /// Create metadata from an AgentTool
    pub fn from_tool<T: AgentTool>(tool: &T) -> Self {
        Self {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            namespace: tool.namespace().to_string(),
            input_schema: tool.input_schema(),
            requires_permission: tool.requires_permission(),
            is_mutating: tool.is_mutating(),
        }
    }
}

/// Dynamic tool wrapper for type-erased execution
#[async_trait]
pub trait DynamicTool: Send + Sync {
    /// Get tool metadata
    fn metadata(&self) -> &ToolMetadata;

    /// Execute with JSON input and output
    async fn execute_json(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> ToolResult<serde_json::Value>;
}

/// Wrapper to make any AgentTool into a DynamicTool
pub struct ToolWrapper<T: AgentTool> {
    tool: T,
    metadata: ToolMetadata,
}

impl<T: AgentTool> ToolWrapper<T> {
    /// Create a new wrapper around an AgentTool
    pub fn new(tool: T) -> Self {
        let metadata = ToolMetadata::from_tool(&tool);
        Self { tool, metadata }
    }
}

#[async_trait]
impl<T: AgentTool + 'static> DynamicTool for ToolWrapper<T>
where
    T::Output: 'static,
{
    fn metadata(&self) -> &ToolMetadata {
        &self.metadata
    }

    async fn execute_json(
        &self,
        input: serde_json::Value,
        context: &ToolContext,
    ) -> ToolResult<serde_json::Value> {
        let typed_input: T::Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;

        let output = self.tool.execute(typed_input, context).await?;

        serde_json::to_value(output).map_err(|e| ToolError::ExecutionError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_metadata() {
        let metadata = ToolMetadata {
            name: "Read".to_string(),
            description: "Read a file".to_string(),
            namespace: "filesystem".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"}
                },
                "required": ["file_path"]
            }),
            requires_permission: false,
            is_mutating: false,
        };

        assert_eq!(metadata.name, "Read");
        assert!(!metadata.is_mutating);
    }
}
