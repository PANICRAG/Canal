//! Integration tests for devtools-server HTTP API.

use devtools_core::config::DevtoolsConfig;
use devtools_core::store::memory::{InMemoryEventBus, InMemoryTraceStore};
use devtools_core::types::*;
use devtools_core::DevtoolsService;
use canal_identity::{DashMapKeyStore, IdentityService, KeyStore};
use std::sync::Arc;

// Re-use the server modules
// We need to build the app manually
mod helpers {
    use super::*;

    pub struct TestServer {
        pub addr: std::net::SocketAddr,
        pub client: reqwest::Client,
        pub api_key: String,
    }

    impl TestServer {
        pub fn url(&self, path: &str) -> String {
            format!("http://{}{}", self.addr, path)
        }
    }

    pub async fn spawn_test_server() -> TestServer {
        let api_key = "test-api-key-12345";

        let identity_service = {
            let key_hash = canal_identity::key_gen::hash_key(api_key);
            let store: Arc<dyn KeyStore> = Arc::new(DashMapKeyStore::with_system_key(
                &key_hash,
                "test-api-key...",
            ));
            Arc::new(IdentityService::new(store))
        };

        let trace_store = Arc::new(InMemoryTraceStore::new(1000));
        let event_bus = Arc::new(InMemoryEventBus::new());
        let devtools_service = Arc::new(DevtoolsService::new(trace_store, event_bus));

        let config = DevtoolsConfig::default();

        // Build the state — we need to replicate the AppState structure
        // Since we can't import private modules from the binary, we build the router directly
        let state = Arc::new(devtools_server_test::TestAppState {
            devtools: devtools_service,
            identity_service: identity_service.clone(),
            config,
        });

        let app = devtools_server_test::build_test_router(state, identity_service);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Give the server a moment to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        TestServer {
            addr,
            client: reqwest::Client::new(),
            api_key: api_key.to_string(),
        }
    }
}

/// Since we can't import private modules from the bin crate,
/// we rebuild the router inline for testing.
mod devtools_server_test {
    use super::*;
    use axum::extract::{Path, Query, State};
    use axum::http::StatusCode;
    use axum::middleware;
    use axum::response::sse::{Event, Sse};
    use axum::response::IntoResponse;
    use axum::routing::{delete, get, post};
    use axum::{Json, Router};
    use chrono::Utc;
    use devtools_core::filter::{MetricsFilter, TraceFilter};
    use serde::Deserialize;
    use std::convert::Infallible;
    use tokio_stream::wrappers::ReceiverStream;
    use tokio_stream::StreamExt;
    use uuid::Uuid;

    pub struct TestAppState {
        pub devtools: Arc<DevtoolsService>,
        pub identity_service: Arc<IdentityService>,
        pub config: DevtoolsConfig,
    }

    pub fn build_test_router(
        state: Arc<TestAppState>,
        identity_service: Arc<IdentityService>,
    ) -> Router {
        let public = Router::new().route("/v1/health", get(health));

        let protected = Router::new()
            .route("/v1/traces", post(create_trace))
            .route("/v1/observations", post(create_observation))
            .route("/v1/ingest", post(batch_ingest))
            .route("/v1/traces", get(list_traces))
            .route("/v1/traces/{id}", get(get_trace))
            .route("/v1/traces/{id}/export", get(export_trace))
            .route("/v1/sessions", get(list_sessions))
            .route("/v1/sessions/{id}/traces", get(get_session_traces))
            .route("/v1/metrics/summary", get(get_metrics_summary))
            .route("/v1/projects", post(create_project))
            .route("/v1/projects", get(list_projects))
            .route("/v1/projects/{id}", get(get_project))
            .route("/v1/projects/{id}", delete(delete_project))
            .route_layer(middleware::from_fn_with_state(
                identity_service,
                require_auth,
            ));

        public.merge(protected).with_state(state)
    }

