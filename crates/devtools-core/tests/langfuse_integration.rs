//! Integration tests for Langfuse export pipeline.
//!
//! Uses an inline axum mock server to simulate the Langfuse ingestion API.

#![cfg(feature = "langfuse")]

use std::sync::Arc;

use devtools_core::config::LangfuseConfig;
use devtools_core::store::langfuse::LangfuseExporter;
use devtools_core::store::memory::{InMemoryEventBus, InMemoryTraceStore};
use devtools_core::traits::TraceExporter;
use devtools_core::*;

use chrono::Utc;
use tokio::sync::Mutex;

/// Shared state for the mock Langfuse server.
struct MockLangfuseState {
    received_batches: Mutex<Vec<serde_json::Value>>,
    received_auth_headers: Mutex<Vec<String>>,
}

/// Start a mock Langfuse HTTP server that records incoming batch requests.
async fn start_mock_langfuse() -> (String, Arc<MockLangfuseState>) {
    use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};

    let state = Arc::new(MockLangfuseState {
        received_batches: Mutex::new(Vec::new()),
        received_auth_headers: Mutex::new(Vec::new()),
    });

    let app = Router::new()
        .route(
            "/api/public/ingestion",
            post(
                |State(st): State<Arc<MockLangfuseState>>,
                 headers: HeaderMap,
                 Json(body): Json<serde_json::Value>| async move {
                    // Record auth header
                    if let Some(auth) = headers.get("authorization") {
                        st.received_auth_headers
                            .lock()
                            .await
                            .push(auth.to_str().unwrap_or("").to_string());
                    }
                    // Record batch body
                    st.received_batches.lock().await.push(body);
                    Json(serde_json::json!({"successes": [], "errors": []}))
                },
            ),
        )
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Brief wait for server to start
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    (url, state)
}

fn make_config(host: &str) -> LangfuseConfig {
    LangfuseConfig {
        host: host.to_string(),
        public_key: "pk-test-integration".to_string(),
        secret_key: "sk-test-integration".to_string(),
        flush_interval_ms: 60000, // Prevent background flush
        batch_size: 100,
    }
}

fn make_trace(id: &str) -> Trace {
    Trace {
        id: id.into(),
        project_id: "proj-int".into(),
        session_id: Some("sess-int".into()),
        name: Some("integration-test".into()),
        user_id: Some("user-int".into()),
        start_time: Utc::now(),
        end_time: None,
        input: Some(serde_json::json!({"query": "test"})),
        output: None,
        metadata: serde_json::Map::new(),
        tags: vec!["integration".into()],
        status: TraceStatus::Running,
        total_tokens: 0,
        total_cost_usd: 0.0,
        observation_count: 0,
    }
}

