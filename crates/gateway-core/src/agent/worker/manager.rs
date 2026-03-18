//! Worker Manager - Topological sort, parallel scheduling, timeout control
//!
//! Manages the lifecycle of worker agents in the Orchestrator-Worker pattern.
//! Handles DAG dependency resolution, semaphore-controlled concurrency,
//! and result collection.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, RwLock, Semaphore};
use uuid::Uuid;

use super::types::{
    OrchestratedResult, OrchestratorConfig, WorkerResult, WorkerSpec, WorkerStatus, WorkerUsage,
};
use crate::chat::StreamEvent;
use crate::error::{Error, Result};
use crate::llm::{ChatRequest, ChatResponse, LlmRouter, Message};

/// Manages parallel execution of worker agents
pub struct WorkerManager {
    config: OrchestratorConfig,
    llm_router: Arc<LlmRouter>,
}

impl WorkerManager {
    /// Create a new WorkerManager with the given configuration
    pub fn new(config: OrchestratorConfig, llm_router: Arc<LlmRouter>) -> Self {
        Self { config, llm_router }
    }

    /// Execute a set of worker specifications respecting DAG dependencies
    ///
    /// Workers are scheduled using topological sort, with concurrent execution
    /// limited by the configured semaphore.
    pub async fn execute_workers(
        &self,
        specs: Vec<WorkerSpec>,
        stream_tx: Option<broadcast::Sender<StreamEvent>>,
    ) -> Result<OrchestratedResult> {
        let timeout = self.config.orchestration_timeout;
        match tokio::time::timeout(timeout, self.execute_workers_inner(specs, stream_tx)).await {
            Ok(result) => result,
            Err(_) => Err(Error::Timeout(format!(
                "Orchestration timed out after {}s",
                timeout.as_secs()
            ))),
        }
    }

