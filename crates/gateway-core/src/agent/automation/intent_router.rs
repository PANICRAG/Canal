//! Intent Router - Layer 1 of the Five-Layer Automation Architecture
//!
//! Analyzes tasks and determines the optimal execution path based on:
//! - Target system (has API? uses canvas?)
//! - Data volume (small = pure CV, large = script generation)
//! - Cached assets (reuse if available)

use super::asset_store::AssetStore;
use super::types::{AutomationPath, PathDecision, RouteAnalysis};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

// ============================================================================
// Target System Detection
// ============================================================================

/// Known target system with its characteristics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetSystem {
    /// System identifier (e.g., "google-sheets", "notion")
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Whether this system has a usable API
    pub has_api: bool,
    /// API information if available
    pub api_info: Option<ApiInfo>,
    /// Whether this system uses canvas rendering
    pub uses_canvas: bool,
    /// Whether this system blocks DOM manipulation
    pub blocks_dom: bool,
    /// URL patterns that match this system
    pub url_patterns: Vec<String>,
    /// Keywords that identify this system in task descriptions
    pub keywords: Vec<String>,
}

impl TargetSystem {
    /// Check if a URL matches this system
    pub fn matches_url(&self, url: &str) -> bool {
        self.url_patterns.iter().any(|pattern| {
            // Simple pattern matching - could be regex
            url.contains(pattern)
        })
    }

    /// Check if task keywords match this system
    pub fn matches_keywords(&self, task: &str) -> bool {
        let task_lower = task.to_lowercase();
        self.keywords.iter().any(|kw| task_lower.contains(kw))
    }
}

/// API information for a target system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiInfo {
    /// API type (rest, graphql, etc.)
    pub api_type: String,
    /// Base endpoint
    pub base_url: Option<String>,
    /// Authentication method
    pub auth_method: Option<String>,
    /// Documentation URL
    pub docs_url: Option<String>,
    /// Commonly used endpoints
    pub common_endpoints: HashMap<String, String>,
}

// ============================================================================
// Intent Analysis
// ============================================================================

/// Result of analyzing a task's intent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentAnalysis {
    /// Original task description
    pub task: String,
    /// Detected target system
    pub target_system: Option<TargetSystem>,
    /// Estimated data volume
    pub data_volume: DataVolume,
    /// Task category
    pub task_category: TaskCategory,
    /// Extracted entities (URLs, names, etc.)
    pub entities: Vec<ExtractedEntity>,
    /// Confidence in the analysis
    pub confidence: f64,
}

/// Estimated data volume
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataVolume {
    /// Very small (1-5 items)
    Tiny,
    /// Small (6-20 items) - borderline for CV
    Small,
    /// Medium (21-100 items) - definitely use scripts
    Medium,
    /// Large (101-1000 items)
    Large,
    /// Very large (1000+ items)
    VeryLarge,
    /// Unknown
    Unknown,
}

impl DataVolume {
    /// Create from item count
    pub fn from_count(count: usize) -> Self {
        match count {
            0..=5 => DataVolume::Tiny,
            6..=20 => DataVolume::Small,
            21..=100 => DataVolume::Medium,
            101..=1000 => DataVolume::Large,
            _ => DataVolume::VeryLarge,
        }
    }

    /// Whether this volume warrants script generation
    pub fn should_generate_script(&self) -> bool {
        matches!(
            self,
            DataVolume::Medium | DataVolume::Large | DataVolume::VeryLarge
        )
    }

    /// Estimated tokens for pure CV approach
    pub fn pure_cv_tokens(&self, items: usize) -> u64 {
        // Each CV operation: ~4000 tokens (screenshot + reasoning)
        // Per item: navigate + click + type + verify = ~4 operations = ~16000 tokens
        // With optimizations (batch verification): ~10000 tokens per item
        let per_item = 10_000u64;
        match self {
            DataVolume::Tiny => (items as u64) * 20_000, // Less optimization for tiny sets
            DataVolume::Small => (items as u64) * 15_000,
            _ => (items as u64) * per_item,
        }
    }
}

