//! Application state

// Allow deprecated imports for backward compatibility with legacy UserMemory
#[allow(deprecated)]
use gateway_core::{
    agent::{
        AgentFactory, BrowserAutomationOrchestrator, BrowserAutomationOrchestratorBuilder,
        CodeOrchestrationConfig, CodeOrchestrationRuntime,
        OrchestratorConfig as WorkerOrchestratorConfig, PermissionManager, PermissionMode,
        PlatformToolConfig, ToolRegistry, WorkerManager,
    },
    artifacts::ArtifactStore,
    billing::BillingService,
    chat::{
        ChatEngine, ChatEngineConfig, ConversationRepository, DefaultSessionManager,
        FileSessionStorage, MemorySessionStorage, MessageRepository, SessionManager,
    },
    executor::{CodeExecutor, ExecutorConfig, RouterMode, UnifiedCodeActRouter},
    filesystem::{DirectoryConfig, DirectoryMode, FilesystemConfig, FilesystemService},
    llm::{
        HealthConfig, HealthTracker, InternalCostTracker, LlmRouter, ModelRegistry, ProfileCatalog,
        RoutingEngine,
    },
    mcp::{BuiltinToolExecutor, McpGateway},
    memory::UnifiedMemoryStore,
    session::{CheckpointManager, SessionRepository},
    tool_system::ToolSystem,
    workflow::{WorkflowEngine, WorkflowExecutor},
};

// Unix-only imports (Firecracker VM)
#[cfg(unix)]
use gateway_core::vm::VmManager;

// Orchestration feature imports (graph + collaboration)
#[cfg(feature = "orchestration")]
use gateway_core::collaboration::{TemplateRegistry, WorkflowRegistry};

// Auto mode selector for chat → graph routing (A24)
#[cfg(feature = "collaboration")]
use gateway_core::agent::mode_selector::AutoModeSelector;

// Cache feature imports
#[cfg(feature = "cache")]
use gateway_core::cache::{
    EmbeddingConfig, InMemorySemanticBackend, PlanCache, PlanCacheConfig, SemanticCache,
    SemanticCacheConfig,
};

// Learning feature imports
#[cfg(feature = "learning")]
use gateway_core::learning::{LearningConfig, LearningEngine};

// Jobs feature imports
#[cfg(feature = "jobs")]
use gateway_core::jobs::{JobScheduler, JobStore, JobsConfig};

// Context Engineering feature imports (always available — no feature gate)
use gateway_core::agent::context::ContextResolverFlags;

use gateway_orchestrator::{ContainerOrchestrator, OrchestratorConfig};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use gateway_core::plugins::{PluginCatalog, PluginManager, SubscriptionStore};
use gateway_core::rte::{
    PendingToolExecutions, RateLimiter, RateLimiterConfig, ToolFallbackConfig,
};

use canal_identity::{DashMapKeyStore, IdentityService, KeyStore};

use crate::error::ApiError;
use crate::routes::tasks::{create_task_store, SharedTaskStore};
use crate::websocket::WebSocketManager;

/// Shared application state
#[allow(dead_code)]
#[derive(Clone)]
pub struct AppState {
    /// Database connection pool
    pub db: PgPool,
    /// LLM Router
    pub llm_router: Arc<RwLock<LlmRouter>>,
    /// Profile Catalog for model routing profiles
    pub profile_catalog: Arc<RwLock<ProfileCatalog>>,
    /// Health Tracker for provider circuit breaker status
    pub health_tracker: Arc<HealthTracker>,
    /// Cost Tracker for usage monitoring
    pub cost_tracker: Arc<InternalCostTracker>,
    /// Model Registry for provider capabilities
    pub model_registry: Arc<ModelRegistry>,
    /// Routing Engine for profile-based LLM routing (optional)
    pub routing_engine: Option<Arc<RoutingEngine>>,
    /// MCP Gateway
    pub mcp_gateway: Arc<McpGateway>,
    /// Unified Tool System (single registry + single execution path)
    pub tool_system: Arc<ToolSystem>,
    /// Workflow Engine
    pub workflow_engine: Arc<RwLock<WorkflowEngine>>,
    /// Workflow Executor (enhanced with pause/resume/checkpoint)
    pub workflow_executor: Arc<RwLock<WorkflowExecutor>>,
    /// Chat Engine
    pub chat_engine: Arc<ChatEngine>,
    /// Unified Memory Store (single source of truth for all memory operations)
    pub unified_memory: Arc<UnifiedMemoryStore>,
    /// Artifact Store
    pub artifact_store: Arc<RwLock<ArtifactStore>>,
    /// Code Executor (optional - requires Docker)
    pub code_executor: Option<Arc<CodeExecutor>>,
    /// Filesystem Service (optional - requires configuration)
    pub filesystem_service: Option<Arc<FilesystemService>>,
    /// Session Repository
    pub session_repository: Arc<SessionRepository>,
    /// Checkpoint Manager
    pub checkpoint_manager: Arc<CheckpointManager>,
    /// Session Manager for Agent SDK compatible session persistence
    pub agent_session_manager: Arc<dyn SessionManager>,
    /// Container Orchestrator for Kubernetes-based isolated execution (optional)
    pub container_orchestrator: Option<Arc<ContainerOrchestrator>>,
    /// Background task store for managing shell tasks
    pub task_store: SharedTaskStore,
    /// Permission Manager for tool execution permission checking
    pub permission_manager: Arc<PermissionManager>,
    /// WebSocket connection manager
    pub ws_manager: Arc<WebSocketManager>,
    /// Message repository for chat message persistence
    pub message_repository: Arc<MessageRepository>,
    /// Conversation repository for conversation persistence
    pub conversation_repository: Arc<ConversationRepository>,
    /// Unified code execution router (K8s / Docker / Firecracker backends)
    pub code_router: Option<Arc<UnifiedCodeActRouter>>,
    /// Firecracker VM manager (optional - requires /dev/kvm + firecracker)
    #[cfg(unix)]
    pub vm_manager: Option<Arc<VmManager>>,
    /// Worker manager for Orchestrator-Worker pattern (optional)
    pub worker_manager: Option<Arc<WorkerManager>>,
    /// Code orchestration runtime for programmatic tool calling (optional)
    pub code_orchestration_runtime: Option<Arc<CodeOrchestrationRuntime>>,
    /// Path to persisted settings JSON file
    pub settings_path: PathBuf,
    /// Cached settings loaded from / written to disk
    pub cached_settings: Arc<RwLock<serde_json::Value>>,
    /// Agent Factory for creating and managing agent sessions (singleton)
    pub agent_factory: Arc<AgentFactory>,
    /// Reference agent tool registry for listing available agent tools via API.
    /// This is a snapshot — actual agent sessions create their own registries.
    pub agent_tool_registry: Arc<ToolRegistry>,
    /// Billing service for usage tracking and cost calculation
    pub billing_service: Option<Arc<BillingService>>,
    /// Five-layer automation orchestrator (optional)
    pub automation_orchestrator: Option<Arc<BrowserAutomationOrchestrator>>,
    /// Template registry for workflow graph patterns (optional)
    /// Provides built-in templates: Simple, WithVerification, PlanExecute, Full, Research
    #[cfg(feature = "orchestration")]
    pub template_registry: Arc<TemplateRegistry>,
    /// Workflow registry for combined built-in + custom template management
    #[cfg(feature = "orchestration")]
    pub workflow_registry: Arc<WorkflowRegistry>,
    /// Plan cache for reusing execution plans on repeated task patterns
    #[cfg(feature = "cache")]
    pub plan_cache: Arc<PlanCache>,
    /// Semantic cache for embedding-based response caching
    #[cfg(feature = "cache")]
    pub semantic_cache: Arc<SemanticCache>,
    /// Learning engine for closed-loop experience collection and knowledge distillation
    #[cfg(feature = "learning")]
    pub learning_engine: Arc<LearningEngine>,
    /// Context resolver feature flags for A20 Context Engineering v2 rollout
    pub context_resolver_flags: Arc<ContextResolverFlags>,
    /// Execution store for debug monitoring and SSE streaming
    #[cfg(feature = "graph")]
    pub execution_store: Arc<gateway_core::graph::ExecutionStore>,
    /// DevTools service for LLM observability (traces, observations, metrics)
    #[cfg(feature = "devtools")]
    pub devtools_service: Arc<devtools_core::DevtoolsService>,
    /// Auto mode selector for intelligent chat → graph routing (A24)
    #[cfg(feature = "collaboration")]
    pub auto_mode_selector: Option<Arc<AutoModeSelector>>,
    /// LLM-based task classifier for intelligent chat routing (A24 Phase 3)
    #[cfg(feature = "collaboration")]
    pub task_classifier: Option<Arc<gateway_core::agent::task_classifier::TaskClassifier>>,
    /// Pending plan approvals store for human-in-the-loop PlanExecute mode
    #[cfg(feature = "collaboration")]
    pub pending_plan_approvals: Arc<gateway_core::collaboration::approval::PendingPlanApprovals>,
    /// Pending clarification store for A43 research planner pipeline
    #[cfg(feature = "collaboration")]
    pub pending_clarifications:
        Arc<gateway_core::collaboration::clarification::PendingClarifications>,
    /// Pending PRD approval store for A43 research planner pipeline
    #[cfg(feature = "collaboration")]
    pub pending_prd_approvals: Arc<gateway_core::collaboration::prd_approval::PendingPrdApprovals>,
    /// Plugin manager for plugin store (catalog + subscriptions) (A25)
    pub plugin_manager: Arc<PluginManager>,
    /// Category resolver for connector ~~category placeholders (A26)
    pub category_resolver: Arc<RwLock<gateway_core::connectors::CategoryResolver>>,
    /// Bundle manager for plugin bundles (A26)
    pub bundle_manager: Arc<RwLock<gateway_core::connectors::BundleManager>>,
    /// Runtime registry for per-user bundle activations (A26)
    pub runtime_registry: Arc<RwLock<gateway_core::connectors::RuntimeRegistry>>,
    /// MCP reference tracker for shared server ref-counting across bundles (A27)
    pub mcp_ref_tracker: Arc<gateway_core::connectors::McpRefTracker>,
    /// MCP connection status tracker for per-server lifecycle monitoring (A27)
    pub mcp_connection_tracker: Arc<gateway_core::connectors::McpConnectionTracker>,
    /// RTE pending tool executions store (A28)
    pub rte_pending: Arc<PendingToolExecutions>,
    /// RTE tool fallback configuration (A28)
    pub rte_fallback_config: Arc<ToolFallbackConfig>,
    /// Rate limiter for per-user, per-endpoint, per-tier throttling (A28)
    pub rate_limiter: Arc<RateLimiter>,
    // === Service Trait Objects (A45 dual-mode deployment) ===
    /// LLM service (monolith: LocalLlmService, distributed: RemoteLlmService)
    pub llm_service: Arc<dyn gateway_service_traits::LlmService>,
    /// Tool service (monolith: LocalToolService, distributed: RemoteToolService)
    pub tool_service: Arc<dyn gateway_service_traits::ToolService>,
    /// Memory service (monolith: LocalMemoryService, distributed: RemoteMemoryService)
    pub memory_service: Arc<dyn gateway_service_traits::MemoryService>,

