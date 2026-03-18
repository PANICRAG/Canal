//! HTTP ingest client for sending traces to a remote devtools-server.
//!
//! Feature-gated behind `client` (requires `reqwest`).
//!
//! # Example
//!
//! ```ignore
//! let client = DevtoolsClient::new(
//!     "http://devtools-server:4200",
//!     "pk_proj_engine_xxxx",
//!     "engine-server-prod",
//! );
//! client.trace(trace).await?;
//! client.observation(obs).await?;
//! client.flush().await?;
//! ```

use std::sync::Arc;
use tokio::sync::Mutex;

use crate::error::DevtoolsError;
use crate::types::{IngestBatch, Observation, Trace};

type Result<T> = std::result::Result<T, DevtoolsError>;

/// Lightweight HTTP client for sending traces to devtools-server.
pub struct DevtoolsClient {
    endpoint: String,
    api_key: String,
    project_id: String,
    http_client: reqwest::Client,
    batch_buffer: Arc<Mutex<IngestBatch>>,
}

impl DevtoolsClient {
    /// Create a new client pointed at a devtools-server endpoint.
    pub fn new(endpoint: &str, api_key: &str, project_id: &str) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            project_id: project_id.to_string(),
            http_client: reqwest::Client::new(),
            batch_buffer: Arc::new(Mutex::new(IngestBatch {
                traces: Vec::new(),
                observations: Vec::new(),
            })),
        }
    }

    /// Queue a trace for batch sending.
    pub async fn trace(&self, mut trace: Trace) -> Result<()> {
        trace.project_id = self.project_id.clone();
        let mut buf = self.batch_buffer.lock().await;
        buf.traces.push(trace);
        Ok(())
    }

    /// Queue an observation for batch sending.
    pub async fn observation(&self, obs: Observation) -> Result<()> {
        let mut buf = self.batch_buffer.lock().await;
        buf.observations.push(obs);
        Ok(())
    }

    /// Send a single trace immediately (non-batched).
    pub async fn send_trace(&self, mut trace: Trace) -> Result<()> {
        trace.project_id = self.project_id.clone();
        let url = format!("{}/v1/traces", self.endpoint);
        let resp = self
            .http_client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&trace)
            .send()
            .await
            .map_err(|e| DevtoolsError::Internal(format!("HTTP error: {}", e)))?;
        if !resp.status().is_success() {
            return Err(DevtoolsError::Internal(format!(
                "send_trace failed with status {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /// Send a single observation immediately (non-batched).
    pub async fn send_observation(&self, obs: Observation) -> Result<()> {
        let url = format!("{}/v1/observations", self.endpoint);
        let resp = self
            .http_client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&obs)
            .send()
            .await
            .map_err(|e| DevtoolsError::Internal(format!("HTTP error: {}", e)))?;
        if !resp.status().is_success() {
            return Err(DevtoolsError::Internal(format!(
                "send_observation failed with status {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /// Flush the batch buffer, sending all queued traces and observations.
    pub async fn flush(&self) -> Result<()> {
        let batch = {
            let mut buf = self.batch_buffer.lock().await;
            let batch = IngestBatch {
                traces: std::mem::take(&mut buf.traces),
                observations: std::mem::take(&mut buf.observations),
            };
            batch
        };

        if batch.traces.is_empty() && batch.observations.is_empty() {
            return Ok(());
        }

        let url = format!("{}/v1/ingest", self.endpoint);
        let resp = self
            .http_client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&batch)
            .send()
            .await
            .map_err(|e| DevtoolsError::Internal(format!("HTTP error: {}", e)))?;
        if !resp.status().is_success() {
            return Err(DevtoolsError::Internal(format!(
                "flush failed with status {}",
                resp.status()
            )));
        }

        Ok(())
    }
}
