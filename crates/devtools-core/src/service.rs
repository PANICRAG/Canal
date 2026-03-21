//! DevtoolsService — facade composing storage and event bus.

use std::sync::Arc;

use crate::error::DevtoolsError;
use crate::filter::{MetricsFilter, ObservationUpdate, TraceFilter, TraceUpdate};
use crate::traits::*;
use crate::types::*;

type Result<T> = std::result::Result<T, DevtoolsError>;

/// Main service facade composing TraceStore and EventBus.
///
/// This is the primary API for both ingest (writing data) and query (reading data).
/// Similar pattern to `BillingService` in billing-core.
pub struct DevtoolsService {
    store: Arc<dyn TraceStore>,
    event_bus: Arc<dyn EventBus>,
    exporters: Vec<Arc<dyn crate::traits::TraceExporter>>,
}

impl DevtoolsService {
    /// Create a new DevtoolsService.
    pub fn new(store: Arc<dyn TraceStore>, event_bus: Arc<dyn EventBus>) -> Self {
        Self {
            store,
            event_bus,
            exporters: Vec::new(),
        }
    }

    /// Add exporters for sending data to external systems (e.g., Langfuse).
    ///
    /// Exporters receive data fire-and-forget after the store persists it.
    pub fn with_exporters(mut self, exporters: Vec<Arc<dyn crate::traits::TraceExporter>>) -> Self {
        self.exporters = exporters;
        self
    }

    // ── Ingest API ──────────────────────────────────────────────────────

    /// Ingest a new trace and publish a creation event.
    pub async fn ingest_trace(&self, trace: Trace) -> Result<()> {
        self.store.ingest_trace(trace.clone()).await?;
        self.event_bus
            .publish_trace_event(TraceEvent::TraceCreated {
                trace: trace.clone(),
            })
            .await;
        // Fire-and-forget to exporters
        for exporter in &self.exporters {
            if let Err(e) = exporter.export_trace(&trace).await {
                tracing::warn!("Exporter failed on trace: {}", e);
            }
        }
        Ok(())
    }

    /// Ingest an observation and publish to trace subscribers.
    pub async fn ingest_observation(&self, obs: Observation) -> Result<()> {
        let trace_id = obs.trace_id().to_string();
        self.store.ingest_observation(obs.clone()).await?;
        self.event_bus
            .publish_observation(&trace_id, obs.clone())
            .await;
        self.event_bus
            .publish_trace_event(TraceEvent::ObservationCreated {
                observation: obs.clone(),
            })
            .await;
        // Fire-and-forget to exporters
        for exporter in &self.exporters {
            if let Err(e) = exporter.export_observation(&obs).await {
                tracing::warn!("Exporter failed on observation: {}", e);
            }
        }
        Ok(())
    }

    /// Update an existing trace.
    pub async fn update_trace(&self, id: &str, update: TraceUpdate) -> Result<()> {
        self.store.update_trace(id, update.clone()).await?;
        if let Some(trace) = self.store.get_trace(id).await? {
            self.event_bus
                .publish_trace_event(TraceEvent::TraceUpdated { trace })
                .await;
        }
        // Fire-and-forget to exporters
        for exporter in &self.exporters {
            if let Err(e) = exporter.export_trace_update(id, &update).await {
                tracing::warn!("Exporter failed on trace update: {}", e);
            }
        }
        Ok(())
    }

    /// Update an existing observation.
    pub async fn update_observation(&self, id: &str, update: ObservationUpdate) -> Result<()> {
        self.store.update_observation(id, update.clone()).await?;
        // Fire-and-forget to exporters
        for exporter in &self.exporters {
            if let Err(e) = exporter.export_observation_update(id, &update).await {
                tracing::warn!("Exporter failed on observation update: {}", e);
            }
        }
        Ok(())
    }

    /// Batch ingest multiple traces and observations.
    pub async fn ingest_batch(&self, batch: IngestBatch) -> Result<()> {
        for trace in batch.traces {
            self.ingest_trace(trace).await?;
        }
        for obs in batch.observations {
            self.ingest_observation(obs).await?;
        }
        Ok(())
    }

    // ── Query API ───────────────────────────────────────────────────────