    /// Server startup timestamp for uptime calculation
    pub started_at: std::time::Instant,
    /// Identity service for agent API key management and scope-based access control (CP2)
    pub identity_service: Arc<IdentityService>,
    /// Billing v2 service (PigaToken-based) — balance, events, spending, plan (A37)
    #[cfg(feature = "billing")]
    pub billing_service_v2: Arc<billing_core::BillingService>,
    /// Metering service for LLM/tool cost recording (A37)
    #[cfg(feature = "billing")]
    pub metering_service: Arc<billing_core::MeteringService>,
    /// Budget guard for pre-request cost estimation (A37)
    #[cfg(feature = "billing")]
    pub budget_guard: Arc<billing_core::BudgetGuard>,
    /// Gift card service v2 (PigaToken-based) (A37)
    #[cfg(feature = "billing")]
    pub gift_card_service_v2: Arc<billing_core::GiftCardService>,
    /// Async job store for persistent background job tracking
    #[cfg(feature = "jobs")]
    pub job_store: Arc<JobStore>,
    /// Async job scheduler for background job execution
    #[cfg(feature = "jobs")]
    pub job_scheduler: Arc<JobScheduler>,
    /// Pending HITL input requests for human-in-the-loop job interaction
    #[cfg(feature = "jobs")]
    pub pending_hitl_inputs: Arc<gateway_core::jobs::PendingHITLInputs>,
    /// Pending step executions for A43 step delegation protocol
    #[cfg(feature = "collaboration")]
    pub pending_step_executions: Arc<gateway_core::agent::step_delegate::PendingStepExecutions>,
    /// Audit log store for cloud console (CP16a)
    pub audit_store: Arc<crate::middleware::audit::AuditStore>,
    /// Remote agent client for microservice mode (CANAL_MODE=microservice)
    pub remote_agent_client: Option<Arc<crate::remote_agent::RemoteAgentClient>>,
}

