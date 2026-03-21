//! Browser Automation Orchestrator
//!
//! Coordinates all five layers of the automation architecture:
//! 1. Intent Router - Analyzes tasks and routes to optimal path
//! 2. CV Explorer - Explores pages via screenshots, generates PageSchema
//! 3. Code Generator - Generates automation scripts from schema
//! 4. Script Executor - Executes scripts with zero token cost
//! 5. Asset Store - Caches scripts for reuse

use super::{
    asset_store::{AssetStore, AssetStoreError, MemoryAssetStore},
    code_generator::{CodeGenerator, GenerationOptions, GeneratorError},
    executor::{ExecutionOptions, ExecutorError, ScriptExecutor},
    explorer::{CvExplorer, ExplorationOptions, ExplorerError},
    intent_router::{IntentRouter, IntentRouterBuilder},
    types::{
        AutomationPath, AutomationRequest, AutomationResult, ExecutionStats, GeneratedScript,
        ScriptAsset,
    },
};
use crate::agent::hybrid::HybridRouter;
use crate::llm::router::LlmRouter;
use canal_cv::ScreenController;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;

// ============================================================================
// Error Types
// ============================================================================

#[derive(Error, Debug)]
pub enum OrchestratorError {
    #[error("Intent analysis failed: {0}")]
    IntentError(String),

    #[error("Exploration failed: {0}")]
    ExplorationError(#[from] ExplorerError),

    #[error("Code generation failed: {0}")]
    GenerationError(#[from] GeneratorError),

    #[error("Execution failed: {0}")]
    ExecutionError(#[from] ExecutorError),

    #[error("Asset store error: {0}")]
    AssetError(#[from] AssetStoreError),

    #[error("Browser not connected")]
    BrowserNotConnected,

    #[error("LLM not configured")]
    LlmNotConfigured,

    #[error("Configuration error: {0}")]
    ConfigError(String),
}

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for the orchestrator
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// CV threshold for using pure CV vs script generation
    pub cv_threshold: usize,
    /// Maximum script age for reuse (seconds)
    pub max_script_age_secs: u64,
    /// Minimum success rate for script reuse
    pub min_reuse_success_rate: f64,
    /// Whether to save generated scripts
    pub save_scripts: bool,
    /// Default timeout (milliseconds)
    pub default_timeout_ms: u64,
    /// Whether to enable metrics
    pub enable_metrics: bool,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            cv_threshold: 10,
            max_script_age_secs: 86400 * 7, // 7 days
            min_reuse_success_rate: 0.8,
            save_scripts: true,
            default_timeout_ms: 300000, // 5 minutes
            enable_metrics: true,
        }
    }
}

// ============================================================================
// Metrics
// ============================================================================

/// Metrics for the orchestrator
#[derive(Debug, Default)]
pub struct OrchestratorMetrics {
    /// Total requests processed
    pub total_requests: AtomicU64,
    /// Successful requests
    pub successful_requests: AtomicU64,
    /// Failed requests
    pub failed_requests: AtomicU64,
    /// Scripts generated
    pub scripts_generated: AtomicU64,
    /// Scripts reused
    pub scripts_reused: AtomicU64,
    /// Total exploration tokens
    pub exploration_tokens: AtomicU64,
    /// Total generation tokens
    pub generation_tokens: AtomicU64,
    /// Estimated tokens saved
    pub tokens_saved: AtomicU64,
    /// Total items processed
    pub items_processed: AtomicU64,
}

impl OrchestratorMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_request(&self, success: bool) {
        self.total_requests.fetch_add(1, Ordering::SeqCst);
        if success {
            self.successful_requests.fetch_add(1, Ordering::SeqCst);
        } else {
            self.failed_requests.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn record_script_generated(&self) {
        self.scripts_generated.fetch_add(1, Ordering::SeqCst);
    }

    pub fn record_script_reused(&self) {
        self.scripts_reused.fetch_add(1, Ordering::SeqCst);
    }

    pub fn record_tokens(&self, exploration: u64, generation: u64, saved: u64) {
        self.exploration_tokens
            .fetch_add(exploration, Ordering::SeqCst);
        self.generation_tokens
            .fetch_add(generation, Ordering::SeqCst);
        self.tokens_saved.fetch_add(saved, Ordering::SeqCst);
    }

    pub fn record_items(&self, count: u64) {
        self.items_processed.fetch_add(count, Ordering::SeqCst);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            total_requests: self.total_requests.load(Ordering::SeqCst),
            successful_requests: self.successful_requests.load(Ordering::SeqCst),
            failed_requests: self.failed_requests.load(Ordering::SeqCst),
            scripts_generated: self.scripts_generated.load(Ordering::SeqCst),
            scripts_reused: self.scripts_reused.load(Ordering::SeqCst),
            exploration_tokens: self.exploration_tokens.load(Ordering::SeqCst),
            generation_tokens: self.generation_tokens.load(Ordering::SeqCst),
            tokens_saved: self.tokens_saved.load(Ordering::SeqCst),
            items_processed: self.items_processed.load(Ordering::SeqCst),
        }
    }
}

/// Snapshot of metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub scripts_generated: u64,
    pub scripts_reused: u64,
    pub exploration_tokens: u64,
    pub generation_tokens: u64,
    pub tokens_saved: u64,
    pub items_processed: u64,
}