    /// Inner implementation of execute_workers, wrapped by orchestration_timeout.
    async fn execute_workers_inner(
        &self,
        specs: Vec<WorkerSpec>,
        stream_tx: Option<broadcast::Sender<StreamEvent>>,
    ) -> Result<OrchestratedResult> {
        let start_time = Instant::now();

        if specs.is_empty() {
            return Ok(OrchestratedResult {
                worker_results: vec![],
                all_succeeded: true,
                synthesized_output: None,
                total_duration_ms: 0,
                total_usage: None,
            });
        }

        // Validate and topologically sort the worker DAG
        let execution_order = self.topological_sort(&specs)?;

        // R1-H18: Budget enforcement — track cumulative spend across waves
        let budget_limit = self.config.max_total_budget_usd;
        let accumulated_cost = Arc::new(RwLock::new(0.0f64));

        // Track worker states
        let results: Arc<RwLock<HashMap<Uuid, WorkerResult>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let statuses: Arc<RwLock<HashMap<Uuid, WorkerStatus>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Initialize all workers as pending
        for spec in &specs {
            statuses
                .write()
                .await
                .insert(spec.id, WorkerStatus::Pending);
        }

        // Create a map from id to spec for quick lookup
        let spec_map: HashMap<Uuid, &WorkerSpec> = specs.iter().map(|s| (s.id, s)).collect();

        // Semaphore for concurrency control
        let semaphore = Arc::new(Semaphore::new(self.config.max_concurrent_workers));

        // Process workers in topological order, launching waves of independent workers
        for wave in execution_order {
            // R1-H18: Check budget before launching each wave
            if let Some(limit) = budget_limit {
                let spent = *accumulated_cost.read().await;
                if spent >= limit {
                    tracing::warn!(
                        spent_usd = spent,
                        limit_usd = limit,
                        remaining_workers = wave.len(),
                        "Worker budget exceeded — cancelling remaining waves"
                    );
                    // Mark all workers in this (and subsequent) wave as cancelled
                    for &wid in &wave {
                        if let Some(s) = spec_map.get(&wid) {
                            statuses.write().await.insert(wid, WorkerStatus::Cancelled);
                            results.write().await.insert(
                                wid,
                                WorkerResult {
                                    worker_id: wid,
                                    success: false,
                                    content: String::new(),
                                    error: Some(format!(
                                        "Worker '{}' cancelled: budget exceeded (${:.4} spent, ${:.2} limit)",
                                        s.name, spent, limit
                                    )),
                                    usage: None,
                                    duration_ms: 0,
                                },
                            );
                        }
                    }
                    // Don't process further waves — break out of the loop
                    break;
                }
            }

            let mut handles = Vec::new();

            for worker_id in wave {
                let spec = match spec_map.get(&worker_id) {
                    Some(s) => (*s).clone(),
                    None => continue,
                };

                let sem = semaphore.clone();
                let results_clone = results.clone();
                let statuses_clone = statuses.clone();
                let accumulated_cost_clone = accumulated_cost.clone();
                let llm_router = self.llm_router.clone();
                let default_model = self.config.default_worker_model.clone();
                let default_timeout = self.config.default_worker_timeout;
                let max_retries = self.config.max_worker_retries;
                let stream_tx_clone = stream_tx.clone();

                let handle = tokio::spawn(async move {
                    let _permit = match sem.acquire().await {
                        Ok(permit) => permit,
                        Err(_) => {
                            // Semaphore closed during shutdown — return a failed result gracefully
                            tracing::warn!(worker_id = %spec.id, "Semaphore closed during shutdown, skipping worker");
                            let failed = WorkerResult {
                                worker_id: spec.id,
                                success: false,
                                content: String::new(),
                                error: Some(
                                    "Worker cancelled: semaphore closed during shutdown"
                                        .to_string(),
                                ),
                                usage: None,
                                duration_ms: 0,
                            };
                            results_clone.write().await.insert(spec.id, failed);
                            statuses_clone
                                .write()
                                .await
                                .insert(spec.id, WorkerStatus::Failed);
                            return;
                        }
                    };

                    // Update status to running
                    statuses_clone
                        .write()
                        .await
                        .insert(spec.id, WorkerStatus::Running);

                    // Notify progress
                    if let Some(ref tx) = stream_tx_clone {
                        let _ = tx.send(StreamEvent::Custom {
                            event_type: "worker_progress".to_string(),
                            data: serde_json::json!({
                                "worker_id": spec.id,
                                "worker_name": spec.name,
                                "status": "running",
                            }),
                        });
                    }

                    let worker_start = Instant::now();
                    let timeout = spec.timeout.unwrap_or(default_timeout);
                    let model = spec.model.clone().unwrap_or(default_model);

                    // Execute with retry logic
                    let mut last_error = None;
                    let mut result = None;

                    for attempt in 0..=max_retries {
                        if attempt > 0 {
                            // Brief delay before retry
                            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                        }

                        let execution = Self::execute_single_worker(&llm_router, &spec, &model);

                        match tokio::time::timeout(timeout, execution).await {
                            Ok(Ok(response)) => {
                                let content = response
                                    .choices
                                    .first()
                                    .map(|c| c.message.content.clone())
                                    .unwrap_or_default();
                                let usage = WorkerUsage {
                                    prompt_tokens: response.usage.prompt_tokens.max(0) as u32,
                                    completion_tokens: response.usage.completion_tokens.max(0)
                                        as u32,
                                    total_tokens: response.usage.total_tokens.max(0) as u32,
                                };

                                // R1-H18: Accumulate cost from this worker
                                let worker_cost = WorkerManager::estimate_cost_usd(&model, &usage);
                                *accumulated_cost_clone.write().await += worker_cost;

                                tracing::debug!(
                                    worker = %spec.name,
                                    model = %model,
                                    prompt_tokens = usage.prompt_tokens,
                                    completion_tokens = usage.completion_tokens,
                                    worker_cost_usd = worker_cost,
                                    "Worker completed — cost recorded"
                                );

                                result = Some(WorkerResult {
                                    worker_id: spec.id,
                                    success: true,
                                    content,
                                    error: None,
                                    usage: Some(usage),
                                    duration_ms: worker_start.elapsed().as_millis() as u64,
                                });
                                break;
                            }
                            Ok(Err(e)) => {
                                last_error = Some(e.to_string());
                            }
                            Err(_) => {
                                last_error = Some(format!(
                                    "Worker '{}' timed out after {}s",
                                    spec.name,
                                    timeout.as_secs()
                                ));
                                // On timeout, no point retrying
                                break;
                            }
                        }
                    }

                    let final_result = result.unwrap_or_else(|| WorkerResult {
                        worker_id: spec.id,
                        success: false,
                        content: String::new(),
                        error: last_error,
                        usage: None,
                        duration_ms: worker_start.elapsed().as_millis() as u64,
                    });

                    let status = if final_result.success {
                        WorkerStatus::Completed
                    } else if final_result
                        .error
                        .as_ref()
                        .map_or(false, |e| e.contains("timed out"))
                    {
                        WorkerStatus::TimedOut
                    } else {
                        WorkerStatus::Failed
                    };

                    statuses_clone.write().await.insert(spec.id, status);

                    // Notify completion
                    if let Some(ref tx) = stream_tx_clone {
                        let _ = tx.send(StreamEvent::Custom {
                            event_type: "worker_progress".to_string(),
                            data: serde_json::json!({
                                "worker_id": spec.id,
                                "worker_name": spec.name,
                                "status": format!("{:?}", status).to_lowercase(),
                                "success": final_result.success,
                                "duration_ms": final_result.duration_ms,
                            }),
                        });
                    }

                    results_clone.write().await.insert(spec.id, final_result);
                });

                handles.push(handle);
            }

            // Wait for all workers in this wave to complete
            for handle in handles {
                if let Err(e) = handle.await {
                    if e.is_panic() {
                        tracing::error!("Worker task panicked: {:?}", e);
                    } else {
                        tracing::warn!("Worker task cancelled: {:?}", e);
                    }
                }
            }

            // R1-H18: After each wave completes, log cumulative spend and check budget.
            // Workers within the same wave run concurrently and their cost is already
            // accumulated above, but we do a post-wave check to provide a clear log
            // line and to short-circuit before the next wave starts.
            if let Some(limit) = budget_limit {
                let spent = *accumulated_cost.read().await;
                tracing::info!(
                    spent_usd = format!("{:.4}", spent),
                    limit_usd = format!("{:.2}", limit),
                    "Worker orchestration budget: ${:.4} / ${:.2} spent",
                    spent,
                    limit
                );
            }
        }

        // Collect results in original order
        let results_map = results.read().await;
        let worker_results: Vec<WorkerResult> = specs
            .iter()
            .filter_map(|s| results_map.get(&s.id).cloned())
            .collect();

        let all_succeeded = worker_results.iter().all(|r| r.success);

        // Aggregate usage
        let total_usage = Self::aggregate_usage(&worker_results);

        let total_duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(OrchestratedResult {
            worker_results,
            all_succeeded,
            synthesized_output: None, // Synthesis is handled by the caller
            total_duration_ms,
            total_usage: Some(total_usage),
        })
    }