impl AppState {
    /// Create a new application state
    pub async fn new() -> anyhow::Result<Self> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:postgres@127.0.0.1:54322/postgres".to_string()
        });

        tracing::info!("Connecting to database...");
        // Configure connection options for Supabase PgBouncer compatibility
        // Disable statement caching to avoid "prepared statement already exists" errors
        let connect_options: PgConnectOptions = database_url.parse()?;
        let connect_options = connect_options.statement_cache_capacity(0); // Disable prepared statement caching for PgBouncer

        let db = PgPoolOptions::new()
            .max_connections(10)
            .min_connections(1)
            .connect_with(connect_options)
            .await?;
        tracing::info!("Database connection established (PgBouncer compatible mode)");

        // Run database migrations (idempotent — safe to run from multiple services)
        tracing::info!("Running database migrations...");
        sqlx::migrate!("../../migrations")
            .run(&db)
            .await
            .map_err(|e| anyhow::anyhow!("Migration failed: {}", e))?;
        tracing::info!("Database migrations complete");

        // Initialize LLM Router
        let llm_config = gateway_core::llm::LlmConfig::default();
        let mut llm_router = LlmRouter::new(llm_config);

        // Register LLM providers from env vars (shared helper)
        let registered = gateway_core::llm::register_providers_from_env(&mut llm_router);
        tracing::info!(
            "Registered {} LLM providers: {:?}",
            registered.len(),
            registered
        );

        // Initialize Model Routing components
        // Load profile catalog from config file (optional)
        let profile_catalog: Arc<RwLock<ProfileCatalog>> = {
            let config_path = std::env::var("MODEL_PROFILES_PATH")
                .unwrap_or_else(|_| "config/model-profiles.yaml".to_string());

            match ProfileCatalog::from_yaml(&config_path).await {
                Ok(catalog) => {
                    let profile_count = catalog.list().await.len();
                    let template_count = catalog.list_templates().await.len();
                    tracing::info!(
                        path = %config_path,
                        profiles = profile_count,
                        templates = template_count,
                        "Loaded model routing profiles"
                    );
                    Arc::new(RwLock::new(catalog))
                }
                Err(e) => {
                    tracing::warn!(
                        path = %config_path,
                        error = %e,
                        "Failed to load model profiles, using empty catalog"
                    );
                    Arc::new(RwLock::new(ProfileCatalog::empty()))
                }
            }
        };

        // Initialize Health Tracker for circuit breaker
        let health_config = HealthConfig::default();
        let health_tracker = Arc::new(HealthTracker::new(health_config));
        tracing::info!("Health tracker initialized for provider circuit breaking");

        // Initialize Cost Tracker for usage monitoring
        let cost_tracker = Arc::new(InternalCostTracker::with_default_pricing());
        tracing::info!("Cost tracker initialized for usage monitoring");

        // Initialize Model Registry
        let model_registry = Arc::new(ModelRegistry::new());
        tracing::info!("Model registry initialized");

        // Wrap LLM router in Arc<RwLock> first so we can share it with RoutingEngine
        let llm_router = Arc::new(RwLock::new(llm_router));

        // Initialize Routing Engine (combines profiles, health, and strategies)
        let routing_engine: Option<Arc<RoutingEngine>> = {
            // Check if we have profiles loaded
            let has_profiles = {
                let catalog = profile_catalog.read().await;
                !catalog.list().await.is_empty()
            };

            if !has_profiles {
                tracing::info!("No routing profiles loaded, routing engine disabled");
                None
            } else {
                // Create routing engine with LLM router for dynamic AI-based routing
                let classifier_model = std::env::var("ROUTER_CLASSIFIER_MODEL")
                    .unwrap_or_else(|_| "qwen-turbo".to_string());

                let engine = RoutingEngine::new(
                    profile_catalog.clone(),
                    health_tracker.clone(),
                    cost_tracker.clone(),
                    model_registry.clone(),
                )
                .with_llm_router(llm_router.clone(), classifier_model.clone());

                tracing::info!(
                    classifier_model = %classifier_model,
                    "Routing engine initialized with AI-based dynamic routing support"
                );
                Some(Arc::new(engine))
            }
        };

        // Attach routing engine to LLM router if available
        if let Some(ref engine) = routing_engine {
            llm_router.write().await.set_routing_engine(engine.clone());
            tracing::info!("Routing engine attached to LLM router");
        }

        // Initialize MCP Gateway
        let mcp_gateway = Arc::new(McpGateway::new());

        // Initialize Unified Tool System
        let tool_system = Arc::new(ToolSystem::new());
        tracing::info!("Unified Tool System initialized");

        // Initialize Workflow Engine with LLM and MCP services
        let workflow_engine =
            WorkflowEngine::with_services(llm_router.clone(), mcp_gateway.clone());

        // Initialize Unified Memory Store (single source of truth for all memory operations)
        let unified_memory = Arc::new(UnifiedMemoryStore::new());
        tracing::info!("Unified memory store initialized");

        // Initialize Agent Session Manager for chat persistence
        // Use file storage if SESSION_STORAGE_PATH is set, otherwise use memory
        let agent_session_manager: Arc<dyn SessionManager> =
            if let Ok(storage_path) = std::env::var("SESSION_STORAGE_PATH") {
                tracing::info!(path = %storage_path, "Using file-based session storage");
                Arc::new(DefaultSessionManager::new(Arc::new(
                    FileSessionStorage::new(storage_path),
                )))
            } else {
                tracing::info!("Using in-memory session storage");
                Arc::new(DefaultSessionManager::new(Arc::new(
                    MemorySessionStorage::new(),
                )))
            };

        // Initialize Chat Engine
        // Note: ChatEngine needs its own LlmRouter instance since LlmRouter isn't Clone
        let chat_llm_config = gateway_core::llm::LlmConfig::default();
        let mut chat_llm_router = LlmRouter::new(chat_llm_config);

        // Register providers for chat engine
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            use gateway_core::llm::providers::AnthropicProvider;
            chat_llm_router.register_provider("anthropic", Arc::new(AnthropicProvider::new()));
        }
        if std::env::var("GOOGLE_AI_API_KEY").is_ok() {
            use gateway_core::llm::providers::GoogleAIProvider;
            chat_llm_router.register_provider("google", Arc::new(GoogleAIProvider::new()));
        }
        if std::env::var("OPENAI_API_KEY").is_ok() {
            use gateway_core::llm::providers::OpenAIProvider;
            chat_llm_router.register_provider("openai", Arc::new(OpenAIProvider::new()));
        }
        if let Ok(qwen_key) = std::env::var("QWEN_API_KEY") {
            use gateway_core::llm::providers::openai::{OpenAIConfig, OpenAIProvider};
            let qwen_config = OpenAIConfig {
                api_key: qwen_key,
                base_url: std::env::var("QWEN_BASE_URL").unwrap_or_else(|_| {
                    "https://dashscope-intl.aliyuncs.com/compatible-mode".to_string()
                }),
                default_model: std::env::var("QWEN_DEFAULT_MODEL")
                    .unwrap_or_else(|_| "qwen3-max-2026-01-23".to_string()),
                organization: None,
                name: "qwen".to_string(),
            };
            chat_llm_router
                .register_provider("qwen", Arc::new(OpenAIProvider::with_config(qwen_config)));
            chat_llm_router.set_default_provider("qwen");
        }

        // Configure chat engine with session persistence
        let chat_config = ChatEngineConfig {
            enable_session_persistence: true,
            ..Default::default()
        };

        let chat_engine = Arc::new(ChatEngine::with_session_manager(
            Arc::new(chat_llm_router),
            Some(mcp_gateway.clone()),
            chat_config,
            agent_session_manager.clone(),
        ));

        // Initialize Workflow Executor
        let workflow_executor = WorkflowExecutor::default();

        // Initialize Artifact Store
        let artifact_store = ArtifactStore::new();

        // Initialize Code Executor (optional - Docker must be available)
        // Use a timeout to prevent hanging if Docker daemon is unresponsive
        let code_executor = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            CodeExecutor::new(ExecutorConfig::default()),
        )
        .await
        {
            Ok(Ok(executor)) => {
                tracing::info!("Code executor initialized with Docker support");
                Some(Arc::new(executor))
            }
            Ok(Err(e)) => {
                tracing::warn!(
                    error = %e,
                    "Code executor not available - Docker may not be running"
                );
                None
            }
            Err(_) => {
                tracing::warn!("Code executor initialization timed out (5s) - Docker daemon may be unresponsive");
                None
            }
        };

        // Initialize Filesystem Service with allowed directories
        let filesystem_service = {
            // Get allowed directories from environment or use defaults
            let allowed_dirs = std::env::var("ALLOWED_DIRECTORIES")
                .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
                .unwrap_or_else(|_| {
                    // Default: allow user's home directory
                    if let Some(home) = dirs::home_dir() {
                        vec![home.to_string_lossy().to_string()]
                    } else {
                        vec![]
                    }
                });

            let allowed_directories: Vec<DirectoryConfig> = allowed_dirs
                .into_iter()
                .map(|path| DirectoryConfig {
                    path,
                    mode: DirectoryMode::ReadWrite,
                    description: None,
                    docker_mount_path: None,
                })
                .collect();

            let config = FilesystemConfig {
                enabled: true,
                allowed_directories,
                max_read_bytes: 10 * 1024 * 1024, // 10MB
                max_write_bytes: 10 * 1024 * 1024,
                blocked_patterns: vec![
                    ".env".to_string(),
                    "*.key".to_string(),
                    "*.pem".to_string(),
                    "*credentials*".to_string(),
                    ".ssh/*".to_string(),
                ],
                follow_symlinks: true,
                default_encoding: "utf-8".to_string(),
            };

            tracing::info!(
                directories = ?config.allowed_directories.iter().map(|d| &d.path).collect::<Vec<_>>(),
                "Filesystem service initialized with allowed directories"
            );

            Some(Arc::new(FilesystemService::new(config)))
        };

        // Set up builtin tool executor for MCP Gateway and ToolSystem
        {
            let builtin_executor =
                BuiltinToolExecutor::new(filesystem_service.clone(), code_executor.clone());
            // Register with legacy MCP Gateway
            mcp_gateway.set_builtin_executor(builtin_executor).await;
            tracing::info!("Builtin tools (filesystem, executor) registered with MCP Gateway");

            // Register with Unified Tool System
            let ts_builtin_executor =
                BuiltinToolExecutor::new(filesystem_service.clone(), code_executor.clone());
            tool_system
                .register_builtin_backend(ts_builtin_executor)
                .await;
            tracing::info!("Builtin tools (filesystem, executor) registered with Tool System");
        }

        // Initialize Firecracker VM manager (optional, Unix only)
        #[cfg(unix)]
        let vm_manager: Option<Arc<VmManager>> = if std::env::var("FIRECRACKER_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false)
        {
            // Check for /dev/kvm and firecracker binary
            let kvm_available = std::path::Path::new("/dev/kvm").exists();
            let firecracker_path =
                std::env::var("FIRECRACKER_PATH").unwrap_or_else(|_| "firecracker".to_string());
            let firecracker_available = std::process::Command::new("which")
                .arg(&firecracker_path)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

            if kvm_available && firecracker_available {
                let vm_config = gateway_core::vm::VmManagerConfig::default();
                let manager = VmManager::new(vm_config);
                tracing::info!("Firecracker VM manager initialized");
                Some(Arc::new(manager))
            } else {
                tracing::info!(
                    kvm = kvm_available,
                    firecracker = firecracker_available,
                    "Firecracker requirements not met, VM manager disabled"
                );
                None
            }
        } else {
            tracing::info!("Firecracker VM disabled (FIRECRACKER_ENABLED not set)");
            None
        };

        // Initialize Session Repository and Checkpoint Manager
        let session_repository = Arc::new(SessionRepository::new(db.clone()));
        let checkpoint_manager = Arc::new(CheckpointManager::new(db.clone()));

        // Initialize Container Orchestrator (optional - requires Kubernetes)
        let container_orchestrator = if std::env::var("K8S_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false)
        {
            // Build orchestrator config from environment
            let mut config = OrchestratorConfig::default();

            if let Ok(namespace) = std::env::var("WORKER_NAMESPACE") {
                config.worker_namespace = namespace;
            }

            if let Ok(image) = std::env::var("WORKER_IMAGE") {
                config.worker_image = image;
            } else {
                tracing::warn!(
                    "K8S_ENABLED is set but WORKER_IMAGE is not configured, using default image"
                );
            }

            if let Ok(gateway_endpoint) = std::env::var("GATEWAY_ENDPOINT") {
                config.gateway_endpoint = Some(gateway_endpoint);
            }

            match ContainerOrchestrator::new(db.clone(), config.clone()).await {
                Ok(orchestrator) => {
                    let orchestrator = Arc::new(orchestrator);
                    // Spawn maintenance loop in background
                    gateway_orchestrator::orchestrator::spawn_maintenance_loop(
                        orchestrator.clone(),
                        config.cleanup_interval_minutes as u64,
                    );
                    tracing::info!("Container orchestrator initialized with Kubernetes support");
                    Some(orchestrator)
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Container orchestrator not available - Kubernetes may not be configured"
                    );
                    None
                }
            }
        } else {
            tracing::info!("Container orchestration disabled (K8S_ENABLED not set)");
            None
        };

        // Build Unified Code Execution Router
        // Detects available backends and creates the appropriate routing mode
        let code_router: Option<Arc<UnifiedCodeActRouter>> = {
            let execution_mode =
                std::env::var("EXECUTION_MODE").unwrap_or_else(|_| "auto".to_string());

            let has_docker = code_executor.is_some();
            let has_k8s = container_orchestrator.is_some();
            #[cfg(unix)]
            let has_firecracker = vm_manager.is_some();
            #[cfg(not(unix))]
            let has_firecracker = false;

            // Determine router mode
            let router_mode = match execution_mode.as_str() {
                "k8s" => Some(RouterMode::CloudOnly),
                "docker" => Some(RouterMode::LocalOnly),
                "firecracker" => Some(RouterMode::CloudOnly),
                "local" => None, // No router, use LocalComputerTool directly
                "auto" | _ => {
                    if has_k8s && has_docker {
                        Some(RouterMode::PreferLocal)
                    } else if has_k8s {
                        Some(RouterMode::CloudOnly)
                    } else if has_docker {
                        Some(RouterMode::LocalOnly)
                    } else if has_firecracker {
                        Some(RouterMode::CloudOnly)
                    } else {
                        None // No backends, use LocalComputerTool fallback
                    }
                }
            };

            if let Some(mode) = router_mode {
                let mut builder = UnifiedCodeActRouter::builder()
                    .mode(mode)
                    .fallback_enabled(true);

                // Add local (Docker) strategy
                if has_docker {
                    let local_strategy = gateway_core::executor::LocalExecutionStrategy::new(
                        10,    // max concurrent
                        8.0,   // total CPU
                        16384, // total memory MB
                    );
                    builder = builder.local(Arc::new(local_strategy));
                }

                // Add K8s strategy as cloud backend
                if has_k8s {
                    let k8s_strategy = crate::execution::K8sExecutionStrategy::new(
                        container_orchestrator.clone().unwrap(),
                        uuid::Uuid::nil(),
                    );
                    builder = builder.cloud(Arc::new(k8s_strategy));
                }

                // Add Firecracker strategy as cloud backend (if no K8s, Unix only)
                #[cfg(unix)]
                if has_firecracker && !has_k8s {
                    let fc_strategy = gateway_core::executor::FirecrackerExecutionStrategy::new(
                        vm_manager.clone().unwrap(),
                    );
                    builder = builder.cloud(Arc::new(fc_strategy));
                }

                let router = Arc::new(builder.build());
                // Start health monitoring
                router.start_health_monitor().await;

                tracing::info!(
                    mode = %mode,
                    docker = has_docker,
                    k8s = has_k8s,
                    firecracker = has_firecracker,
                    "Unified code execution router initialized"
                );

                Some(router)
            } else {
                tracing::info!("No execution backends available, agent will use local fallback");
                None
            }
        };

        // Initialize background task store
        let task_store = create_task_store();
        tracing::info!("Background task store initialized");

        // Initialize Permission Manager for tool execution permission checking
        let permission_manager = Arc::new(PermissionManager::new());

        // Set allowed directories in permission manager based on filesystem config
        if let Some(ref _fs_service) = filesystem_service {
            if let Some(home) = dirs::home_dir() {
                // Use futures executor to run async code in sync context
                let home_str = home.to_string_lossy().to_string();
                // We'll add directories asynchronously in a spawned task
                let pm = permission_manager.clone();
                tokio::spawn(async move {
                    pm.add_allowed_directory(home_str).await;
                });
            }
        }
        tracing::info!("Permission manager initialized");

        // Initialize WebSocket manager for extension connections
        let ws_manager = Arc::new(WebSocketManager::new());
        tracing::info!("WebSocket manager initialized");

        // Initialize Message and Conversation repositories for chat persistence
        let message_repository = Arc::new(MessageRepository::new(db.clone()));
        let conversation_repository = Arc::new(ConversationRepository::new(db.clone()));
        tracing::info!("Chat persistence repositories initialized");

        // Initialize Worker Manager for Orchestrator-Worker pattern (optional)
        let worker_manager: Option<Arc<WorkerManager>> =
            if std::env::var("ORCHESTRATOR_WORKERS_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false)
            {
                // Create a dedicated LlmRouter for worker agents
                let worker_llm_config = gateway_core::llm::LlmConfig::default();
                let mut worker_llm_router = LlmRouter::new(worker_llm_config);

                if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                    use gateway_core::llm::providers::AnthropicProvider;
                    worker_llm_router
                        .register_provider("anthropic", Arc::new(AnthropicProvider::new()));
                }
                if std::env::var("GOOGLE_AI_API_KEY").is_ok() {
                    use gateway_core::llm::providers::GoogleAIProvider;
                    worker_llm_router
                        .register_provider("google", Arc::new(GoogleAIProvider::new()));
                }
                if std::env::var("OPENAI_API_KEY").is_ok() {
                    use gateway_core::llm::providers::OpenAIProvider;
                    worker_llm_router.register_provider("openai", Arc::new(OpenAIProvider::new()));
                }
                if let Ok(qwen_key) = std::env::var("QWEN_API_KEY") {
                    use gateway_core::llm::providers::openai::{OpenAIConfig, OpenAIProvider};
                    let qwen_config = OpenAIConfig {
                        api_key: qwen_key,
                        base_url: std::env::var("QWEN_BASE_URL").unwrap_or_else(|_| {
                            "https://dashscope-intl.aliyuncs.com/compatible-mode".to_string()
                        }),
                        default_model: std::env::var("QWEN_DEFAULT_MODEL")
                            .unwrap_or_else(|_| "qwen3-max-2026-01-23".to_string()),
                        organization: None,
                        name: "qwen".to_string(),
                    };
                    worker_llm_router.register_provider(
                        "qwen",
                        Arc::new(OpenAIProvider::with_config(qwen_config)),
                    );
                    worker_llm_router.set_default_provider("qwen");
                }

                let config = WorkerOrchestratorConfig::default();
                let manager = WorkerManager::new(config, Arc::new(worker_llm_router));
                tracing::info!("Worker manager initialized for Orchestrator-Worker pattern");
                Some(Arc::new(manager))
            } else {
                tracing::info!("Worker manager disabled (ORCHESTRATOR_WORKERS_ENABLED not set)");
                None
            };

        // Initialize Code Orchestration Runtime for programmatic tool calling (optional)
        let code_orchestration_runtime: Option<Arc<CodeOrchestrationRuntime>> =
            if std::env::var("CODE_ORCHESTRATION_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false)
            {
                // Create a base ToolRegistry for the proxy bridge
                // This registry provides built-in tools (Read, Write, Bash, etc.)
                // and MCP tools to the sandbox code via HTTP proxy
                let base_registry = ToolRegistry::with_mcp_gateway(mcp_gateway.clone());

                let config = CodeOrchestrationConfig::default();
                let mut runtime = CodeOrchestrationRuntime::new(Arc::new(base_registry), config);

                // Attach code executor if available for Docker sandbox execution
                if let Some(executor) = &code_executor {
                    runtime = runtime.with_code_executor(executor.clone());
                }

                tracing::info!(
                    "Code orchestration runtime initialized for programmatic tool calling"
                );
                Some(Arc::new(runtime))
            } else {
                tracing::info!("Code orchestration disabled (CODE_ORCHESTRATION_ENABLED not set)");
                None
            };

        // Initialize settings persistence
        let settings_path = Self::resolve_settings_path();
        let cached_settings = Self::load_settings_from_disk(&settings_path);
        tracing::info!(
            path = %settings_path.display(),
            "Settings persistence initialized"
        );

        // Initialize Plugin Manager (catalog + subscriptions) for plugin store (A25)
        let plugin_manager = {
            let catalog_dirs: Vec<std::path::PathBuf> = vec!["plugins/".into()];
            let subs_path = std::path::PathBuf::from("data/plugin-subscriptions.json");

            // Ensure data directory exists for subscription persistence
            if let Some(parent) = subs_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            let mut catalog = PluginCatalog::new(catalog_dirs);
            let discovered = catalog.discover();
            tracing::info!(plugins_discovered = discovered, "Plugin catalog scanned");

            let subs = SubscriptionStore::new(subs_path);
            Arc::new(PluginManager::new(catalog, subs))
        };

        // Initialize Learning Engine early (before AgentFactory) so we can wire it
        #[cfg(feature = "learning")]
        let learning_engine = {
            let config = LearningConfig::default();
            let engine = LearningEngine::new(config).with_unified_store(unified_memory.clone());
            tracing::info!("Learning engine initialized with unified memory persistence");
            Arc::new(engine)
        };

        // Initialize Execution Store for debug monitoring (A22)
        #[cfg(feature = "graph")]
        let execution_store = Arc::new(gateway_core::graph::ExecutionStore::new(100));
        #[cfg(feature = "graph")]
        tracing::info!("Execution store initialized (max 100 records in LRU)");

        // Initialize DevTools Service for LLM observability (DT1+DT2)
        #[cfg(feature = "devtools")]
        let devtools_service = {
            let store: Arc<dyn devtools_core::TraceStore> =
                Arc::new(devtools_core::store::PgTraceStore::new(db.clone()));
            let bus = Arc::new(devtools_core::store::InMemoryEventBus::new());
            let mut service = devtools_core::DevtoolsService::new(store, bus);

            #[cfg(feature = "langfuse")]
            if let Some(lf_config) = devtools_core::config::LangfuseConfig::from_env() {
                tracing::info!(host = %lf_config.host, "Langfuse export enabled");
                let exporter = Arc::new(devtools_core::store::LangfuseExporter::new(lf_config));
                service = service.with_exporters(vec![exporter]);
            }

            Arc::new(service)
        };
        #[cfg(feature = "devtools")]
        tracing::info!("DevtoolsService initialized for LLM observability");

        // Initialize Auto Mode Selector for chat → graph routing (A24)
        #[cfg(feature = "collaboration")]
        let auto_mode_selector = {
            let auto_route_enabled = std::env::var("AUTO_ROUTING_ENABLED")
                .map(|v| v != "false" && v != "0")
                .unwrap_or(true); // Enabled by default

            if auto_route_enabled {
                let selector = AutoModeSelector::builder().build();
                tracing::info!("Auto mode selector initialized (keyword-based routing)");
                Some(Arc::new(selector))
            } else {
                tracing::info!("Auto mode selector disabled via AUTO_ROUTING_ENABLED=false");
                None
            }
        };

        // Initialize pending plan approvals store (human-in-the-loop for PlanExecute)
        #[cfg(feature = "collaboration")]
        let pending_plan_approvals =
            Arc::new(gateway_core::collaboration::approval::PendingPlanApprovals::new());

        // Initialize A43 pending stores (clarification + PRD approval)
        #[cfg(feature = "collaboration")]
        let pending_clarifications =
            Arc::new(gateway_core::collaboration::clarification::PendingClarifications::new());
        #[cfg(feature = "collaboration")]
        let pending_prd_approvals =
            Arc::new(gateway_core::collaboration::prd_approval::PendingPrdApprovals::new());

        // A40: Create HITL inputs store ONCE and share between factory and AppState.
        // Both the replanner (in factory) and the HTTP handler (in AppState) must
        // operate on the same PendingHITLInputs instance.
        let shared_hitl_inputs = Arc::new(gateway_core::jobs::PendingHITLInputs::new());

        // Initialize Agent Factory (singleton for session management)
        // This factory creates AgentRunner instances with all tools properly wired up
        #[cfg(feature = "collaboration")]
        let mut classifier_llm_router_holder: Option<Arc<LlmRouter>> = None;
        let agent_factory = {
            // Create a dedicated LlmRouter for agents
            let agent_llm_config = gateway_core::llm::LlmConfig::default();
            let mut agent_llm_router = LlmRouter::new(agent_llm_config);

            if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                use gateway_core::llm::providers::AnthropicProvider;
                agent_llm_router.register_provider("anthropic", Arc::new(AnthropicProvider::new()));
            }
            if std::env::var("GOOGLE_AI_API_KEY").is_ok() {
                use gateway_core::llm::providers::GoogleAIProvider;
                agent_llm_router.register_provider("google", Arc::new(GoogleAIProvider::new()));
            }
            if std::env::var("OPENAI_API_KEY").is_ok() {
                use gateway_core::llm::providers::OpenAIProvider;
                agent_llm_router.register_provider("openai", Arc::new(OpenAIProvider::new()));
            }
            if let Ok(qwen_key) = std::env::var("QWEN_API_KEY") {
                use gateway_core::llm::providers::openai::{OpenAIConfig, OpenAIProvider};
                let qwen_config = OpenAIConfig {
                    api_key: qwen_key,
                    base_url: std::env::var("QWEN_BASE_URL").unwrap_or_else(|_| {
                        "https://dashscope-intl.aliyuncs.com/compatible-mode".to_string()
                    }),
                    default_model: std::env::var("QWEN_DEFAULT_MODEL")
                        .unwrap_or_else(|_| "qwen3-max-2026-01-23".to_string()),
                    organization: None,
                    name: "qwen".to_string(),
                };
                agent_llm_router
                    .register_provider("qwen", Arc::new(OpenAIProvider::with_config(qwen_config)));
                agent_llm_router.set_default_provider("qwen");
            }

            // Attach routing engine if available
            if let Some(ref engine) = routing_engine {
                agent_llm_router.set_routing_engine(engine.clone());
            }

            let agent_llm_router = Arc::new(agent_llm_router);

            // Clone router for task classifier before factory takes ownership
            #[cfg(feature = "collaboration")]
            {
                classifier_llm_router_holder = Some(agent_llm_router.clone());
            }

            // Permission mode: explicit PERMISSION_MODE env var takes precedence,
            // then falls back to CANAL_ENV-based detection.
            // Values: "default" (ask for all), "accept_edits", "plan", "bypass"
            let permission_mode = if let Ok(mode_str) = std::env::var("PERMISSION_MODE") {
                match mode_str.to_lowercase().as_str() {
                    "default" | "ask" => {
                        tracing::info!("Permission mode from env: Default (ask for all tools)");
                        PermissionMode::Default
                    }
                    "accept_edits" | "acceptedits" => {
                        tracing::info!("Permission mode from env: AcceptEdits");
                        PermissionMode::AcceptEdits
                    }
                    "plan" => {
                        tracing::info!("Permission mode from env: Plan (deny modifications)");
                        PermissionMode::Plan
                    }
                    "bypass" | "none" => {
                        tracing::info!("Permission mode from env: BypassPermissions");
                        PermissionMode::BypassPermissions
                    }
                    _ => {
                        tracing::warn!(
                            mode = %mode_str,
                            "Unknown PERMISSION_MODE value, defaulting to Default"
                        );
                        PermissionMode::Default
                    }
                }
            } else {
                // Auto-approve all tools by default — both dev and production.
                // The permission system is not yet wired to the frontend SSE
                // flow reliably (client disconnect causes tool denial), so
                // BypassPermissions prevents the agent from getting stuck.
                tracing::info!("BypassPermissions enabled (default for all environments)");
                PermissionMode::BypassPermissions
            };

            let mut factory = AgentFactory::new(agent_llm_router)
                .with_mcp_gateway(mcp_gateway.clone())
                .with_tool_system(tool_system.clone())
                .with_max_turns(100)
                .with_permission_mode(permission_mode);

            // Wire up execution backends
            if let Some(ref router) = code_router {
                factory = factory.with_code_router(router.clone());
            }
            #[cfg(unix)]
            if let Some(ref vm) = vm_manager {
                factory = factory.with_vm_manager(vm.clone());
            }
            if let Some(ref wm) = worker_manager {
                factory = factory.with_worker_manager(wm.clone());
            }
            if let Some(ref cor) = code_orchestration_runtime {
                factory = factory.with_code_orchestration(cor.clone());
            }
            // Wire constraint profile from ProfileRegistry if available
            #[cfg(feature = "prompt-constraints")]
            {
                let constraints_path = std::env::var("CONSTRAINTS_CONFIG_PATH")
                    .unwrap_or_else(|_| "config/constraints".to_string());
                let registry = gateway_core::prompt::ProfileRegistry::load_from_directory(
                    std::path::Path::new(&constraints_path),
                )
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to load constraint profiles: {}, using defaults", e);
                    gateway_core::prompt::ProfileRegistry::new()
                });

                // Load user's active profile preference
                let overrides =
                    gateway_core::prompt::UserPromptOverrides::load().unwrap_or_default();
                if let Some(ref profile_name) = overrides.active_profile {
                    if let Some(profile) = registry.get(profile_name) {
                        factory = factory.with_constraint_profile(Some(profile.clone()));
                        tracing::info!(profile = %profile_name, "Loaded constraint profile for agent factory");
                    } else {
                        tracing::warn!(profile = %profile_name, "Active constraint profile not found in registry");
                    }
                } else {
                    tracing::debug!("No active constraint profile set in user overrides");
                }
            }

            // Wire learning engine for knowledge injection
            #[cfg(feature = "learning")]
            {
                factory = factory.with_learning_engine(learning_engine.clone());
                tracing::info!("Learning engine wired to agent factory for knowledge injection");
            }

            // Wire unified memory store for preferences/patterns in prompts
            factory = factory.with_unified_memory(unified_memory.clone());

            // Wire execution store for debug monitoring
            #[cfg(feature = "graph")]
            {
                factory = factory.with_execution_store(execution_store.clone());
                tracing::info!("Execution store wired to agent factory for debug monitoring");
            }

            // Wire DevTools service for LLM observability
            #[cfg(feature = "devtools")]
            {
                factory = factory.with_devtools_service(devtools_service.clone());
                tracing::info!("DevtoolsService wired to agent factory");
            }

            // Load planner config for PlanExecute graph (A24)
            #[cfg(feature = "collaboration")]
            {
                // Use default config (TODO: load from config/planner-prompts.yaml when serde_yaml is added)
                let planner_config = gateway_core::collaboration::PlannerConfig::default();
                tracing::info!(
                    planner_model = %planner_config.planner_model,
                    executor_model = %planner_config.executor_model,
                    "Using default planner config for PlanExecute graph"
                );
                factory = factory.with_planner_config(planner_config);
            }

            // Wire pending plan approvals store for human-in-the-loop PlanExecute
            #[cfg(feature = "collaboration")]
            {
                factory = factory.with_pending_plan_approvals(pending_plan_approvals.clone());
                factory = factory.with_pending_clarifications(pending_clarifications.clone());
                factory = factory.with_pending_prd_approvals(pending_prd_approvals.clone());
                tracing::info!(
                    "Pending plan/clarification/PRD approvals stores wired to agent factory"
                );
            }

            // A40: Wire reflection store for step judge memory
            #[cfg(feature = "collaboration")]
            {
                let reflection_store =
                    Arc::new(gateway_core::learning::reflection::ReflectionStore::new());
                factory = factory.with_reflection_store(reflection_store);
                tracing::info!("ReflectionStore wired to agent factory for judge memory");
            }

            // Wire JudgeConfig with configurable vision model for visual UI verification
            #[cfg(feature = "collaboration")]
            {
                let judge_config = gateway_core::collaboration::judge::JudgeConfig {
                    vision_model: std::env::var("JUDGE_VISION_MODEL")
                        .unwrap_or_else(|_| "claude-sonnet".to_string()),
                    model: std::env::var("JUDGE_MODEL")
                        .unwrap_or_else(|_| "qwen-turbo".to_string()),
                    ..Default::default()
                };
                tracing::info!(
                    model = %judge_config.model,
                    vision_model = %judge_config.vision_model,
                    "JudgeConfig wired to agent factory"
                );
                factory = factory.with_judge_config(judge_config);
            }

            // A40: Wire pending HITL inputs for replanner human guidance
            // IMPORTANT: Reuse the shared_hitl_inputs created earlier so that
            // the factory's replanner and the HTTP endpoint (AppState) use the
            // SAME PendingHITLInputs instance.  Without this, the user's HITL
            // response submitted via POST /api/jobs/:id/input goes to AppState's
            // store while the replanner awaits on a different store.
            #[cfg(feature = "collaboration")]
            {
                factory = factory.with_pending_hitl_inputs(shared_hitl_inputs.clone());
                tracing::info!("PendingHITLInputs wired to agent factory for replan HITL");
            }

            // Wire platform control plane tools for chat-based instance management
            let platform_base_url = std::env::var("PLATFORM_API_URL")
                .unwrap_or_else(|_| "http://localhost:4000".to_string());
            let platform_auth = std::env::var("PLATFORM_AUTH_TOKEN")
                .unwrap_or_else(|_| std::env::var("API_KEY").expect("API_KEY environment variable must be set").to_string());
            let platform_tool_config =
                Arc::new(PlatformToolConfig::new(platform_base_url, platform_auth));
            factory = factory.with_platform_tool_config(platform_tool_config);
            tracing::info!(
                "Platform tools wired to agent factory for chat-based instance management"
            );

            // Wire hosting tools for chat-based app deployment
            let hosting_base_url = std::env::var("HOSTING_API_URL")
                .unwrap_or_else(|_| "http://localhost:8080".to_string());

            // Prefer RS256 service token (works when both services share JWT_PRIVATE_KEY_PEM).
            // Falls back to static API_KEY for local dev where services have different ephemeral keys.
            let hosting_tool_config = if std::env::var("JWT_PRIVATE_KEY_PEM").is_ok() {
                // Production/Docker: shared RSA key → generate RS256 service tokens on-the-fly
                let key_pair = canal_auth::load_key_pair();
                let provider: gateway_core::agent::tools::TokenProvider = Arc::new(move || {
                    let claims = canal_auth::build_service_claims(
                        "gateway-api",
                        vec!["hosting:*".to_string(), "instances:*".to_string()],
                    );
                    canal_auth::issue_service_token(&key_pair, &claims).unwrap_or_else(|e| {
                        tracing::error!("Failed to issue hosting service token: {}", e);
                        String::new()
                    })
                });
                Arc::new(
                    gateway_core::agent::tools::HostingToolConfig::with_token_provider(
                        hosting_base_url,
                        provider,
                    ),
                )
            } else {
                // Dev mode: use API_KEY (both services read from same env/dotenv)
                let hosting_auth = std::env::var("HOSTING_AUTH_TOKEN")
                    .or_else(|_| std::env::var("API_KEY"))
                    .unwrap_or_else(|_| {
                        tracing::warn!(
                            "No JWT_PRIVATE_KEY_PEM, HOSTING_AUTH_TOKEN, or API_KEY set. \
                             Hosting tools will fail auth. Set API_KEY in .env for local dev."
                        );
                        String::new()
                    });
                Arc::new(gateway_core::agent::tools::HostingToolConfig::new(
                    hosting_base_url,
                    hosting_auth,
                ))
            };
            factory = factory.with_hosting_tool_config(hosting_tool_config);
            tracing::info!("Hosting tools wired to agent factory for chat-based app deployment");

            // Wire devtools observation tools for monitoring
            let devtools_base_url = std::env::var("DEVTOOLS_API_URL")
                .unwrap_or_else(|_| "http://localhost:4200".to_string());
            let devtools_api_key = std::env::var("DEVTOOLS_API_KEY")
                .unwrap_or_else(|_| std::env::var("API_KEY").expect("API_KEY environment variable must be set").to_string());
            let devtools_tool_config =
                Arc::new(gateway_core::agent::tools::DevtoolsToolConfig::new(
                    devtools_base_url,
                    devtools_api_key,
                ));
            factory = factory.with_devtools_tool_config(devtools_tool_config);
            tracing::info!("DevTools observation tools wired to agent factory for monitoring");

            // Wire plugin manager for plugin skill injection in system prompts
            factory = factory.with_plugin_manager(plugin_manager.clone());
            tracing::info!("Plugin manager wired to agent factory for skill injection");

            // A46: Apply Role Constraint System (overrides permission mode + namespaces)
            let role_name =
                std::env::var("CANAL_ROLE").unwrap_or_else(|_| "platform_operator".to_string());
            let roles_path = std::env::var("CANAL_ROLES_PATH")
                .unwrap_or_else(|_| "config/roles".to_string());
            let role_registry = gateway_core::roles::RoleRegistry::load_from_directory(
                std::path::Path::new(&roles_path),
            )
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to load role registry: {}, using empty", e);
                gateway_core::roles::RoleRegistry::empty()
            });

            if role_name != "default" {
                if let Some(role) = role_registry.get(&role_name) {
                    factory = factory.with_role(&role);
                    tracing::info!(
                        role = %role_name,
                        namespaces = ?role.tools.enabled_namespaces,
                        discovery = role.tools.discovery_enabled,
                        "Role constraint applied to agent factory"
                    );
                } else {
                    tracing::warn!(
                        role = %role_name,
                        available = ?role_registry.list(),
                        "Requested role not found, using default behavior"
                    );
                }
            } else {
                // Default role: enable tool discovery to save ~65% tokens per turn.
                // Core tools are always visible; hosting/platform/devtools discovered on demand.
                let discovery_enabled = std::env::var("TOOL_DISCOVERY_ENABLED")
                    .map(|v| v != "false" && v != "0")
                    .unwrap_or(true);
                if discovery_enabled {
                    factory = factory.with_tool_discovery(vec![
                        "Read".to_string(),
                        "Write".to_string(),
                        "Edit".to_string(),
                        "Bash".to_string(),
                        "Glob".to_string(),
                        "Grep".to_string(),
                        "Computer".to_string(),
                        "ClaudeCode".to_string(),
                    ]);
                    tracing::info!(
                        "Tool discovery enabled (default role) — 8 core tools + search_tools"
                    );
                } else {
                    tracing::info!("Using default role (no constraints, discovery disabled)");
                }
            }

            tracing::info!("Agent factory initialized (singleton for session management)");
            Arc::new(factory)
        };

        // Create a reference tool registry for listing agent tools via the API
        let agent_tool_registry = agent_factory.create_reference_registry();
        tracing::info!(
            tool_count = agent_tool_registry.list_agent_tools().len(),
            "Agent tool registry snapshot created for API visibility"
        );

        // Initialize LLM Task Classifier for intelligent chat routing (A24 Phase 3)
        #[cfg(feature = "collaboration")]
        let task_classifier = {
            let classifier_enabled = std::env::var("LLM_CLASSIFIER_ENABLED")
                .map(|v| v != "false" && v != "0")
                .unwrap_or(true); // Enabled by default

            if classifier_enabled {
                if let Some(classifier_llm_router) = classifier_llm_router_holder.take() {
                    use gateway_core::agent::task_classifier::{ClassifierConfig, TaskClassifier};
                    let config = ClassifierConfig {
                        model: std::env::var("LLM_CLASSIFIER_MODEL")
                            .unwrap_or_else(|_| "qwen-turbo".into()),
                        ..ClassifierConfig::default()
                    };
                    let classifier = TaskClassifier::new(classifier_llm_router, config);
                    tracing::info!("LLM task classifier initialized for intelligent chat routing");
                    Some(Arc::new(classifier))
                } else {
                    tracing::warn!("classifier_llm_router not set by agent factory init, disabling LLM classifier");
                    None
                }
            } else {
                tracing::info!("LLM task classifier disabled via LLM_CLASSIFIER_ENABLED=false");
                None
            }
        };

        // Initialize Billing Service for per-user usage tracking
        let billing_service = Some(Arc::new(BillingService::new(db.clone())));
        tracing::info!("Billing service initialized for per-user usage tracking");

        // Initialize Five-Layer Automation Orchestrator (optional)
        let automation_orchestrator: Option<Arc<BrowserAutomationOrchestrator>> = if std::env::var(
            "AUTOMATION_ORCHESTRATOR_ENABLED",
        )
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
        {
            let auto_llm_config = gateway_core::llm::LlmConfig::default();
            let mut auto_llm_router = LlmRouter::new(auto_llm_config);

            if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                use gateway_core::llm::providers::AnthropicProvider;
                auto_llm_router.register_provider("anthropic", Arc::new(AnthropicProvider::new()));
            }
            if std::env::var("GOOGLE_AI_API_KEY").is_ok() {
                use gateway_core::llm::providers::GoogleAIProvider;
                auto_llm_router.register_provider("google", Arc::new(GoogleAIProvider::new()));
            }
            if std::env::var("OPENAI_API_KEY").is_ok() {
                use gateway_core::llm::providers::OpenAIProvider;
                auto_llm_router.register_provider("openai", Arc::new(OpenAIProvider::new()));
            }
            if let Ok(qwen_key) = std::env::var("QWEN_API_KEY") {
                use gateway_core::llm::providers::openai::{OpenAIConfig, OpenAIProvider};
                let qwen_config = OpenAIConfig {
                    api_key: qwen_key,
                    base_url: std::env::var("QWEN_BASE_URL").unwrap_or_else(|_| {
                        "https://dashscope-intl.aliyuncs.com/compatible-mode".to_string()
                    }),
                    default_model: std::env::var("QWEN_DEFAULT_MODEL")
                        .unwrap_or_else(|_| "qwen3-max-2026-01-23".to_string()),
                    organization: None,
                    name: "qwen".to_string(),
                };
                auto_llm_router
                    .register_provider("qwen", Arc::new(OpenAIProvider::with_config(qwen_config)));
                auto_llm_router.set_default_provider("qwen");
            }

            let builder =
                BrowserAutomationOrchestratorBuilder::new().llm_router(Arc::new(auto_llm_router));
            let orchestrator = Arc::new(builder.build());

            mcp_gateway
                .set_builtin_automation(orchestrator.clone())
                .await;
            tool_system
                .set_builtin_automation(orchestrator.clone())
                .await;

            tracing::info!(
                "Five-layer automation orchestrator initialized and registered with MCP Gateway + Tool System"
            );
            Some(orchestrator)
        } else {
            tracing::info!(
                "Automation orchestrator disabled (AUTOMATION_ORCHESTRATOR_ENABLED not set)"
            );
            None
        };

        // Initialize Template Registry for workflow graph patterns
        // Provides built-in templates: Simple, WithVerification, PlanExecute, Full, Research
        #[cfg(feature = "orchestration")]
        let template_registry = {
            let registry = TemplateRegistry::with_builtins();
            tracing::info!(
                template_count = registry.count(),
                "Template registry initialized with built-in workflow patterns"
            );
            Arc::new(registry)
        };

        // Initialize Workflow Registry (built-in + custom template management)
        #[cfg(feature = "orchestration")]
        let workflow_registry = {
            let registry = WorkflowRegistry::new();
            let count = registry.list_all().await.len();
            tracing::info!(
                template_count = count,
                "Workflow registry initialized (built-in + custom templates)"
            );
            Arc::new(registry)
        };

        // Initialize Plan Cache (feature-gated)
        #[cfg(feature = "cache")]
        let plan_cache = {
            let config = PlanCacheConfig::default();
            let cache = PlanCache::new(config);
            tracing::info!("Plan cache initialized (L3, in-memory LRU)");
            Arc::new(cache)
        };

        // Initialize Semantic Cache (feature-gated)
        #[cfg(feature = "cache")]
        let semantic_cache = {
            use gateway_core::cache::{EmbeddingProvider, MockEmbeddingProvider};

            let cache_config = SemanticCacheConfig::default();

            // Use RemoteEmbeddingProvider if OPENAI_API_KEY is set, else MockEmbeddingProvider
            let embedding_provider: Arc<dyn EmbeddingProvider> =
                if std::env::var("OPENAI_API_KEY").is_ok() {
                    use gateway_core::cache::RemoteEmbeddingProvider;
                    let embed_config = EmbeddingConfig {
                        api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
                        ..Default::default()
                    };
                    Arc::new(RemoteEmbeddingProvider::new(embed_config))
                } else {
                    tracing::info!("No OPENAI_API_KEY for embeddings, using mock provider");
                    Arc::new(MockEmbeddingProvider::new(1536))
                };

            let backend = Arc::new(InMemorySemanticBackend::new());
            let cache = SemanticCache::new(cache_config, embedding_provider, backend);
            tracing::info!("Semantic cache initialized (L2, in-memory backend)");
            Arc::new(cache)
        };

        // Note: Learning engine initialized earlier (before agent_factory) for wiring

        // Initialize Context Resolver Flags (always initialized, no feature gate)
        let context_resolver_flags = {
            let config_path = std::env::var("CONTEXT_ENGINEERING_CONFIG")
                .unwrap_or_else(|_| "config/context-engineering.yaml".to_string());

            let flags = match ContextResolverFlags::from_yaml(std::path::Path::new(&config_path)) {
                Ok(flags) => {
                    tracing::info!(
                        scoring_rollout = flags.scoring_rollout_pct,
                        knowledge_injection = flags.knowledge_injection,
                        prompt_inspection = flags.prompt_inspection,
                        "Context engineering flags loaded from {}",
                        config_path
                    );
                    flags
                }
                Err(e) => {
                    tracing::warn!(
                        path = %config_path,
                        error = %e,
                        "Failed to load context engineering config, using defaults"
                    );
                    ContextResolverFlags::default()
                }
            };
            Arc::new(flags)
        };

        // Initialize Identity Service for API key management (CP2)
        let identity_service = {
            let system_key =
                std::env::var("API_KEY").unwrap_or_else(|_| std::env::var("API_KEY").expect("API_KEY environment variable must be set").to_string());
            let key_hash = canal_identity::key_gen::hash_key(&system_key);
            let key_prefix = if system_key.len() > 12 {
                format!("{}...", &system_key[..12])
            } else {
                system_key.clone()
            };
            let store: Arc<dyn KeyStore> =
                Arc::new(DashMapKeyStore::with_system_key(&key_hash, &key_prefix));
            let service = IdentityService::new(store);
            tracing::info!("Identity service initialized (system key loaded)");
            Arc::new(service)
        };

        // Seed admin user if not exists
        if let Err(e) = seed_admin_user(&db).await {
            tracing::warn!(error = %e, "Failed to seed admin user (may already exist)");
        }

        // Initialize billing v2 services with shared stores (A37)
        #[cfg(feature = "billing")]
        let billing_v2_services = {
            let pricing = Arc::new(
                billing_core::PricingEngine::from_yaml("config/pricing.yaml").unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "Failed to load pricing.yaml, using defaults");
                    billing_core::PricingEngine::with_defaults()
                }),
            );
            let plans = Arc::new(
                billing_core::PlanRegistry::from_yaml("config/billing-plans.yaml")
                    .unwrap_or_else(|e| {
                        tracing::warn!(error = %e, "Failed to load billing-plans.yaml, using defaults");
                        billing_core::PlanRegistry::with_defaults()
                    }),
            );

            let balance_store: Arc<dyn billing_core::BalanceStore> =
                Arc::new(billing_core::store::PgBalanceStore::new(db.clone()));
            let event_store: Arc<dyn billing_core::EventStore> =
                Arc::new(billing_core::store::PgEventStore::new(db.clone()));
            let gift_store: Arc<dyn billing_core::GiftCardStore> =
                Arc::new(billing_core::store::PgGiftCardStore::new(db.clone()));

            let billing = Arc::new(billing_core::BillingService::new(
                balance_store,
                event_store,
                pricing.clone(),
                plans.clone(),
            ));
            let metering = Arc::new(billing_core::MeteringService::new(
                pricing.clone(),
                billing.clone(),
            ));
            let guard = Arc::new(billing_core::BudgetGuard::new(
                pricing,
                billing.clone(),
                plans,
            ));
            let gifts = Arc::new(billing_core::GiftCardService::new(
                gift_store,
                billing.clone(),
            ));

            tracing::info!("Billing v2 services initialized (PigaToken-based)");
            (billing, metering, guard, gifts)
        };

        // Initialize async job system
        #[cfg(feature = "jobs")]
        let job_store = Arc::new(JobStore::new(db.clone()));
        #[cfg(feature = "jobs")]
        let job_scheduler = {
            let mut config = JobsConfig::default();

            // Override from gateway.yaml jobs section if available
            if let Ok(yaml_str) = std::fs::read_to_string("config/gateway.yaml") {
                if let Ok(yaml_val) = serde_yaml::from_str::<serde_yaml::Value>(&yaml_str) {
                    if let Some(jobs) = yaml_val.get("jobs") {
                        if let Some(enabled) = jobs.get("enabled").and_then(|v| v.as_bool()) {
                            config.enabled = enabled;
                        }
                        if let Some(max_c) = jobs.get("max_concurrent").and_then(|v| v.as_u64()) {
                            config.max_concurrent = max_c as usize;
                        }
                        if let Some(poll) = jobs.get("poll_interval_ms").and_then(|v| v.as_u64()) {
                            config.poll_interval_ms = poll;
                        }
                        if let Some(timeout) = jobs.get("job_timeout_secs").and_then(|v| v.as_u64())
                        {
                            config.job_timeout_secs = timeout;
                        }
                        if let Some(mode) = jobs.get("default_mode").and_then(|v| v.as_str()) {
                            config.default_mode = mode.to_string();
                        }
                        if let Some(model) = jobs.get("default_model").and_then(|v| v.as_str()) {
                            config.default_model = Some(model.to_string());
                        }
                        tracing::info!(
                            default_model = ?config.default_model,
                            default_mode = %config.default_mode,
                            "Loaded job config from gateway.yaml"
                        );
                    }
                }
            }
            let scheduler = JobScheduler::new(
                job_store.clone(),
                agent_factory.clone(),
                #[cfg(feature = "graph")]
                execution_store.clone(),
                config.clone(),
            );
            #[cfg(feature = "devtools")]
            let scheduler = scheduler.with_devtools_service(devtools_service.clone());
            let scheduler = Arc::new(scheduler);
            if config.enabled {
                scheduler.start();
                tracing::info!(
                    max_concurrent = config.max_concurrent,
                    "Job scheduler started"
                );
                if config.recovery.enabled {
                    let recovery_scheduler = scheduler.clone();
                    tokio::spawn(async move {
                        match recovery_scheduler.recover().await {
                            Ok(count) if count > 0 => {
                                tracing::info!(count, "Recovered interrupted jobs");
                            }
                            Ok(_) => {}
                            Err(e) => {
                                tracing::warn!(error = %e, "Job recovery failed");
                            }
                        }
                    });
                }
            }
            scheduler
        };

        // Service trait objects (A45 dual-mode deployment)
        // In monolith mode (default), these wrap concrete types in-process.
        // In distributed mode, these would be replaced with gRPC-backed Remote*Service.
        use gateway_core::services::{LocalLlmService, LocalMemoryService, LocalToolService};
        let llm_service: Arc<dyn gateway_service_traits::LlmService> =
            Arc::new(LocalLlmService::new(llm_router.clone()));
        let tool_service: Arc<dyn gateway_service_traits::ToolService> =
            Arc::new(LocalToolService::new(tool_system.clone()));
        let memory_service: Arc<dyn gateway_service_traits::MemoryService> =
            Arc::new(LocalMemoryService::new(unified_memory.clone()));

        Ok(Self {
            db,
            llm_router,
            profile_catalog,
            health_tracker,
            cost_tracker,
            model_registry,
            routing_engine,
            mcp_gateway,
            tool_system,
            workflow_engine: Arc::new(RwLock::new(workflow_engine)),
            workflow_executor: Arc::new(RwLock::new(workflow_executor)),
            chat_engine,
            unified_memory,
            artifact_store: Arc::new(RwLock::new(artifact_store)),
            code_executor,
            filesystem_service,
            session_repository,
            checkpoint_manager,
            agent_session_manager,
            container_orchestrator,
            task_store,
            permission_manager,
            ws_manager,
            message_repository,
            conversation_repository,
            code_router,
            #[cfg(unix)]
            vm_manager,
            worker_manager,
            code_orchestration_runtime,
            settings_path,
            cached_settings: Arc::new(RwLock::new(cached_settings)),
            agent_factory,
            agent_tool_registry,
            billing_service,
            automation_orchestrator,
            #[cfg(feature = "orchestration")]
            template_registry,
            #[cfg(feature = "orchestration")]
            workflow_registry,
            #[cfg(feature = "cache")]
            plan_cache,
            #[cfg(feature = "cache")]
            semantic_cache,
            #[cfg(feature = "learning")]
            learning_engine,
            context_resolver_flags,
            #[cfg(feature = "graph")]
            execution_store,
            #[cfg(feature = "devtools")]
            devtools_service,
            #[cfg(feature = "collaboration")]
            auto_mode_selector,
            #[cfg(feature = "collaboration")]
            task_classifier,
            #[cfg(feature = "collaboration")]
            pending_plan_approvals,
            #[cfg(feature = "collaboration")]
            pending_clarifications,
            #[cfg(feature = "collaboration")]
            pending_prd_approvals,
            plugin_manager,
            category_resolver: Arc::new(RwLock::new(
                gateway_core::connectors::CategoryResolver::with_defaults(),
            )),
            bundle_manager: {
                let bundle_dirs: Vec<std::path::PathBuf> = vec!["plugin-bundles/".into()];
                let mut mgr = gateway_core::connectors::BundleManager::new(bundle_dirs);
                let discovered = mgr.discover();
                tracing::info!(
                    bundles_discovered = discovered,
                    "Bundle definitions scanned"
                );
                Arc::new(RwLock::new(mgr))
            },
            runtime_registry: {
                let activations_path = std::path::PathBuf::from("data/bundle-activations.json");
                if let Some(parent) = activations_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let registry =
                    gateway_core::connectors::RuntimeRegistry::load(activations_path.clone())
                        .unwrap_or_else(|e| {
                            tracing::warn!(
                                "Failed to load bundle activations: {}, starting fresh",
                                e
                            );
                            gateway_core::connectors::RuntimeRegistry::new(activations_path)
                        });
                Arc::new(RwLock::new(registry))
            },
            mcp_ref_tracker: Arc::new(gateway_core::connectors::McpRefTracker::new()),
            mcp_connection_tracker: Arc::new(gateway_core::connectors::McpConnectionTracker::new()),
            rte_pending: Arc::new(PendingToolExecutions::new()),
            rte_fallback_config: Arc::new(ToolFallbackConfig::default()),
            rate_limiter: Arc::new(RateLimiter::default()),
            llm_service,
            tool_service,
            memory_service,
            started_at: std::time::Instant::now(),
            identity_service,
            // Billing v2 services — shared stores (A37)
            #[cfg(feature = "billing")]
            billing_service_v2: billing_v2_services.0.clone(),
            #[cfg(feature = "billing")]
            metering_service: billing_v2_services.1.clone(),
            #[cfg(feature = "billing")]
            budget_guard: billing_v2_services.2.clone(),
            #[cfg(feature = "billing")]
            gift_card_service_v2: billing_v2_services.3.clone(),
            // Async job system
            #[cfg(feature = "jobs")]
            job_store: job_store.clone(),
            #[cfg(feature = "jobs")]
            job_scheduler: job_scheduler.clone(),
            #[cfg(feature = "jobs")]
            pending_hitl_inputs: shared_hitl_inputs.clone(),
            // A43: Step delegation
            #[cfg(feature = "collaboration")]
            pending_step_executions: Arc::new(
                gateway_core::agent::step_delegate::PendingStepExecutions::new(),
            ),
            // CP16a: Audit log store
            audit_store: Arc::new(crate::middleware::audit::AuditStore::new()),
            // Microservice mode: remote agent client
            remote_agent_client: {
                if std::env::var("CANAL_MODE").as_deref() == Ok("microservice") {
                    let agent_url = std::env::var("AGENT_SERVICE_URL")
                        .unwrap_or_else(|_| "http://127.0.0.1:4033".into());
                    tracing::info!(url = %agent_url, "Microservice mode: connecting to agent-service");
                    match crate::remote_agent::RemoteAgentClient::connect(agent_url).await {
                        Ok(client) => {
                            tracing::info!("Remote agent client connected");
                            Some(Arc::new(client))
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to connect to agent-service — falling back to monolith mode");
                            None
                        }
                    }
                } else {
                    None
                }
            },
        })
    }

    /// Get the container orchestrator if available
    #[allow(dead_code)]
    pub fn orchestrator(&self) -> Option<&Arc<ContainerOrchestrator>> {
        self.container_orchestrator.as_ref()
    }

    /// Get the container orchestrator or return a service unavailable error
    pub fn require_orchestrator(&self) -> Result<&Arc<ContainerOrchestrator>, ApiError> {
        self.container_orchestrator.as_ref().ok_or_else(|| {
            ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Container orchestration not enabled",
            )
        })
    }

    /// Get the automation orchestrator if available
    pub fn automation_orchestrator(&self) -> Option<&Arc<BrowserAutomationOrchestrator>> {
        self.automation_orchestrator.as_ref()
    }

    /// Get the template registry for graph workflow patterns
    #[cfg(feature = "orchestration")]
    pub fn template_registry(&self) -> &Arc<TemplateRegistry> {
        &self.template_registry
    }

    /// Get the template registry or return a service unavailable error
    #[cfg(feature = "orchestration")]
    pub fn require_template_registry(&self) -> Result<&Arc<TemplateRegistry>, ApiError> {
        Ok(&self.template_registry)
    }

    /// Spawn background MCP server reconnection for active bundles.
    ///
    /// On startup, iterates all persisted bundle activations, collects
    /// unique MCP servers via ref tracker, and connects them with a
    /// semaphore-limited concurrency of 5.
    pub fn spawn_mcp_startup_reconnect(&self) {
        use gateway_core::connectors::McpConnectionStatus;
        use gateway_core::mcp::gateway::{McpServerConfig as GwMcpServerConfig, McpTransport};

        let runtime_registry = self.runtime_registry.clone();
        let bundle_manager = self.bundle_manager.clone();
        let ref_tracker = self.mcp_ref_tracker.clone();
        let conn_tracker = self.mcp_connection_tracker.clone();
        let mcp_gateway = self.mcp_gateway.clone();

        tokio::spawn(async move {
            // Read persisted activations
            let runtime = runtime_registry.read().await;
            let all_active = runtime.all_active_bundles();
            drop(runtime);

            let bundle_mgr = bundle_manager.read().await;

            // Collect unique servers and rebuild ref tracker
            let mut unique_servers: std::collections::HashMap<
                String,
                gateway_core::connectors::McpServerDef,
            > = std::collections::HashMap::new();
            let mut rebuild_data: Vec<(String, Vec<String>)> = Vec::new();

            for (_user_id, bundle_names) in &all_active {
                for bundle_name in bundle_names {
                    if let Some(bundle) = bundle_mgr.get(bundle_name) {
                        let server_names: Vec<String> =
                            bundle.mcp_servers.iter().map(|s| s.name.clone()).collect();
                        rebuild_data.push((bundle_name.clone(), server_names));
                        for server_def in &bundle.mcp_servers {
                            unique_servers
                                .entry(server_def.name.clone())
                                .or_insert_with(|| server_def.clone());
                        }
                    }
                }
            }
            drop(bundle_mgr);

            if unique_servers.is_empty() {
                tracing::debug!("No MCP servers to reconnect on startup");
                return;
            }

            // Rebuild ref tracker
            ref_tracker.rebuild(&rebuild_data);

            tracing::info!(
                server_count = unique_servers.len(),
                "Reconnecting MCP servers from active bundles"
            );

            // Background connect with semaphore (max 5 concurrent)
            let semaphore = Arc::new(tokio::sync::Semaphore::new(5));
            let mut handles = Vec::new();

            for (name, server_def) in unique_servers {
                let gw = mcp_gateway.clone();
                let tracker = conn_tracker.clone();
                let sem = semaphore.clone();

                let handle = tokio::spawn(async move {
                    let _permit = sem.acquire().await;
                    tracker.set_status(&name, McpConnectionStatus::Connecting);

                    let config = GwMcpServerConfig {
                        name: name.clone(),
                        transport: McpTransport::Http {
                            url: server_def.url.clone(),
                        },
                        enabled: true,
                        namespace: name.clone(),
                        startup_timeout_secs: 30,
                        auto_restart: false,
                        auth_token: server_def.auth_token.clone(),
                    };

                    if let Err(e) = gw.register_server_config(config).await {
                        tracing::warn!(
                            server = %name,
                            error = %e,
                            "Failed to register MCP config on startup"
                        );
                        tracker.set_status(&name, McpConnectionStatus::Failed(e.to_string()));
                        return;
                    }

                    match gw.connect_server(&name).await {
                        Ok(()) => {
                            tracing::info!(server = %name, "MCP server reconnected on startup");
                            tracker.set_status(&name, McpConnectionStatus::Connected);
                        }
                        Err(e) => {
                            tracing::warn!(
                                server = %name,
                                error = %e,
                                "MCP server reconnect failed"
                            );
                            tracker.set_status(&name, McpConnectionStatus::Failed(e.to_string()));
                        }
                    }
                });
                handles.push(handle);
            }

            // Wait for all connections to complete
            for handle in handles {
                let _ = handle.await;
            }

            tracing::info!("MCP startup reconnection complete");
        });
    }

    /// Spawn the background learning scheduler.
    ///
    /// This task periodically checks if the experience buffer has reached
    /// its threshold and triggers a learning cycle when appropriate.
    /// The scheduler runs every 30 seconds and only triggers when:
    /// - Learning is enabled
    /// - Buffer threshold is reached (default: 10 experiences)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let state = AppState::new().await;
    /// state.spawn_learning_scheduler();
    /// ```
    #[cfg(feature = "learning")]
    pub fn spawn_learning_scheduler(&self) {
        let engine = self.learning_engine.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;

                // Skip if learning is disabled
                if !engine.is_enabled() {
                    continue;
                }

                // Check buffer threshold
                let collector = engine.collector();
                if collector.is_threshold_reached().await {
                    let buffer_size = collector.buffer_size().await;
                    tracing::info!(
                        buffer_size,
                        "Buffer threshold reached, triggering learning cycle"
                    );
                    match engine.learn().await {
                        Ok(report) => {
                            tracing::info!(
                                experiences = report.experiences_processed,
                                patterns = report.patterns_mined,
                                stored = report.patterns_stored,
                                "Background learning cycle completed"
                            );
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Background learning cycle failed");
                        }
                    }
                }
            }
        });
        tracing::info!("Learning scheduler spawned (30s interval, threshold-triggered)");
    }

    /// Resolve the path to the settings JSON file.
    /// Uses `~/.canal/settings.json` via `dirs::config_dir()`,
    /// or falls back to `~/.canal/` under the home directory.
    fn resolve_settings_path() -> PathBuf {
        let base = std::env::var("CANAL_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::config_dir()
                    .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
                    .join("canal")
            });
        base.join("settings.json")
    }

    /// Load settings from disk, returning an empty JSON object if the file
    /// doesn't exist or can't be parsed.
    fn load_settings_from_disk(path: &PathBuf) -> serde_json::Value {
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str::<serde_json::Value>(&contents) {
                Ok(val) if val.is_object() => {
                    tracing::info!(path = %path.display(), "Loaded persisted settings");
                    val
                }
                Ok(_) => {
                    tracing::warn!(path = %path.display(), "Settings file is not a JSON object, ignoring");
                    serde_json::json!({})
                }
                Err(e) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "Failed to parse settings file, using defaults"
                    );
                    serde_json::json!({})
                }
            },
            Err(_) => {
                tracing::debug!(path = %path.display(), "No settings file found, using defaults");
                serde_json::json!({})
            }
        }
    }

    /// Persist the current cached settings to disk atomically.
    /// Writes to a `.tmp` file first, then renames to the target path.
    pub async fn save_settings_to_disk(&self) -> Result<(), ApiError> {
        let settings = self.cached_settings.read().await.clone();
        let path = self.settings_path.clone();

        // Run blocking I/O on a dedicated thread
        tokio::task::spawn_blocking(move || {
            // Ensure parent directory exists
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ApiError::internal(format!(
                        "Failed to create settings directory {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
            }

            let tmp_path = path.with_extension("json.tmp");
            let json_bytes = serde_json::to_string_pretty(&settings)
                .map_err(|e| ApiError::internal(format!("Failed to serialize settings: {}", e)))?;

            std::fs::write(&tmp_path, json_bytes.as_bytes()).map_err(|e| {
                ApiError::internal(format!(
                    "Failed to write settings to {}: {}",
                    tmp_path.display(),
                    e
                ))
            })?;

            std::fs::rename(&tmp_path, &path).map_err(|e| {
                ApiError::internal(format!(
                    "Failed to rename {} -> {}: {}",
                    tmp_path.display(),
                    path.display(),
                    e
                ))
            })?;

            Ok(())
        })
        .await
        .map_err(|e| ApiError::internal(format!("Settings save task panicked: {}", e)))?
    }

    /// Merge a partial JSON object into cached settings under the given key,
    /// then persist to disk.
    pub async fn merge_and_persist_settings(
        &self,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), ApiError> {
        {
            let mut settings = self.cached_settings.write().await;
            let obj = settings
                .as_object_mut()
                .expect("cached_settings is always an object");
            obj.insert(key.to_string(), value);
        }
        self.save_settings_to_disk().await
    }

    /// Merge a flat JSON value into the top-level cached settings, then persist.
    pub async fn merge_and_persist_settings_flat(
        &self,
        value: serde_json::Value,
    ) -> Result<(), ApiError> {
        {
            let mut settings = self.cached_settings.write().await;
            if let (Some(existing), Some(incoming)) = (settings.as_object_mut(), value.as_object())
            {
                for (k, v) in incoming {
                    existing.insert(k.clone(), v.clone());
                }
            }
        }
        self.save_settings_to_disk().await
    }

    // ============ Prompt Constraint System Methods ============

    /// Get the constraint profile registry.
    ///
    /// Loads profiles from `config/constraints/` directory.
    #[cfg(feature = "prompt-constraints")]
    pub async fn get_profile_registry(
        &self,
    ) -> Result<gateway_core::prompt::ProfileRegistry, ApiError> {
        use gateway_core::prompt::ProfileRegistry;
        use std::path::Path;

        let config_path = std::env::var("CONSTRAINTS_CONFIG_PATH")
            .unwrap_or_else(|_| "config/constraints".to_string());

        let path = Path::new(&config_path);

        // If directory doesn't exist, return empty registry with default profile
        if !path.exists() {
            let mut registry = ProfileRegistry::new();
            registry.register(gateway_core::prompt::ConstraintProfile::default());
            return Ok(registry);
        }

        ProfileRegistry::load_from_directory(path)
            .map_err(|e| ApiError::internal(format!("Failed to load constraint profiles: {}", e)))
    }

    /// Get the current user's prompt overrides.
    ///
    /// Loads from `~/.canal/prompt_overrides.yaml`.
    #[cfg(feature = "prompt-constraints")]
    pub async fn get_user_overrides(
        &self,
    ) -> Result<gateway_core::prompt::UserPromptOverrides, ApiError> {
        use gateway_core::prompt::UserPromptOverrides;

        UserPromptOverrides::load()
            .map_err(|e| ApiError::internal(format!("Failed to load user overrides: {}", e)))
    }

    /// Save the user's prompt overrides.
    ///
    /// Persists to `~/.canal/prompt_overrides.yaml`.
    #[cfg(feature = "prompt-constraints")]
    pub async fn save_user_overrides(
        &self,
        overrides: &gateway_core::prompt::UserPromptOverrides,
    ) -> Result<(), ApiError> {
        overrides
            .save()
            .map_err(|e| ApiError::internal(format!("Failed to save user overrides: {}", e)))
    }
}