// ============================================================================
// Status
// ============================================================================

/// Orchestrator status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorStatus {
    /// Whether the orchestrator is ready
    pub ready: bool,
    /// Whether browser is connected
    pub browser_connected: bool,
    /// Whether LLM is available
    pub llm_available: bool,
    /// Number of cached scripts
    pub cached_scripts: u64,
    /// Metrics snapshot
    pub metrics: MetricsSnapshot,
}

// ============================================================================
// Orchestrator
// ============================================================================

/// Browser Automation Orchestrator
#[allow(dead_code)]
pub struct BrowserAutomationOrchestrator {
    /// Intent router (Layer 1)
    intent_router: IntentRouter,
    /// CV explorer (Layer 2)
    explorer: Option<CvExplorer>,
    /// Code generator (Layer 3)
    generator: Option<CodeGenerator>,
    /// Script executor (Layer 4)
    executor: ScriptExecutor,
    /// Asset store (Layer 5)
    asset_store: Arc<dyn AssetStore>,
    /// Browser router
    screen_controller: Option<Arc<dyn ScreenController>>,
    /// LLM router
    llm_router: Option<Arc<LlmRouter>>,
    /// Hybrid router for execution
    hybrid_router: Option<Arc<HybridRouter>>,
    /// Configuration
    config: OrchestratorConfig,
    /// Metrics
    metrics: Arc<OrchestratorMetrics>,
}

impl BrowserAutomationOrchestrator {
    /// Create a builder
    pub fn builder() -> BrowserAutomationOrchestratorBuilder {
        BrowserAutomationOrchestratorBuilder::default()
    }

    /// Analyze a task and determine the optimal path
    pub async fn analyze(
        &self,
        task: &str,
        data_count: Option<usize>,
    ) -> Result<super::types::RouteAnalysis, OrchestratorError> {
        Ok(self.intent_router.analyze(task, data_count).await)
    }

    /// Execute an automation request
    pub async fn execute(
        &self,
        request: AutomationRequest,
    ) -> Result<AutomationResult, OrchestratorError> {
        let _start = std::time::Instant::now();
        let data_count = request.data.len();

        // 1. Analyze intent and get optimal path
        let analysis = self.analyze(&request.task, Some(data_count)).await?;
        let path = analysis.decision.path.clone();

        // 2. Execute based on path
        let result = match &path {
            AutomationPath::ReuseScript { script_id, .. } => {
                self.execute_with_cached_script(script_id, request).await
            }
            AutomationPath::DirectApi { api_type, .. } => {
                self.execute_direct_api(api_type, request).await
            }
            AutomationPath::PureComputerVision { max_items, .. } => {
                self.execute_pure_cv(*max_items, request).await
            }
            AutomationPath::ExploreAndGenerate { target_url, .. } => {
                self.execute_explore_and_generate(target_url, request).await
            }
            AutomationPath::HybridApproach { .. } => self.execute_hybrid(request).await,
            AutomationPath::RequiresHumanAssistance { reason } => Ok(AutomationResult::failure(
                &request.id,
                path.clone(),
                format!("Requires human assistance: {}", reason),
            )),
        };

        // 3. Record metrics
        if self.config.enable_metrics {
            self.metrics
                .record_request(result.as_ref().map(|r| r.success).unwrap_or(false));
            self.metrics.record_items(data_count as u64);
        }

        result
    }