/// Task category
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCategory {
    /// Data entry (filling forms, spreadsheets)
    DataEntry,
    /// Data extraction (scraping)
    DataExtraction,
    /// Navigation (clicking through pages)
    Navigation,
    /// Form submission
    FormSubmission,
    /// File upload/download
    FileOperation,
    /// Document creation
    DocumentCreation,
    /// Mixed/Complex
    Complex,
    /// Unknown
    Unknown,
}

/// Extracted entity from task description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEntity {
    pub entity_type: String,
    pub value: String,
    pub confidence: f64,
}

// ============================================================================
// Intent Router
// ============================================================================

/// Intent Router - Determines optimal automation path
pub struct IntentRouter {
    /// Known target systems
    known_systems: Vec<TargetSystem>,
    /// Asset store for checking cached scripts
    asset_store: Option<Arc<dyn AssetStore>>,
    /// Configuration
    config: IntentRouterConfig,
}

/// Configuration for the intent router
#[derive(Debug, Clone)]
pub struct IntentRouterConfig {
    /// Threshold for preferring CV over scripts (item count)
    pub cv_threshold: usize,
    /// Minimum success rate for reusing scripts
    pub min_reuse_success_rate: f64,
    /// Maximum age for reusing scripts (seconds)
    pub max_script_age_secs: u64,
    /// Whether to prefer API when available
    pub prefer_api: bool,
}

impl Default for IntentRouterConfig {
    fn default() -> Self {
        Self {
            cv_threshold: 10,
            min_reuse_success_rate: 0.8,
            max_script_age_secs: 86400 * 7, // 7 days
            prefer_api: true,
        }
    }
}

impl IntentRouter {
    /// Create a new intent router with default systems
    pub fn new() -> Self {
        Self {
            known_systems: Self::default_known_systems(),
            asset_store: None,
            config: IntentRouterConfig::default(),
        }
    }

    /// Create a builder
    pub fn builder() -> IntentRouterBuilder {
        IntentRouterBuilder::default()
    }

    /// Get default known systems
    fn default_known_systems() -> Vec<TargetSystem> {
        vec![
            // Google Sheets - uses canvas, blocks DOM
            TargetSystem {
                id: "google-sheets".to_string(),
                name: "Google Sheets".to_string(),
                has_api: true,
                api_info: Some(ApiInfo {
                    api_type: "rest".to_string(),
                    base_url: Some("https://sheets.googleapis.com/v4/spreadsheets".to_string()),
                    auth_method: Some("oauth2".to_string()),
                    docs_url: Some("https://developers.google.com/sheets/api".to_string()),
                    common_endpoints: HashMap::from([
                        (
                            "read".to_string(),
                            "GET /spreadsheets/{id}/values/{range}".to_string(),
                        ),
                        (
                            "write".to_string(),
                            "PUT /spreadsheets/{id}/values/{range}".to_string(),
                        ),
                    ]),
                }),
                uses_canvas: true,
                blocks_dom: true,
                url_patterns: vec![
                    "sheets.google.com".to_string(),
                    "docs.google.com/spreadsheets".to_string(),
                ],
                keywords: vec![
                    "google sheets".to_string(),
                    "spreadsheet".to_string(),
                    "表格".to_string(),
                ],
            },
            // Google Docs - uses canvas for content area
            TargetSystem {
                id: "google-docs".to_string(),
                name: "Google Docs".to_string(),
                has_api: true,
                api_info: Some(ApiInfo {
                    api_type: "rest".to_string(),
                    base_url: Some("https://docs.googleapis.com/v1/documents".to_string()),
                    auth_method: Some("oauth2".to_string()),
                    docs_url: Some("https://developers.google.com/docs/api".to_string()),
                    common_endpoints: HashMap::new(),
                }),
                uses_canvas: true,
                blocks_dom: true,
                url_patterns: vec!["docs.google.com/document".to_string()],
                keywords: vec![
                    "google docs".to_string(),
                    "document".to_string(),
                    "文档".to_string(),
                ],
            },
            // Notion - hybrid API/UI
            TargetSystem {
                id: "notion".to_string(),
                name: "Notion".to_string(),
                has_api: true,
                api_info: Some(ApiInfo {
                    api_type: "rest".to_string(),
                    base_url: Some("https://api.notion.com/v1".to_string()),
                    auth_method: Some("bearer".to_string()),
                    docs_url: Some("https://developers.notion.com".to_string()),
                    common_endpoints: HashMap::new(),
                }),
                uses_canvas: false,
                blocks_dom: false,
                url_patterns: vec!["notion.so".to_string(), "notion.site".to_string()],
                keywords: vec!["notion".to_string()],
            },
            // Figma - uses canvas
            TargetSystem {
                id: "figma".to_string(),
                name: "Figma".to_string(),
                has_api: true,
                api_info: Some(ApiInfo {
                    api_type: "rest".to_string(),
                    base_url: Some("https://api.figma.com/v1".to_string()),
                    auth_method: Some("bearer".to_string()),
                    docs_url: Some("https://www.figma.com/developers/api".to_string()),
                    common_endpoints: HashMap::new(),
                }),
                uses_canvas: true,
                blocks_dom: true,
                url_patterns: vec!["figma.com".to_string()],
                keywords: vec!["figma".to_string()],
            },
            // Generic form websites
            TargetSystem {
                id: "generic-form".to_string(),
                name: "Generic Form".to_string(),
                has_api: false,
                api_info: None,
                uses_canvas: false,
                blocks_dom: false,
                url_patterns: vec![],
                keywords: vec!["form".to_string(), "表单".to_string(), "填写".to_string()],
            },
        ]
    }