fn make_span(id: &str, trace_id: &str, parent_id: Option<&str>) -> Observation {
    Observation::Span(SpanData {
        id: id.into(),
        trace_id: trace_id.into(),
        parent_id: parent_id.map(|s| s.into()),
        name: "test-span".into(),
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

fn make_generation(id: &str, trace_id: &str, parent_id: Option<&str>) -> Observation {
    Observation::Generation(GenerationData {
        id: id.into(),
        trace_id: trace_id.into(),
        parent_id: parent_id.map(|s| s.into()),
        name: "llm-call".into(),
        model: "claude-sonnet".into(),
        start_time: Utc::now(),
        end_time: Some(Utc::now()),
        input: Some(serde_json::json!({"messages": []})),
        output: Some(serde_json::json!({"response": "ok"})),
        input_tokens: 200,
        output_tokens: 100,
        total_tokens: 300,
        cost_usd: Some(0.005),
        metadata: serde_json::Map::new(),
        status: ObservationStatus::Completed,
    })
}

#[tokio::test]
async fn test_full_pipeline_trace_to_langfuse() {
    let (url, mock_state) = start_mock_langfuse().await;
    let config = make_config(&url);
    let exporter = LangfuseExporter::new(config);

    // Ingest a trace + span + generation + event
    exporter
        .export_trace(&make_trace("tr-int-1"))
        .await
        .unwrap();
    exporter
        .export_observation(&make_span("span-1", "tr-int-1", None))
        .await
        .unwrap();
    exporter
        .export_observation(&make_generation("gen-1", "tr-int-1", Some("span-1")))
        .await
        .unwrap();
    exporter
        .export_observation(&Observation::Event(EventData {
            id: "evt-1".into(),
            trace_id: "tr-int-1".into(),
            parent_id: Some("span-1".into()),
            name: "tool.test".into(),
            time: Utc::now(),
            input: None,
            output: None,
            metadata: serde_json::Map::new(),
            level: ObservationLevel::Info,
            service_name: None,
        }))
        .await
        .unwrap();

    // Flush to send
    exporter.flush().await.unwrap();

    // Verify mock received the batch
    let batches = mock_state.received_batches.lock().await;
    assert_eq!(batches.len(), 1);
    let batch = &batches[0]["batch"];
    assert_eq!(batch.as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn test_parent_child_hierarchy_preserved() {
    let (url, mock_state) = start_mock_langfuse().await;
    let config = make_config(&url);
    let exporter = LangfuseExporter::new(config);

    // Create nested spans
    exporter
        .export_observation(&make_span("parent-span", "tr-1", None))
        .await
        .unwrap();
    exporter
        .export_observation(&make_span("child-span", "tr-1", Some("parent-span")))
        .await
        .unwrap();

    exporter.flush().await.unwrap();

    let batches = mock_state.received_batches.lock().await;
    let batch = batches[0]["batch"].as_array().unwrap();

    // First item: no parent
    assert!(batch[0]["body"]["parentObservationId"].is_null());
    // Second item: parent set
    assert_eq!(batch[1]["body"]["parentObservationId"], "parent-span");
}

#[tokio::test]
async fn test_trace_update_flow() {
    let (url, mock_state) = start_mock_langfuse().await;
    let config = make_config(&url);
    let exporter = LangfuseExporter::new(config);

    // Create + update
    exporter.export_trace(&make_trace("tr-upd")).await.unwrap();
    exporter
        .export_trace_update(
            "tr-upd",
            &TraceUpdate {
                status: Some(TraceStatus::Completed),
                output: Some(serde_json::json!({"result": "done"})),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    exporter.flush().await.unwrap();

    let batches = mock_state.received_batches.lock().await;
    let batch = batches[0]["batch"].as_array().unwrap();
    assert_eq!(batch.len(), 2);
    assert_eq!(batch[0]["type"], "trace-create");
    assert_eq!(batch[1]["type"], "trace-update");
}

#[tokio::test]
async fn test_generation_usage_fields() {
    let (url, mock_state) = start_mock_langfuse().await;
    let config = make_config(&url);
    let exporter = LangfuseExporter::new(config);

    exporter
        .export_observation(&make_generation("gen-usage", "tr-1", None))
        .await
        .unwrap();

    exporter.flush().await.unwrap();

    let batches = mock_state.received_batches.lock().await;
    let item = &batches[0]["batch"][0];
    assert_eq!(item["type"], "generation-create");
    assert_eq!(item["body"]["usage"]["input"], 200);
    assert_eq!(item["body"]["usage"]["output"], 100);
    assert_eq!(item["body"]["usage"]["total"], 300);
    assert_eq!(item["body"]["usage"]["totalCost"], 0.005);
}

#[tokio::test]
async fn test_auth_header_correct() {
    let (url, mock_state) = start_mock_langfuse().await;
    let config = make_config(&url);
    let exporter = LangfuseExporter::new(config);

    exporter.export_trace(&make_trace("tr-auth")).await.unwrap();
    exporter.flush().await.unwrap();

    let headers = mock_state.received_auth_headers.lock().await;
    assert_eq!(headers.len(), 1);
    // Verify Basic auth format
    assert!(headers[0].starts_with("Basic "));
    let encoded = &headers[0]["Basic ".len()..];
    let decoded = String::from_utf8(
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded).unwrap(),
    )
    .unwrap();
    assert_eq!(decoded, "pk-test-integration:sk-test-integration");
}

#[tokio::test]
async fn test_batch_size_triggers_flush() {
    let (url, mock_state) = start_mock_langfuse().await;
    let mut config = make_config(&url);
    config.batch_size = 3;
    let exporter = LangfuseExporter::new(config);

    // Push 3 items — should auto-flush
    exporter.export_trace(&make_trace("tr-1")).await.unwrap();
    exporter.export_trace(&make_trace("tr-2")).await.unwrap();
    exporter.export_trace(&make_trace("tr-3")).await.unwrap();

    // Brief wait for the auto-flush HTTP call
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let batches = mock_state.received_batches.lock().await;
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0]["batch"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn test_store_and_exporter_both_receive_data() {
    let (url, mock_state) = start_mock_langfuse().await;
    let config = make_config(&url);
    let exporter = Arc::new(LangfuseExporter::new(config));

    let store = Arc::new(InMemoryTraceStore::new(100));
    let bus = Arc::new(InMemoryEventBus::new());
    let svc = DevtoolsService::new(store.clone(), bus)
        .with_exporters(vec![exporter.clone() as Arc<dyn TraceExporter>]);

    svc.ingest_trace(make_trace("tr-dual")).await.unwrap();
    svc.ingest_observation(make_span("span-dual", "tr-dual", None))
        .await
        .unwrap();

    // Store has data
    let trace = svc.get_trace("tr-dual").await.unwrap();
    assert!(trace.is_some());

    // Flush exporter to send to mock
    exporter.flush().await.unwrap();

    // Mock received data
    let batches = mock_state.received_batches.lock().await;
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0]["batch"].as_array().unwrap().len(), 2);
}
