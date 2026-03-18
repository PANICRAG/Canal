//! Embedding providers for the semantic cache (L2).
//!
//! Provides the [`EmbeddingProvider`] trait for generating text embeddings,
//! along with a [`RemoteEmbeddingProvider`] that calls OpenAI-compatible
//! embedding APIs and a [`MockEmbeddingProvider`] for deterministic testing.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the embedding provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Provider type (currently only `"remote"` is supported).
    #[serde(default = "default_provider")]
    pub provider: String,

    /// Embedding model name.
    #[serde(default = "default_model")]
    pub model: String,

    /// Base URL for the embedding API (OpenAI-compatible).
    #[serde(default = "default_api_url")]
    pub api_url: String,

    /// API key for authentication.
    #[serde(default)]
    pub api_key: String,

    /// Embedding vector dimension.
    #[serde(default = "default_dimension")]
    pub dimension: usize,
}

fn default_provider() -> String {
    "remote".to_string()
}

fn default_model() -> String {
    "text-embedding-3-small".to_string()
}

fn default_api_url() -> String {
    "https://api.openai.com/v1/embeddings".to_string()
}

fn default_dimension() -> usize {
    1536
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            model: default_model(),
            api_url: default_api_url(),
            api_key: String::new(),
            dimension: default_dimension(),
        }
    }
}

// ---------------------------------------------------------------------------
// EmbeddingProvider trait
// ---------------------------------------------------------------------------

/// Trait for generating text embeddings.
///
/// Implementations must be `Send + Sync` so they can be shared across
/// async tasks behind an `Arc`.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single text into a floating-point vector.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Embed multiple texts in a single batch request.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// Return the dimensionality of the embedding vectors produced.
    fn dimension(&self) -> usize;
}

// ---------------------------------------------------------------------------
// RemoteEmbeddingProvider
// ---------------------------------------------------------------------------

/// Embedding provider that calls an OpenAI-compatible embedding API.
///
/// Sends HTTP POST requests to the configured `api_url` with the model and
/// input text(s), then parses the standard OpenAI embedding response format.
///
/// # Example
///
/// ```no_run
/// use gateway_memory::cache::embedding::{EmbeddingConfig, EmbeddingProvider, RemoteEmbeddingProvider};
///
/// # async fn example() -> gateway_memory::Result<()> {
/// let config = EmbeddingConfig {
///     api_key: "sk-...".to_string(),
///     ..Default::default()
/// };
/// let provider = RemoteEmbeddingProvider::new(config);
/// let vector = provider.embed("hello world").await?;
/// assert_eq!(vector.len(), 1536);
/// # Ok(())
/// # }
/// ```
pub struct RemoteEmbeddingProvider {
    client: reqwest::Client,
    api_url: String,
    api_key: String,
    model: String,
    dim: usize,
}

impl RemoteEmbeddingProvider {
    /// Create a new remote embedding provider from the given configuration.
    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_url: config.api_url,
            api_key: config.api_key,
            model: config.model,
            dim: config.dimension,
        }
    }
}

/// Response format from an OpenAI-compatible embedding endpoint.
#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

/// A single embedding entry in the response.
#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbeddingProvider for RemoteEmbeddingProvider {
    #[tracing::instrument(skip(self, text), fields(model = %self.model))]
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let body = serde_json::json!({
            "model": self.model,
            "input": text,
        });

        let response = self
            .client
            .post(&self.api_url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable>".to_string());
            // Log the response body at debug level only to avoid leaking
            // API keys or other sensitive data in error messages.
            tracing::debug!(body = %body_text, "Embedding API error body");
            return Err(Error::Internal(format!("Embedding API returned {status}")));
        }

        let resp: EmbeddingResponse = response.json().await?;
        resp.data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| Error::Internal("Embedding API returned empty data array".to_string()))
    }

    #[tracing::instrument(skip(self, texts), fields(model = %self.model, count = texts.len()))]
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let response = self
            .client
            .post(&self.api_url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable>".to_string());
            tracing::debug!(body = %body_text, "Embedding API batch error body");
            return Err(Error::Internal(format!("Embedding API returned {status}")));
        }

        let resp: EmbeddingResponse = response.json().await?;
        Ok(resp.data.into_iter().map(|d| d.embedding).collect())
    }

    fn dimension(&self) -> usize {
        self.dim
    }
}

// ---------------------------------------------------------------------------
// MockEmbeddingProvider
// ---------------------------------------------------------------------------

/// A deterministic embedding provider for testing.
///
/// Generates vectors by hashing the input text and distributing the hash
/// bytes across the vector dimensions, then normalizing to a unit vector.
/// This ensures that:
/// - The same text always produces the same vector.
/// - Different texts produce different vectors (with high probability).
/// - No real API calls are made.
pub struct MockEmbeddingProvider {
    dim: usize,
}

impl MockEmbeddingProvider {
    /// Create a mock provider that returns vectors of the given dimension.
    pub fn new(dimension: usize) -> Self {
        Self { dim: dimension }
    }

