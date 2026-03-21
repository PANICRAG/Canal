//! LLM adapter — bridges gateway-core's LlmProvider to canal-cv's CvLlmClient.

use std::sync::Arc;

use async_trait::async_trait;
use canal_cv::llm_client::{CvChatRequest, CvChatResponse, CvContent, CvLlmClient, CvLlmError};

use crate::llm::router::{ChatRequest, ContentBlock, LlmProvider, Message};

/// Bridges gateway-core's `LlmProvider` to canal-cv's `CvLlmClient`.
///
/// Translates between the two type systems so the CV engine can use
/// gateway-core's LLM routing infrastructure without depending on it directly.
pub struct GatewayCoreLlmClient {
    provider: Arc<dyn LlmProvider>,
}

impl GatewayCoreLlmClient {
    /// Create from a gateway-core LlmProvider.
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl CvLlmClient for GatewayCoreLlmClient {
    async fn chat(&self, request: CvChatRequest) -> Result<CvChatResponse, CvLlmError> {
        let messages: Vec<Message> = request
            .messages
            .into_iter()
            .map(|m| {
                let blocks: Vec<ContentBlock> = m
                    .content
                    .into_iter()
                    .map(|c| match c {
                        CvContent::Text { text } => ContentBlock::Text { text },
                        CvContent::Image {
                            media_type,
                            base64_data,
                        } => ContentBlock::Image {
                            source_type: "base64".to_string(),
                            media_type,
                            data: base64_data,
                        },
                    })
                    .collect();
                Message::with_blocks(m.role, blocks)
            })
            .collect();

        let chat_request = ChatRequest {
            messages,
            model: request.model,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            tools: vec![],
            ..Default::default()
        };

        let response = self
            .provider
            .chat(chat_request)
            .await
            .map_err(|e| CvLlmError::RequestFailed(e.to_string()))?;

        let text = response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        Ok(CvChatResponse { text })
    }

    async fn is_available(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gateway_llm_adapter_creation() {
        // Just verify the type compiles — actual LLM calls need mocking
        let _: fn(Arc<dyn LlmProvider>) -> GatewayCoreLlmClient = GatewayCoreLlmClient::new;
    }
}