    /// Analyze a task and determine the optimal path
    pub async fn analyze(&self, task: &str, data_count: Option<usize>) -> RouteAnalysis {
        // 1. Analyze the task
        let analysis = self.analyze_intent(task, data_count);

        // 2. Check for reusable scripts
        let reusable_script = self.find_reusable_script(task).await;

        // 3. Determine the path
        let decision = self.decide_path(&analysis, reusable_script);

        RouteAnalysis {
            task: task.to_string(),
            target_system: analysis
                .target_system
                .as_ref()
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            data_volume: data_count.unwrap_or(0),
            decision,
            analyzed_at: Utc::now(),
        }
    }

    /// Analyze task intent
    fn analyze_intent(&self, task: &str, data_count: Option<usize>) -> IntentAnalysis {
        // Detect target system
        let target_system = self.detect_target_system(task);

        // Estimate data volume
        let data_volume = data_count
            .map(DataVolume::from_count)
            .unwrap_or_else(|| self.estimate_data_volume(task));

        // Detect task category
        let task_category = self.detect_task_category(task);

        // Extract entities
        let entities = self.extract_entities(task);

        // Calculate confidence
        let confidence = if target_system.is_some() { 0.9 } else { 0.6 };

        IntentAnalysis {
            task: task.to_string(),
            target_system,
            data_volume,
            task_category,
            entities,
            confidence,
        }
    }

    /// Detect target system from task description
    fn detect_target_system(&self, task: &str) -> Option<TargetSystem> {
        // First try keyword matching
        for system in &self.known_systems {
            if system.matches_keywords(task) {
                return Some(system.clone());
            }
        }

        // Extract URLs and try URL matching
        let urls = self.extract_urls(task);
        for url in urls {
            for system in &self.known_systems {
                if system.matches_url(&url) {
                    return Some(system.clone());
                }
            }
        }

        None
    }

    /// Estimate data volume from task description
    fn estimate_data_volume(&self, task: &str) -> DataVolume {
        let task_lower = task.to_lowercase();

        // Look for explicit numbers
        let _number_patterns = [
            (r"\d+\s*(rows?|行|条)", 1.0),
            (r"\d+\s*(items?|个|项)", 1.0),
            (r"\d+\s*(records?|记录)", 1.0),
            (r"batch|bulk|批量", 0.5), // Implies large volume
            (r"all|全部|所有", 0.5),
        ];

        // Simple heuristic - in production would use regex
        if task_lower.contains("1000") || task_lower.contains("thousand") {
            return DataVolume::Large;
        }
        if task_lower.contains("100") || task_lower.contains("hundred") {
            return DataVolume::Medium;
        }
        if task_lower.contains("batch")
            || task_lower.contains("bulk")
            || task_lower.contains("批量")
        {
            return DataVolume::Large;
        }
        if task_lower.contains("few") || task_lower.contains("几") || task_lower.contains("some") {
            return DataVolume::Small;
        }
        if task_lower.contains("one")
            || task_lower.contains("single")
            || task_lower.contains("一个")
        {
            return DataVolume::Tiny;
        }

        DataVolume::Unknown
    }