    /// Generate a deterministic vector from text by hashing.
    fn hash_to_vector(&self, text: &str) -> Vec<f32> {
        // Simple deterministic hash: use bytes of the text to seed values.
        // We cycle through the text bytes to fill all dimensions.
        let text_bytes = text.as_bytes();
        if text_bytes.is_empty() {
            // Return a uniform unit vector for empty text.
            let val = 1.0 / (self.dim as f32).sqrt();
            return vec![val; self.dim];
        }

        let mut raw: Vec<f32> = Vec::with_capacity(self.dim);
        for i in 0..self.dim {
            // Combine position and multiple byte values for better spread.
            let b1 = text_bytes[i % text_bytes.len()] as u32;
            let b2 = text_bytes[(i + 1) % text_bytes.len()] as u32;
            let b3 = text_bytes[(i + 7) % text_bytes.len()] as u32;
            // Mix the values to produce a pseudo-random float in [-1, 1].
            let mixed = ((b1.wrapping_mul(31))
                .wrapping_add(b2.wrapping_mul(127))
                .wrapping_add(b3.wrapping_mul(251))
                .wrapping_add(i as u32 * 17)) as f32;
            raw.push((mixed % 256.0) / 128.0 - 1.0);
        }

        // Normalize to unit vector.
        let magnitude: f32 = raw.iter().map(|x| x * x).sum::<f32>().sqrt();
        if magnitude > 0.0 {
            for v in &mut raw {
                *v /= magnitude;
            }
        }

        raw
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    #[tracing::instrument(skip(self, text), fields(provider = "mock"))]
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self.hash_to_vector(text))
    }

    #[tracing::instrument(skip(self, texts), fields(provider = "mock", count = texts.len()))]
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.hash_to_vector(t)).collect())
    }

    fn dimension(&self) -> usize {
        self.dim
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_provider_embed() {
        let provider = MockEmbeddingProvider::new(384);
        let vec = provider.embed("hello world").await.unwrap();
        assert_eq!(vec.len(), 384);

        // Verify unit vector (magnitude ~1.0).
        let mag: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((mag - 1.0).abs() < 1e-5, "magnitude was {mag}");
    }

    #[tokio::test]
    async fn test_mock_provider_batch() {
        let provider = MockEmbeddingProvider::new(256);
        let texts: Vec<&str> = vec!["alpha", "beta", "gamma"];
        let result = provider.embed_batch(&texts).await.unwrap();
        assert_eq!(result.len(), 3);
        for v in &result {
            assert_eq!(v.len(), 256);
        }
    }

    #[tokio::test]
    async fn test_mock_different_texts_different_vectors() {
        let provider = MockEmbeddingProvider::new(128);
        let v1 = provider.embed("the quick brown fox").await.unwrap();
        let v2 = provider.embed("jumps over the lazy dog").await.unwrap();
        let v3 = provider.embed("the quick brown fox").await.unwrap();

        // Same text should produce identical vectors.
        assert_eq!(v1, v3, "same text must produce identical vectors");

        // Different text should produce different vectors.
        assert_ne!(v1, v2, "different texts should produce different vectors");
    }

    #[test]
    fn test_embedding_config_serialize() {
        let config = EmbeddingConfig {
            provider: "remote".to_string(),
            model: "text-embedding-3-large".to_string(),
            api_url: "https://custom.api/embeddings".to_string(),
            api_key: "sk-test-key".to_string(),
            dimension: 3072,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: EmbeddingConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.provider, "remote");
        assert_eq!(deserialized.model, "text-embedding-3-large");
        assert_eq!(deserialized.api_url, "https://custom.api/embeddings");
        assert_eq!(deserialized.api_key, "sk-test-key");
        assert_eq!(deserialized.dimension, 3072);
    }

    #[test]
    fn test_embedding_config_defaults() {
        let config: EmbeddingConfig = serde_json::from_str("{}").unwrap();

        assert_eq!(config.provider, "remote");
        assert_eq!(config.model, "text-embedding-3-small");
        assert_eq!(config.api_url, "https://api.openai.com/v1/embeddings");
        assert_eq!(config.api_key, "");
        assert_eq!(config.dimension, 1536);
    }

    #[tokio::test]
    async fn test_mock_provider_empty_batch() {
        let provider = MockEmbeddingProvider::new(64);
        let result = provider.embed_batch(&[]).await.unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_mock_provider_dimension() {
        let provider = MockEmbeddingProvider::new(768);
        assert_eq!(provider.dimension(), 768);
    }

    #[tokio::test]
    async fn test_mock_provider_empty_text() {
        let provider = MockEmbeddingProvider::new(128);
        let vec = provider.embed("").await.unwrap();
        assert_eq!(vec.len(), 128);

        // Empty text should still produce a unit vector.
        let mag: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((mag - 1.0).abs() < 1e-5, "magnitude was {mag}");
    }
}
