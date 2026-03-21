//! Tool Traits - Standard interface for agent tools
//!
//! Re-exported from `gateway-tool-types` to maintain backward compatibility.

pub use gateway_tool_types::{
    AgentTool, DynamicTool, ToolError, ToolMetadata, ToolResult, ToolWrapper,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_error_display() {
        let err = ToolError::InvalidInput("missing field".to_string());
        assert_eq!(err.to_string(), "Invalid input: missing field");

        let err = ToolError::PermissionDenied("write not allowed".to_string());
        assert_eq!(err.to_string(), "Permission denied: write not allowed");
    }

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
