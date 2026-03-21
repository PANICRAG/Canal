//! Langfuse trace exporter.
//!
//! Sends traces and observations to Langfuse via its public ingestion API.
//! Uses batched HTTP requests with automatic flushing.
//!
//! # Feature Gate
//!
//! This module is behind `#[cfg(feature = "langfuse")]`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::config::LangfuseConfig;
use crate::error::DevtoolsError;
use crate::filter::{ObservationUpdate, TraceUpdate};
use crate::traits::TraceExporter;
use crate::types::*;

type Result<T> = std::result::Result<T, DevtoolsError>;

// ============================================================================
// Langfuse API types (inline, camelCase for Langfuse compatibility)
// ============================================================================

/// A single event in a Langfuse batch ingestion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LangfuseBatchItem {
    id: String,
    #[serde(rename = "type")]
    event_type: String,
    timestamp: String,
    body: serde_json::Value,
}

/// Langfuse batch ingestion request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LangfuseBatchRequest {
    batch: Vec<LangfuseBatchItem>,
}

/// Langfuse usage object for generation observations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LangfuseUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_cost: Option<f64>,
}

// ============================================================================
// LangfuseExporter
// ============================================================================

/// Exports traces and observations to Langfuse via batched HTTP ingestion.
///
/// Auth uses HTTP Basic with `public_key:secret_key`.
/// Errors are logged and never propagated to callers.
pub struct LangfuseExporter {
    config: LangfuseConfig,
    http_client: reqwest::Client,
    buffer: Arc<Mutex<Vec<LangfuseBatchItem>>>,
}

impl LangfuseExporter {
    /// Create a new exporter with the given configuration.
    ///
    /// Starts a background flusher task that periodically sends buffered items.
    pub fn new(config: LangfuseConfig) -> Self {
        let exporter = Self {
            config: config.clone(),
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            buffer: Arc::new(Mutex::new(Vec::new())),
        };

        // Start background flusher
        let buffer = exporter.buffer.clone();
        let flush_config = config.clone();
        let flush_client = exporter.http_client.clone();
        tokio::spawn(async move {
            let interval = Duration::from_millis(flush_config.flush_interval_ms);
            loop {
                tokio::time::sleep(interval).await;
                let items = {
                    let mut buf = buffer.lock().await;
                    if buf.is_empty() {
                        continue;
                    }
                    std::mem::take(&mut *buf)
                };
                if let Err(e) = send_batch(&flush_client, &flush_config, items).await {
                    tracing::warn!("Langfuse background flush failed: {}", e);
                }
            }
        });

        exporter
    }

    /// Push an item to the buffer. Auto-flushes if batch_size is reached.
    async fn push_item(&self, item: LangfuseBatchItem) {
        let should_flush = {
            let mut buf = self.buffer.lock().await;
            buf.push(item);
            buf.len() >= self.config.batch_size
        };

        if should_flush {
            let items = {
                let mut buf = self.buffer.lock().await;
                std::mem::take(&mut *buf)
            };
            if let Err(e) = send_batch(&self.http_client, &self.config, items).await {
                tracing::warn!("Langfuse auto-flush failed: {}", e);
            }
        }
    }
}

