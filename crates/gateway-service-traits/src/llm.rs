//! LLM routing service trait.
//!
//! Defines the boundary for LLM chat completion routing.
//! - Local impl wraps `LlmRouter` directly (zero overhead)
//! - Remote impl sends requests via gRPC to llm-service

use async_trait::async_trait;
use gateway_llm::{ChatRequest, ChatResponse, StreamResponse};

use crate::error::ServiceResult;

/// Service boundary for LLM routing.
///
/// # Example
///
/// ```rust,ignore
/// // Monolith mode: in-process
/// let llm: Arc<dyn LlmService> = Arc::new(LocalLlmService::new(router));
///
/// // Distributed mode: gRPC
/// let llm: Arc<dyn LlmService> = Arc::new(RemoteLlmService::connect(url).await?);
/// ```
#[async_trait]
pub trait LlmService: Send + Sync {
    /// Send a chat request and get a complete response.
    async fn chat(&self, request: ChatRequest) -> ServiceResult<ChatResponse>;

    /// Send a chat request and get a streaming response.
    async fn chat_stream(&self, request: ChatRequest) -> ServiceResult<StreamResponse>;

    /// Chat with an explicit profile ID for routing.
    async fn chat_with_profile(
        &self,
        profile_id: &str,
        request: ChatRequest,
    ) -> ServiceResult<ChatResponse>;

    /// Streaming chat with an explicit profile ID for routing.
    async fn chat_stream_with_profile(
        &self,
        profile_id: &str,
        request: ChatRequest,
    ) -> ServiceResult<StreamResponse>;

    /// List registered provider names.
    async fn list_providers(&self) -> ServiceResult<Vec<String>>;

    /// Health check for this service.
    async fn health(&self) -> ServiceResult<bool>;
}