    /// Execute a single worker agent, returning the full ChatResponse so
    /// callers can extract both content and token usage.
    async fn execute_single_worker(
        llm_router: &LlmRouter,
        spec: &WorkerSpec,
        model: &str,
    ) -> Result<ChatResponse> {
        let request = ChatRequest {
            messages: vec![
                Message::text(
                    "system",
                    format!(
                        "You are a specialized worker agent with type '{}'. \
                         Execute the following task thoroughly and return your results.",
                        spec.agent_type
                    ),
                ),
                Message::text("user", &spec.prompt),
            ],
            model: Some(model.to_string()),
            max_tokens: Some(4096),
            temperature: Some(0.3),
            stream: false,
            ..Default::default()
        };

        Ok(llm_router.route(request).await?)
    }

    /// Synthesize results from all workers into a cohesive output
    pub async fn synthesize_results(
        &self,
        worker_results: &[WorkerResult],
        synthesis_prompt: Option<&str>,
    ) -> Result<String> {
        let results_text: String = worker_results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                if r.success {
                    format!("## Worker {} Result\n{}", i + 1, r.content)
                } else {
                    format!(
                        "## Worker {} FAILED\nError: {}",
                        i + 1,
                        r.error.as_deref().unwrap_or("Unknown error")
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let prompt = synthesis_prompt.unwrap_or(
            "Synthesize the following worker results into a cohesive, well-organized output. \
             Combine information, resolve any conflicts, and present a unified result.",
        );

        let request = ChatRequest {
            messages: vec![
                Message::text("system", prompt),
                Message::text("user", &results_text),
            ],
            model: Some(self.config.lead_model.clone()),
            max_tokens: Some(8192),
            temperature: Some(0.3),
            stream: false,
            ..Default::default()
        };

        let response = self.llm_router.route(request).await?;

        Ok(response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default())
    }

    /// Topologically sort workers into execution waves
    ///
    /// Returns a Vec of waves, where each wave contains worker IDs that can
    /// execute in parallel (all dependencies satisfied).
    fn topological_sort(&self, specs: &[WorkerSpec]) -> Result<Vec<Vec<Uuid>>> {
        let ids: HashSet<Uuid> = specs.iter().map(|s| s.id).collect();

        // Build adjacency and in-degree maps
        let mut in_degree: HashMap<Uuid, usize> = HashMap::new();
        let mut dependents: HashMap<Uuid, Vec<Uuid>> = HashMap::new();

        for spec in specs {
            in_degree.entry(spec.id).or_insert(0);
            for &dep in &spec.depends_on {
                if !ids.contains(&dep) {
                    return Err(Error::InvalidInput(format!(
                        "Worker '{}' depends on unknown worker ID {}",
                        spec.name, dep
                    )));
                }
                *in_degree.entry(spec.id).or_insert(0) += 1;
                dependents.entry(dep).or_default().push(spec.id);
            }
        }

        let mut waves = Vec::new();
        let mut queue: VecDeque<Uuid> = VecDeque::new();
        let mut processed = 0;

        // Start with nodes that have no dependencies
        for (&id, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(id);
            }
        }

        while !queue.is_empty() {
            let wave: Vec<Uuid> = queue.drain(..).collect();
            processed += wave.len();

            let mut next_queue = VecDeque::new();
            for &id in &wave {
                if let Some(deps) = dependents.get(&id) {
                    for &dependent in deps {
                        let deg = in_degree.get_mut(&dependent).unwrap();
                        *deg -= 1;
                        if *deg == 0 {
                            next_queue.push_back(dependent);
                        }
                    }
                }
            }

            waves.push(wave);
            queue = next_queue;
        }

        // Check for cycles
        if processed != specs.len() {
            let cycle_nodes: Vec<String> = specs
                .iter()
                .filter(|s| in_degree.get(&s.id).map_or(false, |&d| d > 0))
                .map(|s| s.name.clone())
                .collect();

            return Err(Error::WorkerDependencyCycle(format!(
                "Dependency cycle detected involving workers: {}",
                cycle_nodes.join(", ")
            )));
        }

        Ok(waves)
    }

    /// Estimate cost in USD for a worker result using simple per-model pricing.
    ///
    /// Uses the same pricing table as `BillingService` but avoids the DB
    /// dependency. Falls back to a conservative Sonnet-class estimate when
    /// the model is unknown.
    fn estimate_cost_usd(model: &str, usage: &WorkerUsage) -> f64 {
        // USD per million tokens (input, output)
        let (input_rate, output_rate) = if model.starts_with("claude-opus") {
            (15.0, 75.0)
        } else if model.starts_with("claude-sonnet") || model.starts_with("claude-3-5-sonnet") {
            (3.0, 15.0)
        } else if model.starts_with("claude-3-5-haiku") || model.starts_with("claude-haiku") {
            (0.80, 4.0)
        } else if model.starts_with("gpt-4o-mini") {
            (0.15, 0.60)
        } else if model.starts_with("gpt-4o") || model.starts_with("gpt-4") {
            (2.50, 10.0)
        } else if model.starts_with("gemini") {
            (0.10, 0.40)
        } else if model.starts_with("qwen") {
            (1.20, 6.0)
        } else {
            // Conservative fallback: Sonnet-class pricing
            (3.0, 15.0)
        };

        let input_tokens = usage.prompt_tokens as f64;
        let output_tokens = usage.completion_tokens as f64;
        (input_tokens * input_rate + output_tokens * output_rate) / 1_000_000.0
    }

    /// Aggregate usage across all worker results
    fn aggregate_usage(results: &[WorkerResult]) -> WorkerUsage {
        let mut total = WorkerUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        };

        for result in results {
            if let Some(ref usage) = result.usage {
                total.prompt_tokens += usage.prompt_tokens;
                total.completion_tokens += usage.completion_tokens;
                total.total_tokens += usage.total_tokens;
            }
        }

        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmConfig;

    fn create_test_manager() -> WorkerManager {
        let llm_router = Arc::new(LlmRouter::new(LlmConfig::default()));
        WorkerManager::new(OrchestratorConfig::default(), llm_router)
    }

    #[test]
    fn test_topological_sort_no_deps() {
        let manager = create_test_manager();

        let specs = vec![
            WorkerSpec::new("A", "Task A"),
            WorkerSpec::new("B", "Task B"),
            WorkerSpec::new("C", "Task C"),
        ];

        let waves = manager.topological_sort(&specs).unwrap();
        // All workers should be in a single wave (no dependencies)
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].len(), 3);
    }

    #[test]
    fn test_topological_sort_linear_chain() {
        let manager = create_test_manager();

        let a = WorkerSpec::new("A", "Task A");
        let b = WorkerSpec::new("B", "Task B").depends_on(a.id);
        let c = WorkerSpec::new("C", "Task C").depends_on(b.id);

        let specs = vec![a, b, c];
        let waves = manager.topological_sort(&specs).unwrap();

        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0].len(), 1); // A
        assert_eq!(waves[1].len(), 1); // B
        assert_eq!(waves[2].len(), 1); // C
    }

    #[test]
    fn test_topological_sort_diamond() {
        let manager = create_test_manager();

        let a = WorkerSpec::new("A", "Task A");
        let b = WorkerSpec::new("B", "Task B").depends_on(a.id);
        let c = WorkerSpec::new("C", "Task C").depends_on(a.id);
        let d = WorkerSpec::new("D", "Task D")
            .depends_on(b.id)
            .depends_on(c.id);

        let specs = vec![a, b, c, d];
        let waves = manager.topological_sort(&specs).unwrap();

        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0].len(), 1); // A
        assert_eq!(waves[1].len(), 2); // B, C (parallel)
        assert_eq!(waves[2].len(), 1); // D
    }

    #[test]
    fn test_topological_sort_cycle_detection() {
        let manager = create_test_manager();

        let a_id = Uuid::new_v4();
        let b_id = Uuid::new_v4();

        let a = WorkerSpec {
            id: a_id,
            name: "A".to_string(),
            prompt: "Task A".to_string(),
            agent_type: "general-purpose".to_string(),
            depends_on: vec![b_id],
            ..WorkerSpec::new("A", "Task A")
        };

        let b = WorkerSpec {
            id: b_id,
            name: "B".to_string(),
            prompt: "Task B".to_string(),
            agent_type: "general-purpose".to_string(),
            depends_on: vec![a_id],
            ..WorkerSpec::new("B", "Task B")
        };

        let specs = vec![a, b];
        let result = manager.topological_sort(&specs);

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::WorkerDependencyCycle(msg) => {
                assert!(msg.contains("cycle"));
            }
            other => panic!("Expected WorkerDependencyCycle, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_execute_empty_workers() {
        let manager = create_test_manager();
        let result = manager.execute_workers(vec![], None).await.unwrap();

        assert!(result.all_succeeded);
        assert!(result.worker_results.is_empty());
        assert_eq!(result.total_duration_ms, 0);
    }

    #[test]
    fn test_estimate_cost_usd_opus() {
        // Opus: $15/M input, $75/M output
        let usage = WorkerUsage {
            prompt_tokens: 1_000_000,
            completion_tokens: 100_000,
            total_tokens: 1_100_000,
        };
        let cost = WorkerManager::estimate_cost_usd("claude-opus-4-6", &usage);
        // 1M * 15 / 1M + 100K * 75 / 1M = 15.0 + 7.5 = 22.5
        assert!((cost - 22.5).abs() < 0.001);
    }

    #[test]
    fn test_estimate_cost_usd_sonnet() {
        // Sonnet: $3/M input, $15/M output
        let usage = WorkerUsage {
            prompt_tokens: 500_000,
            completion_tokens: 50_000,
            total_tokens: 550_000,
        };
        let cost = WorkerManager::estimate_cost_usd("claude-sonnet-4-6", &usage);
        // 500K * 3 / 1M + 50K * 15 / 1M = 1.5 + 0.75 = 2.25
        assert!((cost - 2.25).abs() < 0.001);
    }

    #[test]
    fn test_estimate_cost_usd_unknown_model_uses_sonnet_fallback() {
        let usage = WorkerUsage {
            prompt_tokens: 1_000_000,
            completion_tokens: 0,
            total_tokens: 1_000_000,
        };
        // Unknown model should use Sonnet-class pricing ($3/M input)
        let cost = WorkerManager::estimate_cost_usd("some-unknown-model", &usage);
        assert!((cost - 3.0).abs() < 0.001);
    }

    #[test]
    fn test_aggregate_usage() {
        let results = vec![
            WorkerResult {
                worker_id: Uuid::new_v4(),
                success: true,
                content: "result 1".to_string(),
                error: None,
                usage: Some(WorkerUsage {
                    prompt_tokens: 100,
                    completion_tokens: 50,
                    total_tokens: 150,
                }),
                duration_ms: 1000,
            },
            WorkerResult {
                worker_id: Uuid::new_v4(),
                success: true,
                content: "result 2".to_string(),
                error: None,
                usage: Some(WorkerUsage {
                    prompt_tokens: 200,
                    completion_tokens: 100,
                    total_tokens: 300,
                }),
                duration_ms: 2000,
            },
        ];

        let total = WorkerManager::aggregate_usage(&results);
        assert_eq!(total.prompt_tokens, 300);
        assert_eq!(total.completion_tokens, 150);
        assert_eq!(total.total_tokens, 450);
    }
}