    /// Detect task category
    fn detect_task_category(&self, task: &str) -> TaskCategory {
        let task_lower = task.to_lowercase();

        if task_lower.contains("fill")
            || task_lower.contains("enter")
            || task_lower.contains("input")
            || task_lower.contains("填写")
            || task_lower.contains("输入")
        {
            return TaskCategory::DataEntry;
        }

        if task_lower.contains("extract")
            || task_lower.contains("scrape")
            || task_lower.contains("get")
            || task_lower.contains("提取")
            || task_lower.contains("获取")
        {
            return TaskCategory::DataExtraction;
        }

        if task_lower.contains("create")
            || task_lower.contains("new")
            || task_lower.contains("创建")
            || task_lower.contains("新建")
        {
            return TaskCategory::DocumentCreation;
        }

        if task_lower.contains("submit") || task_lower.contains("提交") {
            return TaskCategory::FormSubmission;
        }

        if task_lower.contains("upload")
            || task_lower.contains("download")
            || task_lower.contains("上传")
            || task_lower.contains("下载")
        {
            return TaskCategory::FileOperation;
        }

        if task_lower.contains("navigate")
            || task_lower.contains("go to")
            || task_lower.contains("open")
            || task_lower.contains("打开")
            || task_lower.contains("访问")
        {
            return TaskCategory::Navigation;
        }

        TaskCategory::Unknown
    }

    /// Extract entities from task
    fn extract_entities(&self, task: &str) -> Vec<ExtractedEntity> {
        let mut entities = Vec::new();

        // Extract URLs
        for url in self.extract_urls(task) {
            entities.push(ExtractedEntity {
                entity_type: "url".to_string(),
                value: url,
                confidence: 0.95,
            });
        }

        // Could add more entity extraction here (names, numbers, etc.)

        entities
    }

    /// Extract URLs from text
    fn extract_urls(&self, text: &str) -> Vec<String> {
        // Simple URL extraction - in production would use proper regex
        text.split_whitespace()
            .filter(|word| word.starts_with("http://") || word.starts_with("https://"))
            .map(|s| s.to_string())
            .collect()
    }

    /// Find a reusable script for this task
    async fn find_reusable_script(&self, task: &str) -> Option<(String, f64)> {
        let store = self.asset_store.as_ref()?;

        // Generate task signature
        let signature = self.generate_task_signature(task);

        // Query asset store
        let query = super::types::AssetQuery::new()
            .with_task_signature(&signature)
            .with_min_success_rate(self.config.min_reuse_success_rate);

        if let Ok(Some(asset)) = store.find(&query).await {
            // Check age
            let age = Utc::now().signed_duration_since(asset.created_at);
            if age.num_seconds() as u64 <= self.config.max_script_age_secs {
                return Some((asset.id, asset.success_rate));
            }
        }

        None
    }

    /// Generate a task signature for caching
    fn generate_task_signature(&self, task: &str) -> String {
        // Normalize task description for matching
        // In production, would use semantic similarity
        let normalized = task
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>();

        // Simple hash
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        normalized.hash(&mut hasher);
        format!("task_{:x}", hasher.finish())
    }