    /// Execute with a cached script
    async fn execute_with_cached_script(
        &self,
        script_id: &str,
        request: AutomationRequest,
    ) -> Result<AutomationResult, OrchestratorError> {
        // Get script from asset store
        let asset = self.asset_store.get(script_id).await?.ok_or_else(|| {
            OrchestratorError::AssetError(AssetStoreError::NotFound(script_id.to_string()))
        })?;

        // Reconstruct script
        let script = GeneratedScript::new(
            asset.script_type,
            &asset.code,
            &asset.language,
            &asset.schema_hash,
            &asset.task_signature,
        );

        // Execute
        let options = ExecutionOptions {
            timeout_ms: request.timeout_ms,
            ..Default::default()
        };

        let batch_result = self
            .executor
            .execute(&script, request.data.clone(), options)
            .await?;

        // Record usage
        self.asset_store
            .record_usage(script_id, batch_result.success)
            .await?;

        if self.config.enable_metrics {
            self.metrics.record_script_reused();
            // Estimate tokens saved (vs pure CV)
            let saved = (request.data.len() as u64) * 10000; // ~10K per item for CV
            self.metrics.record_tokens(0, 0, saved);
        }

        let mut stats = batch_result.stats.clone();
        stats.script_reused = true;
        stats.pure_cv_estimated_tokens = (request.data.len() as u64) * 10000;
        stats.calculate_savings();

        let mut result = AutomationResult::success(
            &request.id,
            AutomationPath::ReuseScript {
                script_id: script_id.to_string(),
                last_success_rate: asset.success_rate,
            },
        );
        result.success = batch_result.success;
        result.script_id = Some(script_id.to_string());
        result.stats = stats;

        Ok(result)
    }

    /// Execute with direct API (skip browser automation)
    async fn execute_direct_api(
        &self,
        api_type: &str,
        request: AutomationRequest,
    ) -> Result<AutomationResult, OrchestratorError> {
        // For direct API, we'd generate an API script and execute it
        // This is a simplified implementation
        let path = AutomationPath::DirectApi {
            api_type: api_type.to_string(),
            api_endpoint: None,
        };

        // In a full implementation, would generate API calls here
        let mut result = AutomationResult::failure(
            &request.id,
            path,
            "Direct API execution not yet implemented",
        );
        result.stats.pure_cv_estimated_tokens = (request.data.len() as u64) * 10000;

        Ok(result)
    }

    /// Execute with pure CV (for small data sets)
    async fn execute_pure_cv(
        &self,
        max_items: usize,
        request: AutomationRequest,
    ) -> Result<AutomationResult, OrchestratorError> {
        // For pure CV, we use the computer use tools directly
        // This would integrate with the existing CV tools

        let path = AutomationPath::PureComputerVision {
            max_items,
            estimated_tokens: (request.data.len() as u64) * 15000,
        };

        // In a full implementation, would execute CV operations here
        let mut result = AutomationResult::failure(
            &request.id,
            path,
            "Pure CV execution should use computer_* tools directly",
        );
        result.stats.pure_cv_estimated_tokens = (request.data.len() as u64) * 15000;

        Ok(result)
    }

