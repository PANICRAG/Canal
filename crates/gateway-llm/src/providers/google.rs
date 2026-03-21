//! Google AI (Gemini) provider

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::router::{ChatRequest, ChatResponse, Choice, LlmProvider, Message, StopReason, Usage};

/// Google AI API configuration
#[derive(Debug, Clone)]
pub struct GoogleAIConfig {
    pub api_key: String,
    pub base_url: String,
    pub default_model: String,
}

impl Default for GoogleAIConfig {
    fn default() -> Self {
        Self {
            api_key: std::env::var("GOOGLE_AI_API_KEY").unwrap_or_default(),
            base_url: "https://generativelanguage.googleapis.com/v1".to_string(),
            default_model: "gemini-3-pro".to_string(),
        }
    }
}

/// Google AI provider
pub struct GoogleAIProvider {
    client: Client,
    config: GoogleAIConfig,
}

impl GoogleAIProvider {
    /// Create a new Google AI provider with default configuration
    pub fn new() -> Self {
        Self::with_config(GoogleAIConfig::default())
    }

    /// Create a new Google AI provider with custom configuration
    pub fn with_config(config: GoogleAIConfig) -> Self {
        Self {
            client: super::shared_http_client(),
            config,
        }
    }
}

impl Default for GoogleAIProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
    /// R3-M: System instruction for Gemini (equivalent of system messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiPart {
    text: String,
}

#[derive(Debug, Serialize)]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiUsage {
    #[serde(default)]
    prompt_token_count: i32,
    #[serde(default)]
    candidates_token_count: i32,
    #[serde(default)]
    total_token_count: i32,
}

#[derive(Debug, Deserialize)]
struct GeminiError {
    error: GeminiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct GeminiErrorDetail {
    message: String,
    #[allow(dead_code)]
    #[serde(default)]
    status: Option<String>,
}

#[async_trait]
impl LlmProvider for GoogleAIProvider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let model = request
            .model
            .unwrap_or_else(|| self.config.default_model.clone());

        // R3-M: Extract system messages into Gemini's system_instruction field
        let system_text: String = request
            .messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let system_instruction = if system_text.is_empty() {
            None
        } else {
            Some(GeminiContent {
                role: "user".to_string(),
                parts: vec![GeminiPart { text: system_text }],
            })
        };

        // Convert non-system messages to Gemini format
        let contents: Vec<GeminiContent> = request
            .messages
            .into_iter()
            .filter(|m| m.role != "system")
            .map(|m| GeminiContent {
                role: if m.role == "assistant" {
                    "model".to_string()
                } else {
                    "user".to_string()
                },
                parts: vec![GeminiPart { text: m.content }],
            })
            .collect();

        let generation_config = if request.temperature.is_some() || request.max_tokens.is_some() {
            Some(GenerationConfig {
                temperature: request.temperature,
                max_output_tokens: request.max_tokens,
            })
        } else {
            None
        };

        let gemini_request = GeminiRequest {
            contents,
            generation_config,
            system_instruction,
        };

        let url = format!("{}/models/{}:generateContent", self.config.base_url, model);

        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .header("x-goog-api-key", &self.config.api_key)
            .json(&gemini_request)
            .send()
            .await?;

        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();

            // Try to parse as Gemini error
            if let Ok(error_response) = serde_json::from_str::<GeminiError>(&error_text) {
                return Err(Error::Llm(format!(
                    "Google AI API error: {}",
                    error_response.error.message
                )));
            }

            return Err(Error::Llm(format!(
                "Google AI API error: {} - {}",
                status, error_text
            )));
        }

        let gemini_response: GeminiResponse = response
            .json()
            .await
            .map_err(|e| Error::Llm(format!("Failed to parse Google AI response: {}", e)))?;

        let candidate = gemini_response
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| Error::Llm("No response from Google AI".to_string()))?;

        let content = candidate
            .content
            .parts
            .into_iter()
            .map(|p| p.text)
            .collect::<Vec<_>>()
            .join("");

        let usage = gemini_response.usage_metadata.unwrap_or(GeminiUsage {
            prompt_token_count: 0,
            candidates_token_count: 0,
            total_token_count: 0,
        });

        Ok(ChatResponse {
            id: uuid::Uuid::new_v4().to_string(),
            model,
            choices: vec![Choice {
                index: 0,
                message: Message::text("assistant", content),
                finish_reason: candidate
                    .finish_reason
                    .clone()
                    .unwrap_or_else(|| "stop".to_string()),
                stop_reason: Some(StopReason::EndTurn), // Google doesn't have tool_use in basic API
            }],
            usage: Usage {
                prompt_tokens: usage.prompt_token_count,
                completion_tokens: usage.candidates_token_count,
                total_tokens: usage.total_token_count,
            },
        })
    }

    fn name(&self) -> &str {
        "google"
    }

    async fn is_available(&self) -> bool {
        !self.config.api_key.is_empty()
    }
}