/// Seed the default admin user if it doesn't exist.
///
/// Reads credentials from environment variables:
/// - `ADMIN_EMAIL`: admin account email (required)
/// - `ADMIN_PASSWORD`: admin account password (required)
///
/// If either variable is unset, admin seeding is skipped.
async fn seed_admin_user(db: &PgPool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let admin_email = match std::env::var("ADMIN_EMAIL") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            tracing::info!("ADMIN_EMAIL not set, skipping admin user seeding");
            return Ok(());
        }
    };
    let admin_password = match std::env::var("ADMIN_PASSWORD") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            tracing::info!("ADMIN_PASSWORD not set, skipping admin user seeding");
            return Ok(());
        }
    };

    // Check if admin already exists
    let existing: Option<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(&admin_email)
        .fetch_optional(db)
        .await?;

    if existing.is_some() {
        tracing::info!(email = %admin_email, "Admin user already exists");
        return Ok(());
    }

    // Hash the password
    let password_hash = bcrypt::hash(&admin_password, bcrypt::DEFAULT_COST)?;

    // Create admin user
    sqlx::query(
        r#"
        INSERT INTO users (email, name, password_hash, role, status)
        VALUES ($1, $2, $3, 'admin', 'active')
        "#,
    )
    .bind(&admin_email)
    .bind("Administrator")
    .bind(&password_hash)
    .execute(db)
    .await?;

    tracing::info!(email = %admin_email, "Admin user created successfully");
    Ok(())
}