    /// Decide the optimal path
    fn decide_path(
        &self,
        analysis: &IntentAnalysis,
        reusable_script: Option<(String, f64)>,
    ) -> PathDecision {
        let mut alternatives = Vec::new();

        // Check for reusable script first (best case)
        if let Some((script_id, success_rate)) = reusable_script {
            return PathDecision {
                path: AutomationPath::ReuseScript {
                    script_id,
                    last_success_rate: success_rate,
                },
                confidence: 0.95,
                reasoning: "Found cached script with good success rate".to_string(),
                alternatives,
                token_savings_percent: 99.99,
            };
        }

        // Get target system info
        let has_api = analysis
            .target_system
            .as_ref()
            .map(|s| s.has_api)
            .unwrap_or(false);
        let uses_canvas = analysis
            .target_system
            .as_ref()
            .map(|s| s.uses_canvas)
            .unwrap_or(false);
        let api_type = analysis
            .target_system
            .as_ref()
            .and_then(|s| s.api_info.as_ref())
            .map(|a| a.api_type.clone())
            .unwrap_or_else(|| "unknown".to_string());

        // Estimate item count
        let item_count = match analysis.data_volume {
            DataVolume::Tiny => 3,
            DataVolume::Small => 15,
            DataVolume::Medium => 50,
            DataVolume::Large => 500,
            DataVolume::VeryLarge => 2000,
            DataVolume::Unknown => 50, // Assume medium
        };

        // Calculate pure CV cost
        let pure_cv_tokens = analysis.data_volume.pure_cv_tokens(item_count);

        // Decision tree
        let (path, reasoning, confidence) = if has_api && self.config.prefer_api && !uses_canvas {
            // Direct API is best when available and not canvas-based
            (
                AutomationPath::DirectApi {
                    api_type: api_type.clone(),
                    api_endpoint: analysis
                        .target_system
                        .as_ref()
                        .and_then(|s| s.api_info.as_ref())
                        .and_then(|a| a.base_url.clone()),
                },
                "Target has API, preferring direct API access".to_string(),
                0.9,
            )
        } else if item_count <= self.config.cv_threshold {
            // Small data volume - pure CV is acceptable
            (
                AutomationPath::PureComputerVision {
                    max_items: item_count,
                    estimated_tokens: pure_cv_tokens,
                },
                format!("Small data volume ({} items), using pure CV", item_count),
                0.85,
            )
        } else if uses_canvas {
            // Canvas-based app with large data - need exploration + script
            (
                AutomationPath::ExploreAndGenerate {
                    target_url: analysis
                        .entities
                        .iter()
                        .find(|e| e.entity_type == "url")
                        .map(|e| e.value.clone())
                        .unwrap_or_default(),
                    estimated_tokens: 6000,
                },
                "Canvas-based app with large data, will explore and generate script".to_string(),
                0.85,
            )
        } else {
            // Standard web app with large data
            (
                AutomationPath::ExploreAndGenerate {
                    target_url: analysis
                        .entities
                        .iter()
                        .find(|e| e.entity_type == "url")
                        .map(|e| e.value.clone())
                        .unwrap_or_default(),
                    estimated_tokens: 5000,
                },
                "Large data volume, will explore and generate script".to_string(),
                0.8,
            )
        };

        // Add alternatives
        if !matches!(path, AutomationPath::PureComputerVision { .. }) {
            alternatives.push(AutomationPath::PureComputerVision {
                max_items: item_count,
                estimated_tokens: pure_cv_tokens,
            });
        }

        if has_api && !matches!(path, AutomationPath::DirectApi { .. }) {
            alternatives.push(AutomationPath::DirectApi {
                api_type,
                api_endpoint: None,
            });
        }

        // Calculate savings
        let chosen_tokens = path.estimated_tokens();
        let savings = if pure_cv_tokens > 0 {
            ((pure_cv_tokens - chosen_tokens) as f64 / pure_cv_tokens as f64) * 100.0
        } else {
            0.0
        };

        PathDecision {
            path,
            confidence,
            reasoning,
            alternatives,
            token_savings_percent: savings,
        }
    }
}

impl Default for IntentRouter {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Builder
// ============================================================================

/// Builder for IntentRouter
#[derive(Default)]
pub struct IntentRouterBuilder {
    known_systems: Option<Vec<TargetSystem>>,
    asset_store: Option<Arc<dyn AssetStore>>,
    config: IntentRouterConfig,
}

impl IntentRouterBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set known systems
    pub fn known_systems(mut self, systems: Vec<TargetSystem>) -> Self {
        self.known_systems = Some(systems);
        self
    }

