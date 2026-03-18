//! Local (in-process) implementation of LlmService.

use std::sync::Arc;
use tokio::sync::RwLock;

use async_trait::async_trait;
use gateway_llm::{ChatRequest, ChatResponse, LlmRouter, StreamResponse};
use gateway_service_traits::error::{ServiceError, ServiceResult};
use gateway_service_traits::LlmService;

/// In-process LLM service wrapping a concrete `LlmRouter`.
///
/// Zero overhead compared to calling `LlmRouter` directly.
/// All initialization (register_provider, set_routing_engine, etc.)
/// must happen BEFORE wrapping in this struct.
pub struct LocalLlmService {
    router: Arc<RwLock<LlmRouter>>,
}

impl LocalLlmService {
    /// Create a new local LLM service wrapping an existing router.
    pub fn new(router: Arc<RwLock<LlmRouter>>) -> Self {
        Self { router }
    }

    /// Get the underlying router (for initialization or internal use).
    pub fn router(&self) -> &Arc<RwLock<LlmRouter>> {
        &self.router
    }
}

#[async_trait]
impl LlmService for LocalLlmService {
    async fn chat(&self, request: ChatRequest) -> ServiceResult<ChatResponse> {
        let router = self.router.read().await;
        router.route(request).await.map_err(ServiceError::from)
    }

    async fn chat_stream(&self, request: ChatRequest) -> ServiceResult<StreamResponse> {
        let router = self.router.read().await;
        router
            .route_stream(request)
            .await
            .map_err(ServiceError::from)
    }

    async fn chat_with_profile(
        &self,
        profile_id: &str,
        request: ChatRequest,
    ) -> ServiceResult<ChatResponse> {
        let router = self.router.read().await;
        router
            .route_with_profile(profile_id, request)
            .await
            .map_err(ServiceError::from)
    }

    async fn chat_stream_with_profile(
        &self,
        profile_id: &str,
        request: ChatRequest,
    ) -> ServiceResult<StreamResponse> {
        let router = self.router.read().await;
        router
            .route_stream_with_profile(profile_id, request)
            .await
            .map_err(ServiceError::from)
    }

    async fn list_providers(&self) -> ServiceResult<Vec<String>> {
        let router = self.router.read().await;
        Ok(router.list_providers())
    }

    async fn health(&self) -> ServiceResult<bool> {
        let router = self.router.read().await;
        Ok(!router.list_providers().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_llm::LlmConfig;

    #[tokio::test]
    async fn test_local_llm_service_health() {
        let config = LlmConfig {
            default_provider: "test".to_string(),
            ..Default::default()
        };
        let router = Arc::new(RwLock::new(LlmRouter::new(config)));
        let service = LocalLlmService::new(router);
        // No providers registered → still healthy (returns true if not panicking)
        let result = service.health().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_local_llm_service_list_providers_empty() {
        let config = LlmConfig {
            default_provider: "test".to_string(),
            ..Default::default()
        };
        let router = Arc::new(RwLock::new(LlmRouter::new(config)));
        let service = LocalLlmService::new(router);
        let providers = service.list_providers().await.unwrap();
        assert!(providers.is_empty());
    }
}
