//! Request dispatcher for the MCP server
//!
//! Routes tool calls to the appropriate namespace handler after auth checks.

use crate::catalog::{CatalogTool, ToolCatalog};
use crate::handlers::{self, HandlerContext};
use canal_identity::service::IdentityService;
use canal_identity::types::{AgentIdentity, AgentScope};
use std::sync::Arc;
use tracing::{info, warn};

/// Dispatcher routes tool calls to the appropriate namespace handler
pub struct Dispatcher {
    catalog: Arc<ToolCatalog>,
    identity_service: Arc<IdentityService>,
    handler_ctx: Arc<HandlerContext>,
}

impl Dispatcher {
    pub fn new(
        catalog: Arc<ToolCatalog>,
        identity_service: Arc<IdentityService>,
        handler_ctx: Arc<HandlerContext>,
    ) -> Self {
        Self {
            catalog,
            identity_service,
            handler_ctx,
        }
    }

    /// Resolve an API key to an agent identity
    pub async fn resolve_identity(&self, api_key: &str) -> Result<AgentIdentity, DispatchError> {
        self.identity_service
            .resolve(api_key)
            .await
            .map_err(|e| DispatchError::AuthFailed(e.to_string()))
    }

    /// List tools visible to the given identity
    pub fn list_tools(&self, identity: &AgentIdentity) -> Vec<CatalogTool> {
        self.catalog.list_for_scopes(&identity.scopes)
    }

