//! In-memory storage implementations for devtools.
//!
//! Uses DashMap for concurrent access with LRU eviction.
//! Suitable for dev/demo mode and standalone devtools-server.

use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;
use std::collections::{HashMap, VecDeque};
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::error::DevtoolsError;
use crate::filter::{MetricsFilter, ObservationUpdate, TraceFilter, TraceUpdate};
use crate::traits::*;
use crate::types::*;

type Result<T> = std::result::Result<T, DevtoolsError>;

// ============================================================================
// InMemoryTraceStore
// ============================================================================

/// In-memory trace store with LRU eviction, backed by DashMap.
pub struct InMemoryTraceStore {
    traces: DashMap<String, Trace>,
    observations: DashMap<String, Vec<Observation>>,
    sessions: DashMap<String, Session>,
    projects: DashMap<String, Project>,
    /// API key -> project_id mapping
    api_key_map: DashMap<String, String>,
    /// Insertion order for LRU eviction
    trace_order: Mutex<VecDeque<String>>,
    max_traces: usize,
}

impl InMemoryTraceStore {
    /// Create a new store with the specified max trace capacity.
    pub fn new(max_traces: usize) -> Self {
        Self {
            traces: DashMap::new(),
            observations: DashMap::new(),
            sessions: DashMap::new(),
            projects: DashMap::new(),
            api_key_map: DashMap::new(),
            trace_order: Mutex::new(VecDeque::new()),
            max_traces,
        }
    }

    /// Evict oldest traces if over capacity.
    async fn evict_if_needed(&self) {
        let mut order = self.trace_order.lock().await;
        while order.len() > self.max_traces {
            if let Some(old_id) = order.pop_front() {
                self.traces.remove(&old_id);
                self.observations.remove(&old_id);
            }
        }
    }

    /// Ensure a session exists, creating it if the trace has a session_id.
    fn ensure_session(&self, trace: &Trace) {
        if let Some(session_id) = &trace.session_id {
            self.sessions
                .entry(session_id.clone())
                .or_insert_with(|| Session {
                    id: session_id.clone(),
                    project_id: trace.project_id.clone(),
                    created_at: Utc::now(),
                    metadata: serde_json::Map::new(),
                });
        }
    }
}

#[async_trait]
impl TraceStore for InMemoryTraceStore {
    async fn ingest_trace(&self, trace: Trace) -> Result<()> {
        let id = trace.id.clone();
        self.ensure_session(&trace);
        self.traces.insert(id.clone(), trace);
        {
            let mut order = self.trace_order.lock().await;
            order.push_back(id);
        }
        self.evict_if_needed().await;
        Ok(())
    }

    async fn ingest_observation(&self, obs: Observation) -> Result<()> {
        let trace_id = obs.trace_id().to_string();

        // Update trace aggregation counters
        if let Some(mut trace) = self.traces.get_mut(&trace_id) {
            trace.observation_count += 1;
            if let Observation::Generation(ref gen) = obs {
                trace.total_tokens += gen.total_tokens as i64;
                if let Some(cost) = gen.cost_usd {
                    trace.total_cost_usd += cost;
                }
            }
        }

        // Store the observation
        self.observations.entry(trace_id).or_default().push(obs);

        Ok(())
    }

    async fn update_trace(&self, id: &str, update: TraceUpdate) -> Result<()> {
        let mut trace = self
            .traces
            .get_mut(id)
            .ok_or_else(|| DevtoolsError::TraceNotFound { id: id.into() })?;

        if let Some(status) = update.status {
            trace.status = status;
        }
        if let Some(end_time) = update.end_time {
            trace.end_time = Some(end_time);
        }
        if let Some(output) = update.output {
            trace.output = Some(output);
        }
        if let Some(name) = update.name {
            trace.name = Some(name);
        }
        if let Some(tags) = update.tags {
            trace.tags = tags;
        }
        if let Some(metadata) = update.metadata {
            for (k, v) in metadata {
                trace.metadata.insert(k, v);
            }
        }

        Ok(())
    }