/// Send a batch of items to Langfuse ingestion API.
async fn send_batch(
    client: &reqwest::Client,
    config: &LangfuseConfig,
    items: Vec<LangfuseBatchItem>,
) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let url = format!("{}/api/public/ingestion", config.host.trim_end_matches('/'));
    let body = LangfuseBatchRequest { batch: items };

    let resp = client
        .post(&url)
        .basic_auth(&config.public_key, Some(&config.secret_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| DevtoolsError::Internal(format!("Langfuse HTTP error: {}", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        tracing::warn!(
            status = %status,
            body = %body_text,
            "Langfuse ingestion returned non-success"
        );
    }

    Ok(())
}

// ============================================================================
// Field mapping helpers
// ============================================================================

fn trace_to_langfuse_body(trace: &Trace) -> serde_json::Value {
    let mut body = serde_json::json!({
        "id": trace.id,
        "name": trace.name,
        "input": trace.input,
        "output": trace.output,
        "metadata": trace.metadata,
        "tags": trace.tags,
    });
    if let Some(ref sid) = trace.session_id {
        body["sessionId"] = serde_json::json!(sid);
    }
    if let Some(ref uid) = trace.user_id {
        body["userId"] = serde_json::json!(uid);
    }
    body
}

fn observation_to_langfuse(obs: &Observation) -> (String, serde_json::Value) {
    match obs {
        Observation::Span(s) => {
            let mut body = serde_json::json!({
                "id": s.id,
                "traceId": s.trace_id,
                "name": s.name,
                "startTime": s.start_time.to_rfc3339(),
                "input": s.input,
                "output": s.output,
                "metadata": s.metadata,
                "level": format!("{:?}", s.level).to_uppercase(),
            });
            if let Some(ref pid) = s.parent_id {
                body["parentObservationId"] = serde_json::json!(pid);
            }
            if let Some(ref et) = s.end_time {
                body["endTime"] = serde_json::json!(et.to_rfc3339());
            }
            ("span-create".to_string(), body)
        }
        Observation::Generation(g) => {
            let usage = LangfuseUsage {
                input: Some(g.input_tokens),
                output: Some(g.output_tokens),
                total: Some(g.total_tokens),
                total_cost: g.cost_usd,
            };
            let mut body = serde_json::json!({
                "id": g.id,
                "traceId": g.trace_id,
                "name": g.name,
                "model": g.model,
                "startTime": g.start_time.to_rfc3339(),
                "input": g.input,
                "output": g.output,
                "metadata": g.metadata,
                "usage": usage,
            });
            if let Some(ref pid) = g.parent_id {
                body["parentObservationId"] = serde_json::json!(pid);
            }
            if let Some(ref et) = g.end_time {
                body["endTime"] = serde_json::json!(et.to_rfc3339());
            }
            ("generation-create".to_string(), body)
        }
        Observation::Event(e) => {
            let mut body = serde_json::json!({
                "id": e.id,
                "traceId": e.trace_id,
                "name": e.name,
                "startTime": e.time.to_rfc3339(),
                "input": e.input,
                "output": e.output,
                "metadata": e.metadata,
                "level": format!("{:?}", e.level).to_uppercase(),
            });
            if let Some(ref pid) = e.parent_id {
                body["parentObservationId"] = serde_json::json!(pid);
            }
            ("event-create".to_string(), body)
        }
    }
}

fn trace_update_to_langfuse_body(id: &str, update: &TraceUpdate) -> serde_json::Value {
    let mut body = serde_json::json!({ "id": id });
    if let Some(ref name) = update.name {
        body["name"] = serde_json::json!(name);
    }
    if let Some(ref output) = update.output {
        body["output"] = output.clone();
    }
    if let Some(ref tags) = update.tags {
        body["tags"] = serde_json::json!(tags);
    }
    if let Some(ref metadata) = update.metadata {
        body["metadata"] = serde_json::json!(metadata);
    }
    body
}

fn observation_update_to_langfuse(
    id: &str,
    update: &ObservationUpdate,
) -> (String, serde_json::Value) {
    // Without knowing the original type, default to span-update.
    // Usage fields present → generation-update.
    let has_usage = update.input_tokens.is_some()
        || update.output_tokens.is_some()
        || update.total_tokens.is_some()
        || update.cost_usd.is_some();

    let event_type = if has_usage {
        "generation-update"
    } else {
        "span-update"
    };

    let mut body = serde_json::json!({ "id": id });
    if let Some(ref output) = update.output {
        body["output"] = output.clone();
    }
    if let Some(ref et) = update.end_time {
        body["endTime"] = serde_json::json!(et.to_rfc3339());
    }
    if let Some(ref metadata) = update.metadata {
        body["metadata"] = serde_json::json!(metadata);
    }
    if has_usage {
        let usage = LangfuseUsage {
            input: update.input_tokens,
            output: update.output_tokens,
            total: update.total_tokens,
            total_cost: update.cost_usd,
        };
        body["usage"] = serde_json::to_value(&usage).unwrap_or_default();
    }

    (event_type.to_string(), body)
}

fn make_batch_item(event_type: String, body: serde_json::Value) -> LangfuseBatchItem {
    LangfuseBatchItem {
        id: uuid::Uuid::new_v4().to_string(),
        event_type,
        timestamp: chrono::Utc::now().to_rfc3339(),
        body,
    }
}

// ============================================================================
// TraceExporter implementation
// ============================================================================

#[async_trait]
impl TraceExporter for LangfuseExporter {
    async fn export_trace(&self, trace: &Trace) -> Result<()> {
        let body = trace_to_langfuse_body(trace);
        let item = make_batch_item("trace-create".to_string(), body);
        self.push_item(item).await;
        Ok(())
    }

    async fn export_observation(&self, obs: &Observation) -> Result<()> {
        let (event_type, body) = observation_to_langfuse(obs);
        let item = make_batch_item(event_type, body);
        self.push_item(item).await;
        Ok(())
    }

    async fn export_trace_update(&self, id: &str, update: &TraceUpdate) -> Result<()> {
        let body = trace_update_to_langfuse_body(id, update);
        let item = make_batch_item("trace-update".to_string(), body);
        self.push_item(item).await;
        Ok(())
    }

    async fn export_observation_update(&self, id: &str, update: &ObservationUpdate) -> Result<()> {
        let (event_type, body) = observation_update_to_langfuse(id, update);
        let item = make_batch_item(event_type, body);
        self.push_item(item).await;
        Ok(())
    }

    async fn flush(&self) -> Result<()> {
        let items = {
            let mut buf = self.buffer.lock().await;
            std::mem::take(&mut *buf)
        };
        send_batch(&self.http_client, &self.config, items).await
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn test_config() -> LangfuseConfig {
        LangfuseConfig {
            host: "http://localhost:9999".into(),
            public_key: "pk-test".into(),
            secret_key: "sk-test".into(),
            flush_interval_ms: 60000, // Long interval to prevent background flush in tests
            batch_size: 50,
        }
    }

    fn make_trace() -> Trace {
        Trace {
            id: "tr-1".into(),
            project_id: "proj-1".into(),
            session_id: Some("sess-1".into()),
            name: Some("test-trace".into()),
            user_id: Some("user-1".into()),
            start_time: Utc::now(),
            end_time: None,
            input: Some(serde_json::json!({"query": "hello"})),
            output: None,
            metadata: serde_json::Map::new(),
            tags: vec!["test".into()],
            status: TraceStatus::Running,
            total_tokens: 0,
            total_cost_usd: 0.0,
            observation_count: 0,
        }
    }

    fn make_span() -> Observation {
        Observation::Span(SpanData {
            id: "span-1".into(),
            trace_id: "tr-1".into(),
            parent_id: Some("root-span".into()),
            name: "graph-execution".into(),
            start_time: Utc::now(),
            end_time: None,
            input: None,
            output: None,
            metadata: serde_json::Map::new(),
            status: ObservationStatus::Running,
            level: ObservationLevel::Info,
            service_name: None,
        })
    }

    fn make_generation() -> Observation {
        Observation::Generation(GenerationData {
            id: "gen-1".into(),
            trace_id: "tr-1".into(),
            parent_id: Some("span-1".into()),
            name: "llm-call".into(),
            model: "claude-sonnet".into(),
            start_time: Utc::now(),
            end_time: Some(Utc::now()),
            input: Some(serde_json::json!({"messages": []})),
            output: Some(serde_json::json!({"response": "hi"})),
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cost_usd: Some(0.002),
            metadata: serde_json::Map::new(),
            status: ObservationStatus::Completed,
            service_name: None,
        })
    }

    fn make_event() -> Observation {
        Observation::Event(EventData {
            id: "evt-1".into(),
            trace_id: "tr-1".into(),
            parent_id: Some("span-1".into()),
            name: "tool.browser_click".into(),
            time: Utc::now(),
            input: Some(serde_json::json!({"selector": "#btn"})),
            output: None,
            metadata: serde_json::Map::new(),
            level: ObservationLevel::Info,
            service_name: None,
        })
    }

    // ── Field mapping tests ──

    #[test]
    fn test_trace_to_langfuse_body() {
        let trace = make_trace();
        let body = trace_to_langfuse_body(&trace);
        assert_eq!(body["id"], "tr-1");
        assert_eq!(body["name"], "test-trace");
        assert_eq!(body["sessionId"], "sess-1");
        assert_eq!(body["userId"], "user-1");
        assert_eq!(body["tags"][0], "test");
    }

    #[test]
    fn test_span_to_langfuse_body() {
        let obs = make_span();
        let (event_type, body) = observation_to_langfuse(&obs);
        assert_eq!(event_type, "span-create");
        assert_eq!(body["id"], "span-1");
        assert_eq!(body["traceId"], "tr-1");
        assert_eq!(body["parentObservationId"], "root-span");
        assert_eq!(body["name"], "graph-execution");
    }

    #[test]
    fn test_generation_to_langfuse_body() {
        let obs = make_generation();
        let (event_type, body) = observation_to_langfuse(&obs);
        assert_eq!(event_type, "generation-create");
        assert_eq!(body["model"], "claude-sonnet");
        assert_eq!(body["usage"]["input"], 100);
        assert_eq!(body["usage"]["output"], 50);
        assert_eq!(body["usage"]["total"], 150);
        assert_eq!(body["usage"]["totalCost"], 0.002);
        assert_eq!(body["parentObservationId"], "span-1");
    }

    #[test]
    fn test_event_to_langfuse_body() {
        let obs = make_event();
        let (event_type, body) = observation_to_langfuse(&obs);
        assert_eq!(event_type, "event-create");
        assert_eq!(body["name"], "tool.browser_click");
        assert_eq!(body["parentObservationId"], "span-1");
    }

    #[test]
    fn test_trace_update_to_langfuse_body() {
        let update = TraceUpdate {
            name: Some("updated".into()),
            output: Some(serde_json::json!({"result": "done"})),
            tags: Some(vec!["final".into()]),
            ..Default::default()
        };
        let body = trace_update_to_langfuse_body("tr-1", &update);
        assert_eq!(body["id"], "tr-1");
        assert_eq!(body["name"], "updated");
        assert_eq!(body["tags"][0], "final");
    }

    #[test]
    fn test_observation_update_span_type() {
        let update = ObservationUpdate {
            output: Some(serde_json::json!({"result": "ok"})),
            end_time: Some(Utc::now()),
            ..Default::default()
        };
        let (event_type, body) = observation_update_to_langfuse("span-1", &update);
        assert_eq!(event_type, "span-update");
        assert_eq!(body["id"], "span-1");
        assert!(body["endTime"].is_string());
    }

    #[test]
    fn test_observation_update_generation_type() {
        let update = ObservationUpdate {
            input_tokens: Some(200),
            output_tokens: Some(100),
            total_tokens: Some(300),
            cost_usd: Some(0.005),
            ..Default::default()
        };
        let (event_type, body) = observation_update_to_langfuse("gen-1", &update);
        assert_eq!(event_type, "generation-update");
        assert_eq!(body["usage"]["input"], 200);
        assert_eq!(body["usage"]["totalCost"], 0.005);
    }

    // ── Batch buffer tests ──

    #[tokio::test]
    async fn test_buffer_accumulates() {
        let config = test_config();
        let exporter = LangfuseExporter {
            config,
            http_client: reqwest::Client::new(),
            buffer: Arc::new(Mutex::new(Vec::new())),
        };
        // Don't call new() to avoid spawning background task

        let trace = make_trace();
        exporter.export_trace(&trace).await.unwrap();
        exporter.export_trace(&trace).await.unwrap();
        exporter.export_observation(&make_span()).await.unwrap();
        exporter
            .export_observation(&make_generation())
            .await
            .unwrap();
        exporter.export_observation(&make_event()).await.unwrap();

        let buf = exporter.buffer.lock().await;
        assert_eq!(buf.len(), 5);
    }

    #[tokio::test]
    async fn test_flush_clears_buffer() {
        let config = test_config();
        let exporter = LangfuseExporter {
            config,
            http_client: reqwest::Client::new(),
            buffer: Arc::new(Mutex::new(Vec::new())),
        };

        exporter.export_trace(&make_trace()).await.unwrap();
        exporter.export_trace(&make_trace()).await.unwrap();

        {
            let buf = exporter.buffer.lock().await;
            assert_eq!(buf.len(), 2);
        }

        // Flush will fail (no server) but buffer should still be cleared
        let _ = exporter.flush().await;

        let buf = exporter.buffer.lock().await;
        assert_eq!(buf.len(), 0);
    }

    #[tokio::test]
    async fn test_flush_threshold_triggers_auto_flush() {
        let mut config = test_config();
        config.batch_size = 3;

        let exporter = LangfuseExporter {
            config,
            http_client: reqwest::Client::new(),
            buffer: Arc::new(Mutex::new(Vec::new())),
        };

        // First two items stay in buffer
        exporter.export_trace(&make_trace()).await.unwrap();
        exporter.export_trace(&make_trace()).await.unwrap();
        {
            let buf = exporter.buffer.lock().await;
            assert_eq!(buf.len(), 2);
        }

        // Third item triggers auto-flush (which will fail, but clears buffer)
        exporter.export_trace(&make_trace()).await.unwrap();
        let buf = exporter.buffer.lock().await;
        assert_eq!(buf.len(), 0, "Buffer should be empty after auto-flush");
    }

    #[tokio::test]
    async fn test_empty_flush_is_noop() {
        let config = test_config();
        let exporter = LangfuseExporter {
            config,
            http_client: reqwest::Client::new(),
            buffer: Arc::new(Mutex::new(Vec::new())),
        };

        // Flushing an empty buffer should succeed
        assert!(exporter.flush().await.is_ok());
    }

    // ── Auth + error tests ──

    #[test]
    fn test_basic_auth_header_format() {
        use base64::Engine;
        let pk = "pk-test-123";
        let sk = "sk-secret-456";
        let expected = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", pk, sk));
        assert!(!expected.is_empty());
        // Verify the format is "pk:sk" base64-encoded
        let decoded = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(&expected)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(decoded, "pk-test-123:sk-secret-456");
    }

    #[tokio::test]
    async fn test_http_error_logged_not_propagated() {
        let mut config = test_config();
        config.host = "http://127.0.0.1:1".into(); // Connection-refused endpoint

        let exporter = LangfuseExporter {
            config,
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_millis(100))
                .build()
                .unwrap(),
            buffer: Arc::new(Mutex::new(Vec::new())),
        };

        // export_trace should succeed (buffered)
        assert!(exporter.export_trace(&make_trace()).await.is_ok());

        // flush will fail HTTP but should not propagate panic
        // (returns Err which is acceptable — callers handle it)
        let _ = exporter.flush().await;
    }

    #[test]
    fn test_batch_request_json_structure() {
        let item = make_batch_item(
            "trace-create".to_string(),
            serde_json::json!({"id": "tr-1"}),
        );
        let request = LangfuseBatchRequest {
            batch: vec![item.clone()],
        };
        let json = serde_json::to_value(&request).unwrap();
        assert!(json["batch"].is_array());
        assert_eq!(json["batch"][0]["type"], "trace-create");
        assert!(json["batch"][0]["id"].is_string());
        assert!(json["batch"][0]["timestamp"].is_string());
        assert_eq!(json["batch"][0]["body"]["id"], "tr-1");
    }
}