    async fn require_auth(
        State(identity_service): State<Arc<IdentityService>>,
        req: axum::http::Request<axum::body::Body>,
        next: middleware::Next,
    ) -> Result<axum::response::Response, axum::response::Response> {
        let auth_header = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));

        let api_key = match auth_header {
            Some(key) if !key.is_empty() => key,
            _ => {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({"error": "Missing Authorization"})),
                )
                    .into_response());
            }
        };

        match identity_service.resolve(api_key).await {
            Ok(_) => Ok(next.run(req).await),
            Err(e) => Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": format!("{}", e)})),
            )
                .into_response()),
        }
    }

    async fn health(State(_state): State<Arc<TestAppState>>) -> Json<serde_json::Value> {
        Json(serde_json::json!({"status": "ok", "service": "devtools-server"}))
    }

    async fn create_trace(
        State(state): State<Arc<TestAppState>>,
        Json(trace): Json<Trace>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        state
            .devtools
            .ingest_trace(trace)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Json(serde_json::json!({"status": "ok"})))
    }

    async fn create_observation(
        State(state): State<Arc<TestAppState>>,
        Json(obs): Json<Observation>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        state
            .devtools
            .ingest_observation(obs)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Json(serde_json::json!({"status": "ok"})))
    }

    async fn batch_ingest(
        State(state): State<Arc<TestAppState>>,
        Json(batch): Json<IngestBatch>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        state
            .devtools
            .ingest_batch(batch)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Json(serde_json::json!({"status": "ok"})))
    }

    async fn list_traces(
        State(state): State<Arc<TestAppState>>,
        Query(filter): Query<TraceFilter>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        let traces = state
            .devtools
            .list_traces(filter)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Json(
            serde_json::json!({"data": traces, "count": traces.len()}),
        ))
    }

    async fn get_trace(
        State(state): State<Arc<TestAppState>>,
        Path(id): Path<String>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        let tree = state
            .devtools
            .get_trace_tree(&id)
            .await
            .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
        Ok(Json(serde_json::json!(tree)))
    }

    async fn export_trace(
        State(state): State<Arc<TestAppState>>,
        Path(id): Path<String>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        let export = state
            .devtools
            .export_trace(&id)
            .await
            .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
        Ok(Json(export))
    }

    #[derive(Deserialize)]
    pub struct SessionListQuery {
        pub project_id: Option<String>,
        #[serde(default = "default_50")]
        pub limit: usize,
    }
    fn default_50() -> usize {
        50
    }

    async fn list_sessions(
        State(state): State<Arc<TestAppState>>,
        Query(q): Query<SessionListQuery>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        let sessions = state
            .devtools
            .list_sessions(q.project_id.as_deref(), q.limit)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Json(
            serde_json::json!({"data": sessions, "count": sessions.len()}),
        ))
    }

    async fn get_session_traces(
        State(state): State<Arc<TestAppState>>,
        Path(id): Path<String>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        let traces = state
            .devtools
            .get_session_traces(&id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Json(
            serde_json::json!({"data": traces, "count": traces.len()}),
        ))
    }

    async fn get_metrics_summary(
        State(state): State<Arc<TestAppState>>,
        Query(filter): Query<MetricsFilter>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        let metrics = state
            .devtools
            .get_metrics(filter)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Json(serde_json::json!(metrics)))
    }

    #[derive(Deserialize)]
    pub struct CreateProjectReq {
        pub name: String,
        pub service_type: String,
        pub endpoint: Option<String>,
    }

    async fn create_project(
        State(state): State<Arc<TestAppState>>,
        Json(req): Json<CreateProjectReq>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        let id = req.name.to_lowercase().replace(' ', "-");
        let api_key = format!("pk_proj_{}", id);
        let project = Project {
            id: id.clone(),
            name: req.name,
            service_type: req.service_type,
            endpoint: req.endpoint,
            api_key: api_key.clone(),
            created_at: Utc::now(),
            metadata: serde_json::Map::new(),
        };
        state
            .devtools
            .create_project(project)
            .await
            .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;
        Ok((
            StatusCode::CREATED,
            Json(serde_json::json!({"id": id, "api_key": api_key})),
        ))
    }

    async fn list_projects(
        State(state): State<Arc<TestAppState>>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        let projects = state
            .devtools
            .list_projects()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Json(
            serde_json::json!({"data": projects, "count": projects.len()}),
        ))
    }

    async fn get_project(
        State(state): State<Arc<TestAppState>>,
        Path(id): Path<String>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        match state.devtools.get_project(&id).await {
            Ok(Some(p)) => Ok(Json(serde_json::json!(p))),
            Ok(None) => Err((StatusCode::NOT_FOUND, format!("project not found: {}", id))),
            Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
        }
    }

    async fn delete_project(
        State(state): State<Arc<TestAppState>>,
        Path(id): Path<String>,
    ) -> Result<impl IntoResponse, (StatusCode, String)> {
        state
            .devtools
            .delete_project(&id)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        Ok(Json(serde_json::json!({"status": "deleted"})))
    }
}