    /// Get a single trace by ID.
    pub async fn get_trace(&self, id: &str) -> Result<Option<Trace>> {
        self.store.get_trace(id).await
    }

    /// List traces matching a filter.
    pub async fn list_traces(&self, filter: TraceFilter) -> Result<Vec<Trace>> {
        self.store.list_traces(filter).await
    }

    /// Get a trace with its full observation tree.
    pub async fn get_trace_tree(&self, trace_id: &str) -> Result<TraceTree> {
        let trace =
            self.store
                .get_trace(trace_id)
                .await?
                .ok_or_else(|| DevtoolsError::TraceNotFound {
                    id: trace_id.into(),
                })?;
        let observations = self.store.get_trace_observations(trace_id).await?;
        Ok(TraceTree {
            trace,
            observations,
        })
    }

    /// Get aggregated metrics.
    pub async fn get_metrics(&self, filter: MetricsFilter) -> Result<MetricsSummary> {
        self.store.get_metrics_summary(filter).await
    }

    // ── Session API ─────────────────────────────────────────────────────

    /// Get a session by ID.
    pub async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.store.get_session(id).await
    }

    /// List sessions.
    pub async fn list_sessions(
        &self,
        project_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Session>> {
        self.store.list_sessions(project_id, limit).await
    }

    /// Get all traces in a session.
    pub async fn get_session_traces(&self, session_id: &str) -> Result<Vec<Trace>> {
        self.store.get_session_traces(session_id).await
    }

    // ── SSE subscriptions ───────────────────────────────────────────────

    /// Subscribe to real-time observations for a specific trace.
    pub async fn subscribe_trace(
        &self,
        trace_id: &str,
    ) -> tokio::sync::mpsc::Receiver<Observation> {
        self.event_bus.subscribe_trace(trace_id).await
    }

    /// Subscribe to global trace events.
    pub async fn subscribe_global(&self) -> tokio::sync::mpsc::Receiver<TraceEvent> {
        self.event_bus.subscribe_global().await
    }

    // ── Export ───────────────────────────────────────────────────────────

    /// Export a complete trace as a JSON value.
    pub async fn export_trace(&self, trace_id: &str) -> Result<serde_json::Value> {
        let tree = self.get_trace_tree(trace_id).await?;
        Ok(serde_json::to_value(&tree).unwrap_or_default())
    }

    // ── Project management ──────────────────────────────────────────────

    /// Create a new project.
    pub async fn create_project(&self, project: Project) -> Result<()> {
        self.store.create_project(project).await
    }

    /// Get a project by ID.
    pub async fn get_project(&self, id: &str) -> Result<Option<Project>> {
        self.store.get_project(id).await
    }

    /// List all projects.
    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        self.store.list_projects().await
    }

    /// Delete a project.
    pub async fn delete_project(&self, id: &str) -> Result<()> {
        self.store.delete_project(id).await
    }

    /// Resolve a project API key to a project ID.
    pub async fn resolve_project_key(&self, api_key: &str) -> Result<Option<String>> {
        self.store.resolve_project_key(api_key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::memory::{InMemoryEventBus, InMemoryTraceStore};
    use chrono::Utc;

    fn test_service() -> DevtoolsService {
        let store = Arc::new(InMemoryTraceStore::new(100));
        let bus = Arc::new(InMemoryEventBus::new());
        DevtoolsService::new(store, bus)
    }

    fn make_trace(id: &str) -> Trace {
        Trace {
            id: id.into(),
            project_id: "proj-1".into(),
            session_id: Some("sess-1".into()),
            name: Some("test".into()),
            user_id: None,
            start_time: Utc::now(),
            end_time: None,
            input: Some(serde_json::json!({"query": "hello"})),
            output: None,
            metadata: serde_json::Map::new(),
            tags: vec![],
            status: TraceStatus::Running,
            total_tokens: 0,
            total_cost_usd: 0.0,
            observation_count: 0,
        }
    }

    #[tokio::test]
    async fn test_service_ingest_and_query() {
        let svc = test_service();

        svc.ingest_trace(make_trace("tr-1")).await.unwrap();
        svc.ingest_trace(make_trace("tr-2")).await.unwrap();

        let trace = svc.get_trace("tr-1").await.unwrap();
        assert!(trace.is_some());

        let traces = svc.list_traces(TraceFilter::default()).await.unwrap();
        assert_eq!(traces.len(), 2);
    }

    #[tokio::test]
    async fn test_service_export_trace() {
        let svc = test_service();
        svc.ingest_trace(make_trace("tr-1")).await.unwrap();

        let gen = Observation::Generation(GenerationData {
            id: "gen-1".into(),
            trace_id: "tr-1".into(),
            parent_id: None,
            name: "llm-call".into(),
            model: "claude-sonnet".into(),
            start_time: Utc::now(),
            end_time: Some(Utc::now()),
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
        svc.ingest_observation(gen).await.unwrap();

        let export = svc.export_trace("tr-1").await.unwrap();
        assert!(export["trace"]["id"] == "tr-1");
        assert_eq!(export["observations"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_service_project_isolation() {
        let svc = test_service();

        let mut t1 = make_trace("tr-1");
        t1.project_id = "proj-1".into();
        let mut t2 = make_trace("tr-2");
        t2.project_id = "proj-2".into();

        svc.ingest_trace(t1).await.unwrap();
        svc.ingest_trace(t2).await.unwrap();

        let proj1 = svc
            .list_traces(TraceFilter {
                project_id: Some("proj-1".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(proj1.len(), 1);
        assert_eq!(proj1[0].id, "tr-1");
    }

    #[tokio::test]
    async fn test_service_update_trace_status() {
        let svc = test_service();
        svc.ingest_trace(make_trace("tr-1")).await.unwrap();

        svc.update_trace(
            "tr-1",
            TraceUpdate {
                status: Some(TraceStatus::Completed),
                end_time: Some(Utc::now()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let trace = svc.get_trace("tr-1").await.unwrap().unwrap();
        assert_eq!(trace.status, TraceStatus::Completed);
    }

    #[tokio::test]
    async fn test_service_trace_tree() {
        let svc = test_service();
        svc.ingest_trace(make_trace("tr-1")).await.unwrap();

        let span = Observation::Span(SpanData {
            id: "span-1".into(),
            trace_id: "tr-1".into(),
            parent_id: None,
            name: "ANALYZE".into(),
            start_time: Utc::now(),
            end_time: None,
            input: None,
            output: None,
            metadata: serde_json::Map::new(),
            status: ObservationStatus::Running,
            level: ObservationLevel::Info,
            service_name: None,
        });
        svc.ingest_observation(span).await.unwrap();

        let tree = svc.get_trace_tree("tr-1").await.unwrap();
        assert_eq!(tree.trace.id, "tr-1");
        assert_eq!(tree.observations.len(), 1);
    }

    #[tokio::test]
    async fn test_service_batch_ingest() {
        let svc = test_service();

        let batch = IngestBatch {
            traces: vec![make_trace("tr-1"), make_trace("tr-2")],
            observations: vec![Observation::Event(EventData {
                id: "evt-1".into(),
                trace_id: "tr-1".into(),
                parent_id: None,
                name: "test".into(),
                time: Utc::now(),
                input: None,
                output: None,
                metadata: serde_json::Map::new(),
                level: ObservationLevel::Info,
                service_name: None,
            })],
        };

        svc.ingest_batch(batch).await.unwrap();
        assert_eq!(
            svc.list_traces(TraceFilter::default()).await.unwrap().len(),
            2
        );
    }

    // ── Exporter forwarding tests ──

    /// Mock exporter that captures all calls for verification.
    struct MockExporter {
        traces: Arc<tokio::sync::Mutex<Vec<Trace>>>,
        observations: Arc<tokio::sync::Mutex<Vec<Observation>>>,
        trace_updates: Arc<tokio::sync::Mutex<Vec<(String, TraceUpdate)>>>,
        obs_updates: Arc<tokio::sync::Mutex<Vec<(String, ObservationUpdate)>>>,
        should_fail: bool,
    }

    impl MockExporter {
        fn new() -> Self {
            Self {
                traces: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                observations: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                trace_updates: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                obs_updates: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                should_fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                should_fail: true,
                ..Self::new()
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::traits::TraceExporter for MockExporter {
        async fn export_trace(
            &self,
            trace: &Trace,
        ) -> std::result::Result<(), crate::error::DevtoolsError> {
            if self.should_fail {
                return Err(crate::error::DevtoolsError::Internal("mock failure".into()));
            }
            self.traces.lock().await.push(trace.clone());
            Ok(())
        }

        async fn export_observation(
            &self,
            obs: &Observation,
        ) -> std::result::Result<(), crate::error::DevtoolsError> {
            if self.should_fail {
                return Err(crate::error::DevtoolsError::Internal("mock failure".into()));
            }
            self.observations.lock().await.push(obs.clone());
            Ok(())
        }

        async fn export_trace_update(
            &self,
            id: &str,
            update: &TraceUpdate,
        ) -> std::result::Result<(), crate::error::DevtoolsError> {
            if self.should_fail {
                return Err(crate::error::DevtoolsError::Internal("mock failure".into()));
            }
            self.trace_updates
                .lock()
                .await
                .push((id.to_string(), update.clone()));
            Ok(())
        }

        async fn export_observation_update(
            &self,
            id: &str,
            update: &ObservationUpdate,
        ) -> std::result::Result<(), crate::error::DevtoolsError> {
            if self.should_fail {
                return Err(crate::error::DevtoolsError::Internal("mock failure".into()));
            }
            self.obs_updates
                .lock()
                .await
                .push((id.to_string(), update.clone()));
            Ok(())
        }

        async fn flush(&self) -> std::result::Result<(), crate::error::DevtoolsError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_service_without_exporters_unchanged() {
        let svc = test_service();
        svc.ingest_trace(make_trace("tr-1")).await.unwrap();
        let trace = svc.get_trace("tr-1").await.unwrap();
        assert!(trace.is_some());
    }

    #[tokio::test]
    async fn test_ingest_trace_forwards_to_exporter() {
        let exporter = Arc::new(MockExporter::new());
        let store = Arc::new(InMemoryTraceStore::new(100));
        let bus = Arc::new(InMemoryEventBus::new());
        let svc = DevtoolsService::new(store, bus).with_exporters(vec![
            exporter.clone() as Arc<dyn crate::traits::TraceExporter>
        ]);

        svc.ingest_trace(make_trace("tr-1")).await.unwrap();

        let exported = exporter.traces.lock().await;
        assert_eq!(exported.len(), 1);
        assert_eq!(exported[0].id, "tr-1");
    }

    #[tokio::test]
    async fn test_ingest_observation_forwards_to_exporter() {
        let exporter = Arc::new(MockExporter::new());
        let store = Arc::new(InMemoryTraceStore::new(100));
        let bus = Arc::new(InMemoryEventBus::new());
        let svc = DevtoolsService::new(store, bus).with_exporters(vec![
            exporter.clone() as Arc<dyn crate::traits::TraceExporter>
        ]);

        svc.ingest_trace(make_trace("tr-1")).await.unwrap();

        let span = Observation::Span(SpanData {
            id: "span-1".into(),
            trace_id: "tr-1".into(),
            parent_id: None,
            name: "test".into(),
            start_time: Utc::now(),
            end_time: None,
            input: None,
            output: None,
            metadata: serde_json::Map::new(),
            status: ObservationStatus::Running,
            level: ObservationLevel::Info,
            service_name: None,
        });
        let gen = Observation::Generation(GenerationData {
            id: "gen-1".into(),
            trace_id: "tr-1".into(),
            parent_id: None,
            name: "llm".into(),
            model: "claude".into(),
            start_time: Utc::now(),
            end_time: None,
            input: None,
            output: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            cost_usd: None,
            metadata: serde_json::Map::new(),
            status: ObservationStatus::Running,
            service_name: None,
        });
        let event = Observation::Event(EventData {
            id: "evt-1".into(),
            trace_id: "tr-1".into(),
            parent_id: None,
            name: "tool".into(),
            time: Utc::now(),
            input: None,
            output: None,
            metadata: serde_json::Map::new(),
            level: ObservationLevel::Info,
            service_name: None,
        });

        svc.ingest_observation(span).await.unwrap();
        svc.ingest_observation(gen).await.unwrap();
        svc.ingest_observation(event).await.unwrap();

        let exported = exporter.observations.lock().await;
        assert_eq!(exported.len(), 3);
    }

    #[tokio::test]
    async fn test_update_trace_forwards_to_exporter() {
        let exporter = Arc::new(MockExporter::new());
        let store = Arc::new(InMemoryTraceStore::new(100));
        let bus = Arc::new(InMemoryEventBus::new());
        let svc = DevtoolsService::new(store, bus).with_exporters(vec![
            exporter.clone() as Arc<dyn crate::traits::TraceExporter>
        ]);

        svc.ingest_trace(make_trace("tr-1")).await.unwrap();
        svc.update_trace(
            "tr-1",
            TraceUpdate {
                status: Some(TraceStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let updates = exporter.trace_updates.lock().await;
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, "tr-1");
    }

    #[tokio::test]
    async fn test_update_observation_forwards_to_exporter() {
        let exporter = Arc::new(MockExporter::new());
        let store = Arc::new(InMemoryTraceStore::new(100));
        let bus = Arc::new(InMemoryEventBus::new());
        let svc = DevtoolsService::new(store, bus).with_exporters(vec![
            exporter.clone() as Arc<dyn crate::traits::TraceExporter>
        ]);

        svc.ingest_trace(make_trace("tr-1")).await.unwrap();
        let span = Observation::Span(SpanData {
            id: "span-1".into(),
            trace_id: "tr-1".into(),
            parent_id: None,
            name: "test".into(),
            start_time: Utc::now(),
            end_time: None,
            input: None,
            output: None,
            metadata: serde_json::Map::new(),
            status: ObservationStatus::Running,
            level: ObservationLevel::Info,
            service_name: None,
        });
        svc.ingest_observation(span).await.unwrap();
        svc.update_observation(
            "span-1",
            ObservationUpdate {
                status: Some(ObservationStatus::Completed),
                end_time: Some(Utc::now()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let updates = exporter.obs_updates.lock().await;
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, "span-1");
    }

    #[tokio::test]
    async fn test_exporter_error_doesnt_block_store() {
        let exporter = Arc::new(MockExporter::failing());
        let store = Arc::new(InMemoryTraceStore::new(100));
        let bus = Arc::new(InMemoryEventBus::new());
        let svc = DevtoolsService::new(store, bus)
            .with_exporters(vec![exporter as Arc<dyn crate::traits::TraceExporter>]);

        // Exporter fails but store should still have the trace
        svc.ingest_trace(make_trace("tr-1")).await.unwrap();
        let trace = svc.get_trace("tr-1").await.unwrap();
        assert!(trace.is_some());
    }

    #[tokio::test]
    async fn test_multiple_exporters_all_receive_data() {
        let exp1 = Arc::new(MockExporter::new());
        let exp2 = Arc::new(MockExporter::new());
        let store = Arc::new(InMemoryTraceStore::new(100));
        let bus = Arc::new(InMemoryEventBus::new());
        let svc = DevtoolsService::new(store, bus).with_exporters(vec![
            exp1.clone() as Arc<dyn crate::traits::TraceExporter>,
            exp2.clone() as Arc<dyn crate::traits::TraceExporter>,
        ]);

        svc.ingest_trace(make_trace("tr-1")).await.unwrap();

        assert_eq!(exp1.traces.lock().await.len(), 1);
        assert_eq!(exp2.traces.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn test_batch_ingest_forwards_all() {
        let exporter = Arc::new(MockExporter::new());
        let store = Arc::new(InMemoryTraceStore::new(100));
        let bus = Arc::new(InMemoryEventBus::new());
        let svc = DevtoolsService::new(store, bus).with_exporters(vec![
            exporter.clone() as Arc<dyn crate::traits::TraceExporter>
        ]);

        let batch = IngestBatch {
            traces: vec![make_trace("tr-1"), make_trace("tr-2")],
            observations: vec![Observation::Event(EventData {
                id: "evt-1".into(),
                trace_id: "tr-1".into(),
                parent_id: None,
                name: "test".into(),
                time: Utc::now(),
                input: None,
                output: None,
                metadata: serde_json::Map::new(),
                level: ObservationLevel::Info,
                service_name: None,
            })],
        };

        svc.ingest_batch(batch).await.unwrap();

        assert_eq!(exporter.traces.lock().await.len(), 2);
        assert_eq!(exporter.observations.lock().await.len(), 1);
    }
}
