//! Five-Layer Browser Automation Architecture
//!
//! This module implements a hybrid automation architecture that dramatically reduces
//! token consumption for browser automation tasks by separating perception, decision,
//! and execution concerns.
//!
//! # Architecture Overview
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    Layer 1: Intent Router                   │
//! │  Analyze task type, data volume → routing decision          │
//! ├─────────────────────────────────────────────────────────────┤
//! │                    Layer 2: CV Exploration                  │
//! │  Screenshot → PageSchema (fixed ~3000-5000 tokens)          │
//! ├─────────────────────────────────────────────────────────────┤
//! │                    Layer 3: Code Generation                 │
//! │  Schema → Playwright/API script (fixed ~500-1000 tokens)    │
//! ├─────────────────────────────────────────────────────────────┤
//! │                    Layer 4: Execution                       │
//! │  Run scripts locally (0 tokens, data never reaches LLM)     │
//! ├─────────────────────────────────────────────────────────────┤
//! │                    Layer 5: Feedback & Assets               │
//! │  Error handling, script iteration, asset accumulation       │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Token Savings
//!
//! | Scenario                    | Pure CV     | Hybrid     | Savings |
//! |-----------------------------|-------------|------------|---------|
//! | Process 1000 rows          | 4,100,000   | ~6,000     | 99.85%  |
//! | Reuse cached script        | 4,100,000   | ~500       | 99.99%  |
//! | Small data (<10 items)     | ~20,000     | ~20,000    | 0%      |
//!
//! # Usage
//!
//! ```rust,ignore
//! use gateway_core::agent::automation::{
//!     BrowserAutomationOrchestrator, AutomationPath, IntentRouter
//! };
//!
//! let orchestrator = BrowserAutomationOrchestrator::builder()
//!     .llm_client(llm)
//!     .browser_router(browser_router)
//!     .hybrid_router(hybrid_router)
//!     .build();
//!
//! // Analyze task and get optimal path
//! let task = "Fill 1000 rows in Google Sheets with customer data";
//! let analysis = orchestrator.analyze(task).await?;
//!
//! match analysis.path {
//!     AutomationPath::ExploreAndGenerate { .. } => {
//!         // Will use CV to explore, generate script, execute
//!         let result = orchestrator.execute(task, data).await?;
//!     }
//!     AutomationPath::ReuseScript { script_id } => {
//!         // Reuse cached script, minimal tokens
//!         let result = orchestrator.execute_cached(&script_id, data).await?;
//!     }
//!     _ => { /* handle other paths */ }
//! }
//! ```

pub mod asset_store;
pub mod code_generator;
pub mod executor;
pub mod explorer;
pub mod intent_router;
pub mod orchestrator;
pub mod types;

// Re-export core types
pub use types::{
    ActionSchema,
    AssetQuery,
    AssetStats,
    // Automation path
    AutomationPath,
    // Execution types
    AutomationRequest,
    AutomationResult,
    Coordinates,
    ElementSchema,
    ElementType,
    ExecutionStats,
    // Script types
    GeneratedScript,
    // Page schema types
    PageSchema,
    PathDecision,
    RouteAnalysis,
    // Asset types
    ScriptAsset,
    ScriptMetadata,
    ScriptType,
};

// Re-export intent router
pub use intent_router::{ApiInfo, IntentAnalysis, IntentRouter, IntentRouterBuilder, TargetSystem};

// Re-export explorer
pub use explorer::{CvExplorer, CvExplorerBuilder, ExplorationOptions, ExplorationResult};

// Re-export code generator
pub use code_generator::{
    CodeGenerator, CodeGeneratorBuilder, GenerationOptions, GenerationResult,
};

// Re-export executor
pub use executor::{ExecutionOptions, ScriptExecutor, ScriptExecutorBuilder};

// Re-export asset store
pub use asset_store::{
    AssetStore, AssetStoreBuilder, AssetStoreConfig, FileAssetStore, MemoryAssetStore,
};

// Re-export orchestrator
pub use orchestrator::{
    BrowserAutomationOrchestrator, BrowserAutomationOrchestratorBuilder, MetricsSnapshot,
    OrchestratorConfig, OrchestratorMetrics, OrchestratorStatus,
};