    async fn update_observation(&self, id: &str, update: ObservationUpdate) -> Result<()> {
        // Find the observation across all traces
        for mut entry in self.observations.iter_mut() {
            for obs in entry.value_mut().iter_mut() {
                if obs.id() == id {
                    match obs {
                        Observation::Span(ref mut span) => {
                            if let Some(status) = &update.status {
                                span.status = status.clone();
                            }
                            if let Some(end_time) = update.end_time {
                                span.end_time = Some(end_time);
                            }
                            if let Some(output) = &update.output {
                                span.output = Some(output.clone());
                            }
                            if let Some(metadata) = &update.metadata {
                                for (k, v) in metadata {
                                    span.metadata.insert(k.clone(), v.clone());
                                }
                            }
                        }
                        Observation::Generation(ref mut gen) => {
                            if let Some(status) = &update.status {
                                gen.status = status.clone();
                            }
                            if let Some(end_time) = update.end_time {
                                gen.end_time = Some(end_time);
                            }
                            if let Some(output) = &update.output {
                                gen.output = Some(output.clone());
                            }
                            if let Some(input_tokens) = update.input_tokens {
                                gen.input_tokens = input_tokens;
                            }
                            if let Some(output_tokens) = update.output_tokens {
                                gen.output_tokens = output_tokens;
                            }
                            if let Some(total_tokens) = update.total_tokens {
                                gen.total_tokens = total_tokens;
                            }
                            if let Some(cost_usd) = update.cost_usd {
                                gen.cost_usd = Some(cost_usd);
                            }
                            if let Some(metadata) = &update.metadata {
                                for (k, v) in metadata {
                                    gen.metadata.insert(k.clone(), v.clone());
                                }
                            }
                        }
                        Observation::Event(_) => {
                            // Events are immutable point-in-time records
                        }
                    }
                    return Ok(());
                }
            }
        }

        Err(DevtoolsError::ObservationNotFound { id: id.into() })
    }

    async fn get_trace(&self, id: &str) -> Result<Option<Trace>> {
        Ok(self.traces.get(id).map(|t| t.clone()))
    }

