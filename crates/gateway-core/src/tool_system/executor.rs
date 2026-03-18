//! Unified Tool Executor
//!
//! Single dispatch point for all tool executions. Routes by ToolSource:
//! - Agent -> DynamicTool.execute_json()
//! - McpBuiltin -> BuiltinToolExecutor.execute()
//! - McpExternal -> McpConnection.call_tool()

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::agent::tools::{DynamicTool, ToolContext};
use crate::error::{Error, Result};
use crate::mcp::builtin::BuiltinToolExecutor;
use crate::mcp::connection::McpConnection;
use crate::mcp::gateway::McpServerConfig;
use crate::mcp::protocol::ToolCallResult;

use super::registry::UnifiedToolRegistry;
use super::types::{ToolEntry, ToolSource};

/// Unified executor that dispatches tool calls to the appropriate backend
pub struct UnifiedToolExecutor {
    /// Agent built-in tools keyed by name
    agent_tools: RwLock<HashMap<String, Arc<dyn DynamicTool>>>,
    /// Builtin executor for MCP builtin tools (filesystem, executor, browser, mac, automation)
    builtin_executor: Arc<RwLock<Option<BuiltinToolExecutor>>>,
    /// MCP server connections (shared with McpGateway)
    connections: Arc<RwLock<HashMap<String, McpConnection>>>,
    /// MCP server configs (shared with McpGateway, reserved for future use)
    #[allow(dead_code)]
    configs: Arc<RwLock<HashMap<String, McpServerConfig>>>,
}

impl UnifiedToolExecutor {
    /// Create a new executor
    pub fn new(
        connections: Arc<RwLock<HashMap<String, McpConnection>>>,
        configs: Arc<RwLock<HashMap<String, McpServerConfig>>>,
    ) -> Self {
        Self {
            agent_tools: RwLock::new(HashMap::new()),
            builtin_executor: Arc::new(RwLock::new(None)),
            connections,
            configs,
        }
    }

    /// Register an agent tool
    pub async fn register_agent_tool(&self, name: String, tool: Arc<dyn DynamicTool>) {
        let mut tools = self.agent_tools.write().await;
        tools.insert(name, tool);
    }

    /// Set the builtin executor
    pub async fn set_builtin_executor(&self, executor: BuiltinToolExecutor) {
        let mut builtin = self.builtin_executor.write().await;
        *builtin = Some(executor);
    }

    /// Get a reference to the builtin executor Arc for external mutation
    pub fn builtin_executor_ref(&self) -> &Arc<RwLock<Option<BuiltinToolExecutor>>> {
        &self.builtin_executor
    }

    /// Execute a tool given its entry and input
    pub async fn execute(
        &self,
        entry: &ToolEntry,
        input: serde_json::Value,
    ) -> Result<ToolCallResult> {
        match &entry.source {
            ToolSource::Agent => self.execute_agent(&entry.id.name, input).await,
            ToolSource::McpBuiltin => {
                self.execute_builtin(&entry.id.namespace, &entry.id.name, input)
                    .await
            }
            ToolSource::McpExternal { server_name } => {
                self.execute_external(server_name, &entry.id.namespace, &entry.id.name, input)
                    .await
            }
        }
    }

    /// Execute by namespace and name (resolves through registry)
    pub async fn execute_by_namespace(
        &self,
        registry: &UnifiedToolRegistry,
        namespace: &str,
        name: &str,
        input: serde_json::Value,
    ) -> Result<ToolCallResult> {
        let id = super::types::ToolId::new(namespace, name);
        let entry = registry
            .get(&id)
            .ok_or_else(|| Error::NotFound(format!("Tool not found: {}.{}", namespace, name)))?;
        self.execute(entry, input).await
    }

    /// Execute by LLM name (resolves through registry)
    pub async fn execute_by_llm_name(
        &self,
        registry: &UnifiedToolRegistry,
        llm_name: &str,
        input: serde_json::Value,
    ) -> Result<ToolCallResult> {
        let entry = registry
            .get_by_llm_name(llm_name)
            .ok_or_else(|| Error::NotFound(format!("Tool not found: {}", llm_name)))?;
        self.execute(entry, input).await
    }

    /// Execute an agent tool
    async fn execute_agent(&self, name: &str, input: serde_json::Value) -> Result<ToolCallResult> {
        let tools = self.agent_tools.read().await;
        let tool = tools
            .get(name)
            .ok_or_else(|| Error::NotFound(format!("Agent tool not found: {}", name)))?;

        let context = ToolContext::default();
        match tool.execute_json(input, &context).await {
            Ok(value) => Ok(ToolCallResult::text(
                serde_json::to_string_pretty(&value).unwrap_or_default(),
            )),
            Err(e) => Ok(ToolCallResult::error(e.to_string())),
        }
    }

    /// Execute a builtin MCP tool
    async fn execute_builtin(
        &self,
        namespace: &str,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<ToolCallResult> {
        let builtin = self.builtin_executor.read().await;
        let executor = builtin
            .as_ref()
            .ok_or_else(|| Error::Internal("Builtin executor not initialized".to_string()))?;

        if executor.handles_namespace(namespace) {
            executor.execute(namespace, tool_name, input).await
        } else {
            Err(Error::NotFound(format!(
                "Builtin executor does not handle namespace: {}",
                namespace
            )))
        }
    }

    /// Execute an external MCP server tool
    async fn execute_external(
        &self,
        server_name: &str,
        _namespace: &str,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<ToolCallResult> {
        let connections = self.connections.read().await;
        let connection = connections.get(server_name).ok_or_else(|| {
            Error::NotFound(format!(
                "Server '{}' is not connected. Call connect_server first.",
                server_name
            ))
        })?;

        let result = connection.call_tool(tool_name, input).await?;
        Ok(result)
    }
}