use helpers::*;

fn make_trace(id: &str, project_id: &str) -> Trace {
    Trace {
        id: id.into(),
        project_id: project_id.into(),
        session_id: Some("sess-1".into()),
        name: Some("test-trace".into()),
        user_id: None,
        start_time: chrono::Utc::now(),
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

#[tokio::test]
async fn test_health_endpoint() {
    let server = spawn_test_server().await;
    let resp = server
        .client
        .get(server.url("/v1/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn test_ingest_trace_unauthorized() {
    let server = spawn_test_server().await;
    let resp = server
        .client
        .post(server.url("/v1/traces"))
        .json(&make_trace("tr-1", "proj-1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_ingest_trace_success() {
    let server = spawn_test_server().await;
    let resp = server
        .client
        .post(server.url("/v1/traces"))
        .bearer_auth(&server.api_key)
        .json(&make_trace("tr-1", "proj-1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_ingest_observation_success() {
    let server = spawn_test_server().await;

    // First create a trace
    server
        .client
        .post(server.url("/v1/traces"))
        .bearer_auth(&server.api_key)
        .json(&make_trace("tr-1", "proj-1"))
        .send()
        .await
        .unwrap();

    // Then add an observation
    let obs = Observation::Generation(GenerationData {
        id: "gen-1".into(),
        trace_id: "tr-1".into(),
        parent_id: None,
        name: "llm-call".into(),
        model: "claude-sonnet".into(),
        start_time: chrono::Utc::now(),
        end_time: Some(chrono::Utc::now()),
        input: None,
        output: None,
        input_tokens: 100,
        output_tokens: 50,
        total_tokens: 150,
        cost_usd: Some(0.002),
        metadata: serde_json::Map::new(),
        status: ObservationStatus::Completed,
        service_name: None,
    });

    let resp = server
        .client
        .post(server.url("/v1/observations"))
        .bearer_auth(&server.api_key)
        .json(&obs)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_batch_ingest() {
    let server = spawn_test_server().await;
    let batch = IngestBatch {
        traces: vec![make_trace("tr-1", "proj-1"), make_trace("tr-2", "proj-1")],
        observations: vec![],
    };
    let resp = server
        .client
        .post(server.url("/v1/ingest"))
        .bearer_auth(&server.api_key)
        .json(&batch)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_list_traces() {
    let server = spawn_test_server().await;

    // Ingest two traces
    for id in ["tr-1", "tr-2"] {
        server
            .client
            .post(server.url("/v1/traces"))
            .bearer_auth(&server.api_key)
            .json(&make_trace(id, "proj-1"))
            .send()
            .await
            .unwrap();
    }

    let resp = server
        .client
        .get(server.url("/v1/traces?project_id=proj-1"))
        .bearer_auth(&server.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["count"], 2);
}

#[tokio::test]
async fn test_get_trace_with_observations() {
    let server = spawn_test_server().await;

    server
        .client
        .post(server.url("/v1/traces"))
        .bearer_auth(&server.api_key)
        .json(&make_trace("tr-1", "proj-1"))
        .send()
        .await
        .unwrap();

    let obs = Observation::Span(SpanData {
        id: "span-1".into(),
        trace_id: "tr-1".into(),
        parent_id: None,
        name: "ANALYZE".into(),
        start_time: chrono::Utc::now(),
        end_time: None,
        input: None,
        output: None,
        metadata: serde_json::Map::new(),
        status: ObservationStatus::Running,
        level: ObservationLevel::Info,
        service_name: None,
    });
    server
        .client
        .post(server.url("/v1/observations"))
        .bearer_auth(&server.api_key)
        .json(&obs)
        .send()
        .await
        .unwrap();

    let resp = server
        .client
        .get(server.url("/v1/traces/tr-1"))
        .bearer_auth(&server.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["trace"]["id"], "tr-1");
    assert_eq!(body["observations"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_trace_export() {
    let server = spawn_test_server().await;

    server
        .client
        .post(server.url("/v1/traces"))
        .bearer_auth(&server.api_key)
        .json(&make_trace("tr-1", "proj-1"))
        .send()
        .await
        .unwrap();

    let resp = server
        .client
        .get(server.url("/v1/traces/tr-1/export"))
        .bearer_auth(&server.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["trace"].is_object());
}

#[tokio::test]
async fn test_session_traces() {
    let server = spawn_test_server().await;

    server
        .client
        .post(server.url("/v1/traces"))
        .bearer_auth(&server.api_key)
        .json(&make_trace("tr-1", "proj-1"))
        .send()
        .await
        .unwrap();

    let resp = server
        .client
        .get(server.url("/v1/sessions/sess-1/traces"))
        .bearer_auth(&server.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["count"], 1);
}

#[tokio::test]
async fn test_metrics_summary() {
    let server = spawn_test_server().await;

    server
        .client
        .post(server.url("/v1/traces"))
        .bearer_auth(&server.api_key)
        .json(&make_trace("tr-1", "proj-1"))
        .send()
        .await
        .unwrap();

    let resp = server
        .client
        .get(server.url("/v1/metrics/summary?project_id=proj-1"))
        .bearer_auth(&server.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["total_traces"], 1);
}

#[tokio::test]
async fn test_project_crud() {
    let server = spawn_test_server().await;

    // Create
    let resp = server
        .client
        .post(server.url("/v1/projects"))
        .bearer_auth(&server.api_key)
        .json(&serde_json::json!({
            "name": "Test Engine",
            "service_type": "engine-server",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let project_id = body["id"].as_str().unwrap().to_string();

    // List
    let resp = server
        .client
        .get(server.url("/v1/projects"))
        .bearer_auth(&server.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["count"], 1);

    // Get
    let resp = server
        .client
        .get(server.url(&format!("/v1/projects/{}", project_id)))
        .bearer_auth(&server.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Delete
    let resp = server
        .client
        .delete(server.url(&format!("/v1/projects/{}", project_id)))
        .bearer_auth(&server.api_key)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_project_data_isolation() {
    let server = spawn_test_server().await;

    // Ingest traces for different projects
    let mut t1 = make_trace("tr-1", "proj-a");
    t1.session_id = None;
    let mut t2 = make_trace("tr-2", "proj-b");
    t2.session_id = None;

    server
        .client
        .post(server.url("/v1/traces"))
        .bearer_auth(&server.api_key)
        .json(&t1)
        .send()
        .await
        .unwrap();
    server
        .client
        .post(server.url("/v1/traces"))
        .bearer_auth(&server.api_key)
        .json(&t2)
        .send()
        .await
        .unwrap();

    // Query only proj-a
    let resp = server
        .client
        .get(server.url("/v1/traces?project_id=proj-a"))
        .bearer_auth(&server.api_key)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["count"], 1);
    assert_eq!(body["data"][0]["id"], "tr-1");
}