    async fn list_traces(&self, filter: TraceFilter) -> Result<Vec<Trace>> {
        let mut traces: Vec<Trace> = self
            .traces
            .iter()
            .filter(|entry| {
                let t = entry.value();
                if let Some(ref project_id) = filter.project_id {
                    if &t.project_id != project_id {
                        return false;
                    }
                }
                if let Some(ref session_id) = filter.session_id {
                    if t.session_id.as_deref() != Some(session_id.as_str()) {
                        return false;
                    }
                }
                if let Some(ref status) = filter.status {
                    if &t.status != status {
                        return false;
                    }
                }
                if let Some(ref user_id) = filter.user_id {
                    if t.user_id.as_deref() != Some(user_id.as_str()) {
                        return false;
                    }
                }
                if let Some(ref tag) = filter.tag {
                    if !t.tags.contains(tag) {
                        return false;
                    }
                }
                if let Some(ref name) = filter.name {
                    if let Some(ref trace_name) = t.name {
                        if !trace_name.contains(name.as_str()) {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
                if let Some(start_after) = filter.start_after {
                    if t.start_time < start_after {
                        return false;
                    }
                }
                if let Some(start_before) = filter.start_before {
                    if t.start_time > start_before {
                        return false;
                    }
                }
                true
            })
            .map(|entry| entry.value().clone())
            .collect();

        // Sort by start_time descending (newest first)
        traces.sort_by(|a, b| b.start_time.cmp(&a.start_time));

        // Apply pagination
        let total = traces.len();
        let start = filter.offset.min(total);
        let end = (start + filter.limit).min(total);
        Ok(traces[start..end].to_vec())
    }

    async fn get_trace_observations(&self, trace_id: &str) -> Result<Vec<Observation>> {
        Ok(self
            .observations
            .get(trace_id)
            .map(|obs| obs.clone())
            .unwrap_or_default())
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        Ok(self.sessions.get(id).map(|s| s.clone()))
    }

    async fn list_sessions(&self, project_id: Option<&str>, limit: usize) -> Result<Vec<Session>> {
        let mut sessions: Vec<Session> = self
            .sessions
            .iter()
            .filter(|entry| {
                if let Some(pid) = project_id {
                    entry.value().project_id == pid
                } else {
                    true
                }
            })
            .map(|entry| entry.value().clone())
            .collect();
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        sessions.truncate(limit);
        Ok(sessions)
    }

    async fn get_session_traces(&self, session_id: &str) -> Result<Vec<Trace>> {
        let mut traces: Vec<Trace> = self
            .traces
            .iter()
            .filter(|entry| entry.value().session_id.as_deref() == Some(session_id))
            .map(|entry| entry.value().clone())
            .collect();
        traces.sort_by(|a, b| a.start_time.cmp(&b.start_time));
        Ok(traces)
    }

    async fn get_metrics_summary(&self, filter: MetricsFilter) -> Result<MetricsSummary> {
        let mut total_traces = 0usize;
        let mut total_observations = 0usize;
        let mut total_tokens = 0i64;
        let mut total_cost_usd = 0.0f64;
        let mut duration_sum_ms = 0.0f64;
        let mut duration_count = 0usize;
        let mut model_map: HashMap<String, (usize, i64, f64)> = HashMap::new();
        let mut status_counts = TracesByStatus::default();

        for entry in self.traces.iter() {
            let t = entry.value();

            // Apply project filter
            if let Some(ref pid) = filter.project_id {
                if &t.project_id != pid {
                    continue;
                }
            }
            // Apply time range filter
            if let Some(start) = filter.start_time {
                if t.start_time < start {
                    continue;
                }
            }
            if let Some(end) = filter.end_time {
                if t.start_time > end {
                    continue;
                }
            }

            total_traces += 1;
            total_observations += t.observation_count;
            total_tokens += t.total_tokens;
            total_cost_usd += t.total_cost_usd;

            match t.status {
                TraceStatus::Running => status_counts.running += 1,
                TraceStatus::Completed => status_counts.completed += 1,
                TraceStatus::Error => status_counts.error += 1,
            }

            if let (Some(end_time), start_time) = (t.end_time, t.start_time) {
                let duration = (end_time - start_time).num_milliseconds() as f64;
                duration_sum_ms += duration;
                duration_count += 1;
            }
        }

        // Build model usage from observations
        for entry in self.observations.iter() {
            for obs in entry.value() {
                if let Observation::Generation(gen) = obs {
                    // Check project filter
                    if let Some(ref pid) = filter.project_id {
                        if let Some(trace) = self.traces.get(&gen.trace_id) {
                            if &trace.project_id != pid {
                                continue;
                            }
                        }
                    }
                    let entry = model_map.entry(gen.model.clone()).or_default();
                    entry.0 += 1;
                    entry.1 += gen.total_tokens as i64;
                    entry.2 += gen.cost_usd.unwrap_or(0.0);
                }
            }
        }

        let model_usage: Vec<ModelUsage> = model_map
            .into_iter()
            .map(|(model, (count, tokens, cost))| ModelUsage {
                model,
                call_count: count,
                total_tokens: tokens,
                total_cost_usd: cost,
            })
            .collect();

        let avg_duration = if duration_count > 0 {
            duration_sum_ms / duration_count as f64
        } else {
            0.0
        };

        Ok(MetricsSummary {
            total_traces,
            total_observations,
            total_tokens,
            total_cost_usd,
            avg_trace_duration_ms: avg_duration,
            model_usage,
            traces_by_status: status_counts,
        })
    }

    async fn create_project(&self, project: Project) -> Result<()> {
        if self.projects.contains_key(&project.id) {
            return Err(DevtoolsError::ProjectAlreadyExists {
                id: project.id.clone(),
            });
        }
        self.api_key_map
            .insert(project.api_key.clone(), project.id.clone());
        self.projects.insert(project.id.clone(), project);
        Ok(())
    }

    async fn get_project(&self, id: &str) -> Result<Option<Project>> {
        Ok(self.projects.get(id).map(|p| p.clone()))
    }

    async fn list_projects(&self) -> Result<Vec<Project>> {
        let projects: Vec<Project> = self.projects.iter().map(|p| p.value().clone()).collect();
        Ok(projects)
    }

    async fn delete_project(&self, id: &str) -> Result<()> {
        if let Some((_, project)) = self.projects.remove(id) {
            self.api_key_map.remove(&project.api_key);

            // Remove all traces and observations for this project
            let trace_ids: Vec<String> = self
                .traces
                .iter()
                .filter(|e| e.value().project_id == id)
                .map(|e| e.key().clone())
                .collect();

            for tid in &trace_ids {
                self.traces.remove(tid);
                self.observations.remove(tid);
            }

            // R5-M: Also clean trace_order to prevent ghost entries
            let mut order = self.trace_order.lock().await;
            order.retain(|id| !trace_ids.contains(id));
        }
        Ok(())
    }

    async fn resolve_project_key(&self, api_key: &str) -> Result<Option<String>> {
        Ok(self.api_key_map.get(api_key).map(|v| v.clone()))
    }
}

// ============================================================================
// InMemoryEventBus
// ============================================================================

/// In-memory event bus using mpsc channels for real-time subscriptions.
pub struct InMemoryEventBus {
    /// Per-trace subscribers: trace_id -> list of senders
    trace_subs: RwLock<HashMap<String, Vec<mpsc::Sender<Observation>>>>,
    /// Global subscribers for trace-level events
    global_subs: RwLock<Vec<mpsc::Sender<TraceEvent>>>,
}

impl Default for InMemoryEventBus {
    fn default() -> Self {
        Self {
            trace_subs: RwLock::new(HashMap::new()),
            global_subs: RwLock::new(Vec::new()),
        }
    }
}

impl InMemoryEventBus {
    /// Create a new event bus.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl EventBus for InMemoryEventBus {
    async fn subscribe_trace(&self, trace_id: &str) -> mpsc::Receiver<Observation> {
        let (tx, rx) = mpsc::channel(256);
        let mut subs = self.trace_subs.write().await;
        subs.entry(trace_id.to_string()).or_default().push(tx);
        rx
    }

    async fn subscribe_global(&self) -> mpsc::Receiver<TraceEvent> {
        let (tx, rx) = mpsc::channel(256);
        let mut subs = self.global_subs.write().await;
        subs.push(tx);
        rx
    }

    async fn publish_trace_event(&self, event: TraceEvent) {
        let mut subs = self.global_subs.write().await;
        subs.retain(|tx| tx.try_send(event.clone()).is_ok());
    }

    async fn publish_observation(&self, trace_id: &str, obs: Observation) {
        let mut subs = self.trace_subs.write().await;
        if let Some(subscribers) = subs.get_mut(trace_id) {
            subscribers.retain(|tx| tx.try_send(obs.clone()).is_ok());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use std::sync::Arc;

    fn make_trace(id: &str, project_id: &str, session_id: Option<&str>) -> Trace {
        Trace {
            id: id.into(),
            project_id: project_id.into(),
            session_id: session_id.map(|s| s.into()),
            name: Some(format!("trace-{}", id)),
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

    fn make_generation(id: &str, trace_id: &str, parent_id: Option<&str>) -> Observation {
        Observation::Generation(GenerationData {
            id: id.into(),
            trace_id: trace_id.into(),
            parent_id: parent_id.map(|s| s.into()),
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
        })
    }

    fn make_span(id: &str, trace_id: &str) -> Observation {
        Observation::Span(SpanData {
            id: id.into(),
            trace_id: trace_id.into(),
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
        })
    }

    fn make_event(id: &str, trace_id: &str) -> Observation {
        Observation::Event(EventData {
            id: id.into(),
            trace_id: trace_id.into(),
            parent_id: None,
            name: "tool.click".into(),
            time: Utc::now(),
            input: None,
            output: None,
            metadata: serde_json::Map::new(),
            level: ObservationLevel::Debug,
            service_name: None,
        })
    }

    #[tokio::test]
    async fn test_ingest_and_get_trace() {
        let store = InMemoryTraceStore::new(100);
        let trace = make_trace("tr-1", "proj-1", None);
        store.ingest_trace(trace).await.unwrap();

        let found = store.get_trace("tr-1").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "tr-1");
    }

    #[tokio::test]
    async fn test_ingest_observation_updates_trace_metrics() {
        let store = InMemoryTraceStore::new(100);
        let trace = make_trace("tr-1", "proj-1", None);
        store.ingest_trace(trace).await.unwrap();

        let gen = make_generation("gen-1", "tr-1", None);
        store.ingest_observation(gen).await.unwrap();

        let trace = store.get_trace("tr-1").await.unwrap().unwrap();
        assert_eq!(trace.total_tokens, 150);
        assert!((trace.total_cost_usd - 0.002).abs() < 0.0001);
        assert_eq!(trace.observation_count, 1);
    }

    #[tokio::test]
    async fn test_list_traces_with_filter() {
        let store = InMemoryTraceStore::new(100);
        store
            .ingest_trace(make_trace("tr-1", "proj-1", Some("sess-1")))
            .await
            .unwrap();
        store
            .ingest_trace(make_trace("tr-2", "proj-1", Some("sess-1")))
            .await
            .unwrap();
        store
            .ingest_trace(make_trace("tr-3", "proj-2", None))
            .await
            .unwrap();

        // Filter by project
        let result = store
            .list_traces(TraceFilter {
                project_id: Some("proj-1".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(result.len(), 2);

        // Filter by session
        let result = store
            .list_traces(TraceFilter {
                session_id: Some("sess-1".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(result.len(), 2);

        // Filter by status
        let result = store
            .list_traces(TraceFilter {
                status: Some(TraceStatus::Running),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(result.len(), 3);
    }

    #[tokio::test]
    async fn test_lru_eviction() {
        let store = InMemoryTraceStore::new(3);

        for i in 0..5 {
            store
                .ingest_trace(make_trace(&format!("tr-{}", i), "proj-1", None))
                .await
                .unwrap();
        }

        // Only the last 3 traces should remain
        assert_eq!(store.traces.len(), 3);
        assert!(store.get_trace("tr-0").await.unwrap().is_none());
        assert!(store.get_trace("tr-1").await.unwrap().is_none());
        assert!(store.get_trace("tr-2").await.unwrap().is_some());
        assert!(store.get_trace("tr-3").await.unwrap().is_some());
        assert!(store.get_trace("tr-4").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_get_trace_observations_tree() {
        let store = InMemoryTraceStore::new(100);
        store
            .ingest_trace(make_trace("tr-1", "proj-1", None))
            .await
            .unwrap();

        store
            .ingest_observation(make_span("span-1", "tr-1"))
            .await
            .unwrap();
        store
            .ingest_observation(make_generation("gen-1", "tr-1", Some("span-1")))
            .await
            .unwrap();
        store
            .ingest_observation(make_event("evt-1", "tr-1"))
            .await
            .unwrap();

        let obs = store.get_trace_observations("tr-1").await.unwrap();
        assert_eq!(obs.len(), 3);
    }

    #[tokio::test]
    async fn test_session_grouping() {
        let store = InMemoryTraceStore::new(100);
        store
            .ingest_trace(make_trace("tr-1", "proj-1", Some("sess-1")))
            .await
            .unwrap();
        store
            .ingest_trace(make_trace("tr-2", "proj-1", Some("sess-1")))
            .await
            .unwrap();
        store
            .ingest_trace(make_trace("tr-3", "proj-1", Some("sess-2")))
            .await
            .unwrap();

        let session = store.get_session("sess-1").await.unwrap();
        assert!(session.is_some());

        let traces = store.get_session_traces("sess-1").await.unwrap();
        assert_eq!(traces.len(), 2);

        let sessions = store.list_sessions(None, 10).await.unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_metrics_summary_aggregation() {
        let store = InMemoryTraceStore::new(100);

        let mut trace = make_trace("tr-1", "proj-1", None);
        trace.end_time = Some(trace.start_time + Duration::milliseconds(500));
        trace.status = TraceStatus::Completed;
        store.ingest_trace(trace).await.unwrap();

        store
            .ingest_observation(make_generation("gen-1", "tr-1", None))
            .await
            .unwrap();
        store
            .ingest_observation(make_generation("gen-2", "tr-1", None))
            .await
            .unwrap();

        let metrics = store
            .get_metrics_summary(MetricsFilter {
                project_id: Some("proj-1".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(metrics.total_traces, 1);
        assert_eq!(metrics.traces_by_status.completed, 1);
        assert!(metrics.total_tokens > 0);
        assert!(metrics.total_cost_usd > 0.0);
    }

    #[tokio::test]
    async fn test_concurrent_ingestion() {
        let store = Arc::new(InMemoryTraceStore::new(1000));

        let mut handles = Vec::new();
        for i in 0..50 {
            let s = store.clone();
            handles.push(tokio::spawn(async move {
                s.ingest_trace(make_trace(&format!("tr-{}", i), "proj-1", None))
                    .await
                    .unwrap();
                s.ingest_observation(make_generation(
                    &format!("gen-{}", i),
                    &format!("tr-{}", i),
                    None,
                ))
                .await
                .unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(store.traces.len(), 50);
    }

    #[tokio::test]
    async fn test_update_trace_status() {
        let store = InMemoryTraceStore::new(100);
        store
            .ingest_trace(make_trace("tr-1", "proj-1", None))
            .await
            .unwrap();

        store
            .update_trace(
                "tr-1",
                TraceUpdate {
                    status: Some(TraceStatus::Completed),
                    end_time: Some(Utc::now()),
                    output: Some(serde_json::json!({"result": "done"})),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let trace = store.get_trace("tr-1").await.unwrap().unwrap();
        assert_eq!(trace.status, TraceStatus::Completed);
        assert!(trace.end_time.is_some());
        assert!(trace.output.is_some());
    }

    #[tokio::test]
    async fn test_project_crud() {
        let store = InMemoryTraceStore::new(100);

        let project = Project {
            id: "proj-1".into(),
            name: "Test Project".into(),
            service_type: "gateway-api".into(),
            endpoint: Some("http://localhost:4000".into()),
            api_key: "pk_test_xxx".into(),
            created_at: Utc::now(),
            metadata: serde_json::Map::new(),
        };

        store.create_project(project).await.unwrap();

        let found = store.get_project("proj-1").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "Test Project");

        let projects = store.list_projects().await.unwrap();
        assert_eq!(projects.len(), 1);

        // Resolve API key
        let resolved = store.resolve_project_key("pk_test_xxx").await.unwrap();
        assert_eq!(resolved, Some("proj-1".into()));

        // Delete
        store.delete_project("proj-1").await.unwrap();
        assert!(store.get_project("proj-1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_project_data_isolation() {
        let store = InMemoryTraceStore::new(100);

        store
            .ingest_trace(make_trace("tr-1", "proj-1", None))
            .await
            .unwrap();
        store
            .ingest_trace(make_trace("tr-2", "proj-2", None))
            .await
            .unwrap();

        let proj1 = store
            .list_traces(TraceFilter {
                project_id: Some("proj-1".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(proj1.len(), 1);
        assert_eq!(proj1[0].id, "tr-1");

        let proj2 = store
            .list_traces(TraceFilter {
                project_id: Some("proj-2".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(proj2.len(), 1);
        assert_eq!(proj2[0].id, "tr-2");
    }

    // ── EventBus tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_event_bus_subscribe_trace() {
        let bus = InMemoryEventBus::new();
        let mut rx = bus.subscribe_trace("tr-1").await;

        let obs = make_span("span-1", "tr-1");
        bus.publish_observation("tr-1", obs).await;

        let received = rx.try_recv().unwrap();
        assert_eq!(received.id(), "span-1");
    }

    #[tokio::test]
    async fn test_event_bus_subscribe_global() {
        let bus = InMemoryEventBus::new();
        let mut rx = bus.subscribe_global().await;

        let trace = make_trace("tr-1", "proj-1", None);
        bus.publish_trace_event(TraceEvent::TraceCreated {
            trace: trace.clone(),
        })
        .await;

        let received = rx.try_recv().unwrap();
        match received {
            TraceEvent::TraceCreated { trace } => assert_eq!(trace.id, "tr-1"),
            _ => panic!("unexpected event"),
        }
    }

    #[tokio::test]
    async fn test_event_bus_publish_to_subscribers() {
        let bus = InMemoryEventBus::new();
        let mut rx1 = bus.subscribe_trace("tr-1").await;
        let mut rx2 = bus.subscribe_trace("tr-1").await;

        let obs = make_span("span-1", "tr-1");
        bus.publish_observation("tr-1", obs).await;

        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[tokio::test]
    async fn test_event_bus_isolation() {
        let bus = InMemoryEventBus::new();
        let mut rx_tr1 = bus.subscribe_trace("tr-1").await;
        let mut rx_tr2 = bus.subscribe_trace("tr-2").await;

        let obs = make_span("span-1", "tr-1");
        bus.publish_observation("tr-1", obs).await;

        assert!(rx_tr1.try_recv().is_ok());
        assert!(rx_tr2.try_recv().is_err()); // should not receive tr-1's observation
    }
}