    /// Execute with explore and generate (main path for large data)
    async fn execute_explore_and_generate(
        &self,
        target_url: &str,
        request: AutomationRequest,
    ) -> Result<AutomationResult, OrchestratorError> {
        let mut total_stats = ExecutionStats::default();
        let data_count = request.data.len();

        // 1. CV Exploration (Layer 2)
        let explorer = self
            .explorer
            .as_ref()
            .ok_or(OrchestratorError::LlmNotConfigured)?;

        let exploration_result = explorer
            .explore(target_url, ExplorationOptions::default())
            .await?;

        total_stats.exploration_tokens = exploration_result.tokens_used;

        // 2. Code Generation (Layer 3)
        let generator = self
            .generator
            .as_ref()
            .ok_or(OrchestratorError::LlmNotConfigured)?;

        let generation_result = generator
            .generate(
                &request.task,
                &exploration_result.schema,
                GenerationOptions::default(),
            )
            .await?;

        total_stats.generation_tokens = generation_result.tokens_used;

        // 3. Script Execution (Layer 4) - 0 tokens
        let options = ExecutionOptions {
            timeout_ms: request.timeout_ms,
            ..Default::default()
        };

        let batch_result = self
            .executor
            .execute(&generation_result.script, request.data.clone(), options)
            .await?;

        total_stats.execution_tokens = 0;
        total_stats.total_tokens = total_stats.exploration_tokens + total_stats.generation_tokens;
        total_stats.items_processed = batch_result.successful_items;
        total_stats.items_failed = batch_result.failed_items;
        total_stats.duration_ms = batch_result.total_duration_ms;
        total_stats.pure_cv_estimated_tokens = (data_count as u64) * 10000;
        total_stats.calculate_savings();

        // 4. Save to Asset Store (Layer 5)
        let script_id = if self.config.save_scripts && batch_result.success {
            let asset = ScriptAsset::from_script(&generation_result.script);
            Some(self.asset_store.save(asset).await?)
        } else {
            None
        };

        // 5. Record metrics
        if self.config.enable_metrics {
            self.metrics.record_script_generated();
            self.metrics.record_tokens(
                total_stats.exploration_tokens,
                total_stats.generation_tokens,
                total_stats
                    .pure_cv_estimated_tokens
                    .saturating_sub(total_stats.total_tokens),
            );
        }

        let path = AutomationPath::ExploreAndGenerate {
            target_url: target_url.to_string(),
            estimated_tokens: total_stats.total_tokens,
        };

        let mut result = if batch_result.success {
            AutomationResult::success(&request.id, path)
        } else {
            AutomationResult::failure(
                &request.id,
                path,
                format!(
                    "{} of {} items failed",
                    batch_result.failed_items, data_count
                ),
            )
        };

        result.stats = total_stats;
        result.script_id = script_id;

        Ok(result)
    }

    /// Execute with hybrid approach
    async fn execute_hybrid(
        &self,
        request: AutomationRequest,
    ) -> Result<AutomationResult, OrchestratorError> {
        // Hybrid combines CV exploration with script execution
        // Falls back to explore_and_generate for now
        let url = request.target_url.clone().unwrap_or_default();
        self.execute_explore_and_generate(&url, request).await
    }

    /// Get orchestrator status
    pub async fn status(&self) -> OrchestratorStatus {
        let browser_connected = self.screen_controller.is_some();
        let llm_available = self.llm_router.is_some();

        let cached_scripts = self
            .asset_store
            .stats()
            .await
            .map(|s| s.total_assets)
            .unwrap_or(0);

        OrchestratorStatus {
            ready: browser_connected && llm_available,
            browser_connected,
            llm_available,
            cached_scripts,
            metrics: self.metrics.snapshot(),
        }
    }

    /// Get metrics
    pub fn metrics(&self) -> &Arc<OrchestratorMetrics> {
        &self.metrics
    }

    /// Get asset store
    pub fn asset_store(&self) -> &Arc<dyn AssetStore> {
        &self.asset_store
    }

    /// List cached scripts
    pub async fn list_scripts(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<super::types::ScriptAsset>, OrchestratorError> {
        self.asset_store
            .list(limit)
            .await
            .map_err(|e| OrchestratorError::AssetError(e))
    }

    /// Delete a cached script
    pub async fn delete_script(&self, script_id: &str) -> Result<(), OrchestratorError> {
        self.asset_store
            .delete(script_id)
            .await
            .map_err(|e| OrchestratorError::AssetError(e))
    }
}

// ============================================================================
// Builder
// ============================================================================

/// Builder for BrowserAutomationOrchestrator
#[derive(Default)]
pub struct BrowserAutomationOrchestratorBuilder {
    screen_controller: Option<Arc<dyn ScreenController>>,
    llm_router: Option<Arc<LlmRouter>>,
    hybrid_router: Option<Arc<HybridRouter>>,
    asset_store: Option<Arc<dyn AssetStore>>,
    config: OrchestratorConfig,
}

impl BrowserAutomationOrchestratorBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set screen controller
    pub fn screen_controller(mut self, controller: Arc<dyn ScreenController>) -> Self {
        self.screen_controller = Some(controller);
        self
    }

    /// Set LLM router
    pub fn llm_router(mut self, router: Arc<LlmRouter>) -> Self {
        self.llm_router = Some(router);
        self
    }

    /// Set hybrid router
    pub fn hybrid_router(mut self, router: Arc<HybridRouter>) -> Self {
        self.hybrid_router = Some(router);
        self
    }