    /// Add a known system
    pub fn add_system(mut self, system: TargetSystem) -> Self {
        self.known_systems
            .get_or_insert_with(|| IntentRouter::default_known_systems())
            .push(system);
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

    /// Set minimum reuse success rate
    pub fn min_reuse_success_rate(mut self, rate: f64) -> Self {
        self.config.min_reuse_success_rate = rate;
        self
    }

    /// Set maximum script age
    pub fn max_script_age(mut self, secs: u64) -> Self {
        self.config.max_script_age_secs = secs;
        self
    }

    /// Set prefer API flag
    pub fn prefer_api(mut self, prefer: bool) -> Self {
        self.config.prefer_api = prefer;
        self
    }

    /// Build the router
    pub fn build(self) -> IntentRouter {
        IntentRouter {
            known_systems: self
                .known_systems
                .unwrap_or_else(IntentRouter::default_known_systems),
            asset_store: self.asset_store,
            config: self.config,
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
    fn test_target_system_matching() {
        let system = TargetSystem {
            id: "google-sheets".to_string(),
            name: "Google Sheets".to_string(),
            has_api: true,
            api_info: None,
            uses_canvas: true,
            blocks_dom: true,
            url_patterns: vec!["sheets.google.com".to_string()],
            keywords: vec!["google sheets".to_string(), "spreadsheet".to_string()],
        };

        assert!(system.matches_url("https://sheets.google.com/d/abc"));
        assert!(system.matches_keywords("Fill data in Google Sheets"));
        assert!(system.matches_keywords("Update the spreadsheet"));
    }

    #[test]
    fn test_data_volume_from_count() {
        assert_eq!(DataVolume::from_count(3), DataVolume::Tiny);
        assert_eq!(DataVolume::from_count(10), DataVolume::Small);
        assert_eq!(DataVolume::from_count(50), DataVolume::Medium);
        assert_eq!(DataVolume::from_count(500), DataVolume::Large);
        assert_eq!(DataVolume::from_count(5000), DataVolume::VeryLarge);
    }

    #[test]
    fn test_data_volume_should_generate_script() {
        assert!(!DataVolume::Tiny.should_generate_script());
        assert!(!DataVolume::Small.should_generate_script());
        assert!(DataVolume::Medium.should_generate_script());
        assert!(DataVolume::Large.should_generate_script());
    }

    #[tokio::test]
    async fn test_intent_router_analyze() {
        let router = IntentRouter::new();

        // Test Google Sheets task
        let analysis = router
            .analyze("Fill 1000 rows in Google Sheets", Some(1000))
            .await;
        assert_eq!(analysis.target_system, "Google Sheets");
        assert!(matches!(
            analysis.decision.path,
            AutomationPath::ExploreAndGenerate { .. }
        ));

        // Test small task
        let analysis = router.analyze("Enter 5 items in a form", Some(5)).await;
        assert!(matches!(
            analysis.decision.path,
            AutomationPath::PureComputerVision { .. }
        ));
    }

    #[test]
    fn test_task_category_detection() {
        let router = IntentRouter::new();

        let analysis = router.analyze_intent("Fill the form with customer data", None);
        assert_eq!(analysis.task_category, TaskCategory::DataEntry);

        let analysis = router.analyze_intent("Extract prices from the website", None);
        assert_eq!(analysis.task_category, TaskCategory::DataExtraction);

        let analysis = router.analyze_intent("Create a new document", None);
        assert_eq!(analysis.task_category, TaskCategory::DocumentCreation);
    }

    #[test]
    fn test_path_decision_savings() {
        let router = IntentRouter::new();
        let analysis = router.analyze_intent("Process 1000 rows in Google Sheets", Some(1000));
        let decision = router.decide_path(&analysis, None);

        // Should have significant savings compared to pure CV
        assert!(decision.token_savings_percent > 90.0);
    }
}
