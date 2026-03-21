//! Local (in-process) implementation of ToolService.

use std::sync::Arc;

use async_trait::async_trait;
use gateway_service_traits::error::{ServiceError, ServiceResult};
use gateway_service_traits::tools::{ToolCallOutput, ToolContentBlock, ToolInfo, ToolService};

use crate::tool_system::ToolSystem;

/// In-process tool service wrapping a concrete `ToolSystem`.
pub struct LocalToolService {
    tool_system: Arc<ToolSystem>,
}

impl LocalToolService {
    /// Create a new local tool service.
    pub fn new(tool_system: Arc<ToolSystem>) -> Self {
        Self { tool_system }
    }

    /// Get the underlying tool system (for internal use).
    pub fn tool_system(&self) -> &Arc<ToolSystem> {
        &self.tool_system
    }
}

/// Convert internal `ToolCallResult` to the service boundary `ToolCallOutput`.
fn to_output(result: crate::mcp::protocol::ToolCallResult) -> ToolCallOutput {
    let content = result
        .content
        .into_iter()
        .map(|c| match c {
            crate::mcp::protocol::ToolContent::Text { text } => ToolContentBlock::Text { text },
            crate::mcp::protocol::ToolContent::Image { data, mime_type } => {
                ToolContentBlock::Image { data, mime_type }
            }
            crate::mcp::protocol::ToolContent::Resource { uri, mime_type } => {
                // Map resource to text representation at boundary
                ToolContentBlock::Text {
                    text: format!("[resource: {} ({})]", uri, mime_type),
                }
            }
        })
        .collect();
    ToolCallOutput {
        content,
        is_error: result.is_error,
    }
}

/// Convert internal errors to service errors.
fn map_err(e: crate::error::Error) -> ServiceError {
    match e {
        crate::error::Error::NotFound(msg) => ServiceError::NotFound(msg),
        crate::error::Error::PermissionDenied(msg) => ServiceError::PermissionDenied(msg),
        crate::error::Error::RateLimited => ServiceError::RateLimited,
        crate::error::Error::Timeout(msg) => ServiceError::Timeout(msg),
        crate::error::Error::InvalidInput(msg) => ServiceError::InvalidInput(msg),
        other => ServiceError::Internal(other.to_string()),
    }
}

#[async_trait]
impl ToolService for LocalToolService {
    async fn execute(
        &self,
        namespace: &str,
        name: &str,
        input: serde_json::Value,
    ) -> ServiceResult<ToolCallOutput> {
        let result = self
            .tool_system
            .execute(namespace, name, input)
            .await
            .map_err(map_err)?;
        // execute() returns serde_json::Value, wrap it
        Ok(ToolCallOutput {
            content: vec![ToolContentBlock::Text {
                text: serde_json::to_string(&result).unwrap_or_default(),
            }],
            is_error: false,
        })
    }

    async fn execute_llm_tool_call(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> ServiceResult<ToolCallOutput> {
        let result = self
            .tool_system
            .execute_llm_tool_call(tool_name, arguments)
            .await
            .map_err(map_err)?;
        Ok(to_output(result))
    }

    async fn execute_llm_tool_call_approved(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> ServiceResult<ToolCallOutput> {
        let result = self
            .tool_system
            .execute_llm_tool_call_approved(tool_name, arguments)
            .await
            .map_err(map_err)?;
        Ok(to_output(result))
    }

    async fn list_tools(&self) -> ServiceResult<Vec<ToolInfo>> {
        let entries = self.tool_system.list_tools().await;
        Ok(entries
            .into_iter()
            .map(|e| ToolInfo {
                namespace: e.id.namespace,
                name: e.id.name,
                description: e.description,
                input_schema: e.input_schema,
            })
            .collect())
    }

    async fn list_tools_filtered(
        &self,
        enabled_namespaces: &[String],
    ) -> ServiceResult<Vec<ToolInfo>> {
        let entries = self
            .tool_system
            .list_tools_filtered(enabled_namespaces)
            .await;
        Ok(entries
            .into_iter()
            .map(|e| ToolInfo {
                namespace: e.id.namespace,
                name: e.id.name,
                description: e.description,
                input_schema: e.input_schema,
            })
            .collect())
    }

    async fn schemas_for_llm(&self) -> ServiceResult<Vec<serde_json::Value>> {
        Ok(self.tool_system.tools_for_llm().await)
    }

    async fn health(&self) -> ServiceResult<bool> {
        Ok(self.tool_system.tool_count().await > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_tool_service_list_empty() {
        let ts = Arc::new(ToolSystem::new());
        let service = LocalToolService::new(ts);
        let tools = service.list_tools().await.unwrap();
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn test_local_tool_service_health_empty() {
        let ts = Arc::new(ToolSystem::new());
        let service = LocalToolService::new(ts);
        // No tools registered → health returns false (count == 0)
        let healthy = service.health().await.unwrap();
        assert!(!healthy);
    }
}