    /// Set asset store
    pub fn asset_store(mut self, store: Arc<dyn AssetStore>) -> Self {
        self.asset_store = Some(store);
        self
    }

    /// Set CV threshold
    pub fn cv_threshold(mut self, threshold: usize) -> Self {
        self.config.cv_threshold = threshold;
        self
    }

    /// Set min reuse success rate
    pub fn min_reuse_success_rate(mut self, rate: f64) -> Self {
        self.config.min_reuse_success_rate = rate;
        self
    }

    /// Enable/disable script saving
    pub fn save_scripts(mut self, save: bool) -> Self {
        self.config.save_scripts = save;
        self
    }

    /// Enable/disable metrics
    pub fn enable_metrics(mut self, enable: bool) -> Self {
        self.config.enable_metrics = enable;
        self
    }

    /// Build the orchestrator
    pub fn build(self) -> BrowserAutomationOrchestrator {
        // Create asset store if not provided
        let asset_store: Arc<dyn AssetStore> = self
            .asset_store
            .unwrap_or_else(|| Arc::new(MemoryAssetStore::new()));

        // Create intent router
        let intent_router = IntentRouterBuilder::new()
            .asset_store(asset_store.clone())
            .cv_threshold(self.config.cv_threshold)
            .min_reuse_success_rate(self.config.min_reuse_success_rate)
            .build();

        // Create explorer if screen controller and LLM are available
        let explorer = if let (Some(controller), Some(llm)) =
            (self.screen_controller.clone(), self.llm_router.clone())
        {
            Some(CvExplorer::new(controller, llm))
        } else {
            None
        };

        // Create generator if LLM is available
        let generator = self
            .llm_router
            .as_ref()
            .map(|llm| CodeGenerator::new(llm.clone()));

        // Create executor
        let mut executor_builder = ScriptExecutor::builder();
        if let Some(hybrid) = self.hybrid_router.clone() {
            executor_builder = executor_builder.hybrid_router(hybrid);
        }
        let executor = executor_builder.build();

        BrowserAutomationOrchestrator {
            intent_router,
            explorer,
            generator,
            executor,
            asset_store,
            screen_controller: self.screen_controller,
            llm_router: self.llm_router,
            hybrid_router: self.hybrid_router,
            config: self.config,
            metrics: Arc::new(OrchestratorMetrics::new()),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = OrchestratorConfig::default();
        assert_eq!(config.cv_threshold, 10);
        assert_eq!(config.min_reuse_success_rate, 0.8);
        assert!(config.save_scripts);
    }

    #[test]
    fn test_metrics() {
        let metrics = OrchestratorMetrics::new();

        metrics.record_request(true);
        metrics.record_request(true);
        metrics.record_request(false);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.total_requests, 3);
        assert_eq!(snapshot.successful_requests, 2);
        assert_eq!(snapshot.failed_requests, 1);
    }

    #[test]
    fn test_metrics_tokens() {
        let metrics = OrchestratorMetrics::new();

        metrics.record_tokens(3000, 1000, 100000);
        metrics.record_script_generated();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.exploration_tokens, 3000);
        assert_eq!(snapshot.generation_tokens, 1000);
        assert_eq!(snapshot.tokens_saved, 100000);
        assert_eq!(snapshot.scripts_generated, 1);
    }

    #[tokio::test]
    async fn test_builder_minimal() {
        let orchestrator = BrowserAutomationOrchestrator::builder().build();

        let status = orchestrator.status().await;
        assert!(!status.browser_connected);
        assert!(!status.llm_available);
        assert!(!status.ready);
    }

    #[tokio::test]
    async fn test_analyze_task() {
        let orchestrator = BrowserAutomationOrchestrator::builder()
            .cv_threshold(10)
            .build();

        // Small data should route to pure CV
        let analysis = orchestrator
            .analyze("Fill form with data", Some(5))
            .await
            .unwrap();
        assert!(matches!(
            analysis.decision.path,
            AutomationPath::PureComputerVision { .. }
        ));

        // Large data should route to explore and generate
        let analysis = orchestrator
            .analyze("Fill 1000 rows in Google Sheets", Some(1000))
            .await
            .unwrap();
        assert!(matches!(
            analysis.decision.path,
            AutomationPath::ExploreAndGenerate { .. }
        ));
    }
}
