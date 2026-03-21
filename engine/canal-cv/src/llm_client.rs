//! Minimal LLM client trait for computer vision operations.
//!
//! This trait decouples the CV engine from gateway-core's LLM subsystem.
//! Gateway-core provides `GatewayCoreLlmClient` that bridges to its `LlmProvider`.

use async_trait::async_trait;

/// Minimal LLM client for computer vision operations.
///
/// Only supports the subset needed by CV: vision chat with images.
/// Implementors bridge to their LLM infrastructure.
///
/// # Example
/// ```ignore
/// struct MyLlmClient { /* ... */ }
///
/// #[async_trait]
/// impl CvLlmClient for MyLlmClient {
///     async fn chat(&self, request: CvChatRequest) -> Result<CvChatResponse, CvLlmError> {
///         // Call your LLM provider
///         Ok(CvChatResponse { text: "response".into() })
///     }
///     async fn is_available(&self) -> bool { true }
/// }
/// ```
#[async_trait]
pub trait CvLlmClient: Send + Sync {
    /// Send a chat request with optional image content.
    async fn chat(&self, request: CvChatRequest) -> Result<CvChatResponse, CvLlmError>;

    /// Check if the LLM backend is available.
    async fn is_available(&self) -> bool;
}

/// Chat request for CV operations.
#[derive(Debug, Clone)]
pub struct CvChatRequest {
    /// Messages in the conversation.
    pub messages: Vec<CvMessage>,
    /// Model identifier (optional, provider may have a default).
    pub model: Option<String>,
    /// Maximum tokens in response.
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    pub temperature: Option<f32>,
}

/// A single message in a CV chat request.
#[derive(Debug, Clone)]
pub struct CvMessage {
    /// Role: "user", "assistant", "system".
    pub role: String,
    /// Content blocks (text and/or images).
    pub content: Vec<CvContent>,
}

impl CvMessage {
    /// Create a message with the given role and content blocks.
    pub fn new(role: &str, content: Vec<CvContent>) -> Self {
        Self {
            role: role.to_string(),
            content,
        }
    }
}

/// Content block in a CV message.
#[derive(Debug, Clone)]
pub enum CvContent {
    /// Text content.
    Text {
        /// The text string.
        text: String,
    },
    /// Base64-encoded image.
    Image {
        /// MIME type (e.g., "image/jpeg", "image/png").
        media_type: String,
        /// Base64-encoded image data.
        base64_data: String,
    },
}

/// Response from a CV LLM call.
#[derive(Debug, Clone)]
pub struct CvChatResponse {
    /// The text response from the LLM.
    pub text: String,
}

/// Error type for CV LLM operations.
#[derive(Debug, thiserror::Error)]
pub enum CvLlmError {
    /// The LLM request failed.
    #[error("request failed: {0}")]
    RequestFailed(String),
    /// No response was returned.
    #[error("no response from LLM")]
    NoResponse,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockLlmClient;

    #[async_trait]
    impl CvLlmClient for MockLlmClient {
        async fn chat(&self, _request: CvChatRequest) -> Result<CvChatResponse, CvLlmError> {
            Ok(CvChatResponse {
                text: "mock response".into(),
            })
        }
        async fn is_available(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn test_cv_llm_client_trait() {
        let client = MockLlmClient;
        let request = CvChatRequest {
            messages: vec![CvMessage::new(
                "user",
                vec![CvContent::Text {
                    text: "hello".into(),
                }],
            )],
            model: None,
            max_tokens: Some(100),
            temperature: Some(0.1),
        };
        let response = client.chat(request).await.unwrap();
        assert_eq!(response.text, "mock response");
    }

    #[test]
    fn test_cv_chat_request_build() {
        let msg = CvMessage::new(
            "user",
            vec![
                CvContent::Image {
                    media_type: "image/jpeg".into(),
                    base64_data: "abc123".into(),
                },
                CvContent::Text {
                    text: "describe this".into(),
                },
            ],
        );
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content.len(), 2);
    }

    #[test]
    fn test_cv_llm_error_display() {
        let err = CvLlmError::RequestFailed("timeout".into());
        assert_eq!(err.to_string(), "request failed: timeout");

        let err2 = CvLlmError::NoResponse;
        assert_eq!(err2.to_string(), "no response from LLM");
    }
}