    /// Dispatch a tool call after auth + scope check
    pub async fn dispatch(
        &self,
        identity: &AgentIdentity,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, DispatchError> {
        // 1. Check tool exists
        let tool = self
            .catalog
            .get(tool_name)
            .ok_or_else(|| DispatchError::ToolNotFound(tool_name.to_string()))?;

        // 2. Check scope
        if !identity.scopes.contains(&tool.required_scope) {
            warn!(
                agent_id = %identity.id,
                tool = tool_name,
                required_scope = ?tool.required_scope,
                "Scope check failed"
            );
            return Err(DispatchError::InsufficientScope {
                tool: tool_name.to_string(),
                required: tool.required_scope.clone(),
            });
        }

        info!(
            agent_id = %identity.id,
            tool = tool_name,
            namespace = tool.namespace,
            "Dispatching tool call"
        );

        // 3. Budget check skipped for MCP server (billing-core BudgetGuard requires
        //    model/token info which isn't available at tool-call dispatch level).
        //    Usage is recorded after execution via record_tool_usage().

        // 4. Route to namespace handler
        let start = std::time::Instant::now();
        let result = match tool.namespace.as_str() {
            "engine" => {
                handlers::engine::handle(&self.handler_ctx, identity, tool_name, arguments).await
            }
            "platform" => {
                handlers::platform::handle(&self.handler_ctx, identity, tool_name, arguments).await
            }
            "tools" => {
                handlers::tools::handle(&self.handler_ctx, identity, tool_name, arguments).await
            }
            "billing" => {
                handlers::billing::handle(&self.handler_ctx, identity, tool_name, arguments).await
            }
            "mcp" => {
                handlers::mcp_proxy::handle(&self.handler_ctx, identity, tool_name, arguments).await
            }
            "runtime" => {
                handlers::runtime::handle(&self.handler_ctx, identity, tool_name, arguments).await
            }
            "workflow" => {
                handlers::workflow::handle(&self.handler_ctx, identity, tool_name, arguments).await
            }
            "control" => {
                handlers::control::handle(&self.handler_ctx, identity, tool_name, arguments).await
            }
            ns => Err(DispatchError::UnknownNamespace(ns.to_string())),
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        // 5. Record usage via billing-core (async, best-effort)
        let success = result.is_ok();
        let metadata = serde_json::json!({
            "tool_name": tool_name,
            "duration_ms": duration_ms,
            "success": success,
        });
        if let Err(e) = self
            .handler_ctx
            .metering
            .record_tool_usage(identity.id, tool_name, Some(metadata))
            .await
        {
            warn!(error = %e, tool = tool_name, "Failed to record tool usage (non-fatal)");
        }

        result
    }
}

/// Errors from the dispatcher
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Insufficient scope for tool '{tool}': requires {required:?}")]
    InsufficientScope { tool: String, required: AgentScope },

    #[error("Unknown namespace: {0}")]
    UnknownNamespace(String),

    #[error("Budget exceeded: {0}")]
    BudgetExceeded(String),

    #[error("Handler error: {0}")]
    HandlerError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::build_default_catalog;
    use canal_identity::store::DashMapKeyStore;
    use canal_identity::types::AgentTier;

    fn make_test_dispatcher() -> Dispatcher {
        use billing_core::store::memory::{InMemoryBalanceStore, InMemoryEventStore};
        use billing_core::{BillingService, MeteringService, PlanRegistry, PricingEngine};

        let catalog = build_default_catalog();
        let store = DashMapKeyStore::new();
        let identity_service = Arc::new(IdentityService::new(Arc::new(store)));

        let pricing = Arc::new(PricingEngine::with_defaults());
        let plans = Arc::new(PlanRegistry::with_defaults());
        let balance = Arc::new(InMemoryBalanceStore::new());
        let events = Arc::new(InMemoryEventStore::new());
        let billing = Arc::new(BillingService::new(balance, events, pricing.clone(), plans));
        let metering = Arc::new(MeteringService::new(pricing, billing.clone()));

        let handler_ctx = Arc::new(HandlerContext::new(
            None,
            None,
            serde_json::json!({}),
            metering,
            billing,
        ));

        Dispatcher::new(catalog, identity_service, handler_ctx)
    }

    #[test]
    fn test_list_tools_filters_by_scope() {
        let dispatcher = make_test_dispatcher();
        let identity = AgentIdentity {
            id: uuid::Uuid::new_v4(),
            name: "test-agent".to_string(),
            tier: AgentTier::Standard,
            scopes: vec![AgentScope::EngineChat, AgentScope::ToolsRead],
            created_at: chrono::Utc::now(),
        };

        let tools = dispatcher.list_tools(&identity);
        // Should see engine.* and tools.list (ToolsRead) + platform.* (ToolsRead)
        // But NOT tools.call (ToolsWrite) or admin.* or mcp.*
        for tool in &tools {
            assert!(
                tool.required_scope == AgentScope::EngineChat
                    || tool.required_scope == AgentScope::ToolsRead,
                "Unexpected scope: {:?} for tool {}",
                tool.required_scope,
                tool.name
            );
        }
    }

    #[tokio::test]
    async fn test_dispatch_tool_not_found() {
        let dispatcher = make_test_dispatcher();
        let identity = AgentIdentity {
            id: uuid::Uuid::new_v4(),
            name: "test-agent".to_string(),
            tier: AgentTier::System,
            scopes: AgentTier::System.default_scopes(),
            created_at: chrono::Utc::now(),
        };

        let result = dispatcher
            .dispatch(&identity, "nonexistent.tool", serde_json::json!({}))
            .await;
        assert!(matches!(result, Err(DispatchError::ToolNotFound(_))));
    }

    #[tokio::test]
    async fn test_dispatch_insufficient_scope() {
        let dispatcher = make_test_dispatcher();
        let identity = AgentIdentity {
            id: uuid::Uuid::new_v4(),
            name: "limited-agent".to_string(),
            tier: AgentTier::Trial,
            scopes: vec![AgentScope::ToolsRead], // No EngineChat scope
            created_at: chrono::Utc::now(),
        };

        let result = dispatcher
            .dispatch(&identity, "engine.chat", serde_json::json!({}))
            .await;
        assert!(matches!(
            result,
            Err(DispatchError::InsufficientScope { .. })
        ));
    }

    #[tokio::test]
    async fn test_dispatch_platform_health() {
        let dispatcher = make_test_dispatcher();
        let identity = AgentIdentity {
            id: uuid::Uuid::new_v4(),
            name: "test-agent".to_string(),
            tier: AgentTier::System,
            scopes: AgentTier::System.default_scopes(),
            created_at: chrono::Utc::now(),
        };

        let result = dispatcher
            .dispatch(&identity, "platform.health", serde_json::json!({}))
            .await;
        assert!(result.is_ok());
    }
}
