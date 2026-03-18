//! Core types for the Five-Layer Automation Architecture

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Page Schema Types (Layer 2 Output)
// ============================================================================

/// Represents the structured understanding of a web page from CV analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSchema {
    /// URL of the analyzed page
    pub url: String,
    /// Page title
    pub title: String,
    /// Identified interactive elements
    pub elements: Vec<ElementSchema>,
    /// Possible actions on this page
    pub actions: Vec<ActionSchema>,
    /// Page-level metadata
    pub metadata: PageMetadata,
    /// Hash for cache invalidation
    pub schema_hash: String,
    /// When this schema was generated
    pub created_at: DateTime<Utc>,
}

impl PageSchema {
    /// Create a new empty page schema
    pub fn new(url: impl Into<String>, title: impl Into<String>) -> Self {
        let url = url.into();
        let title = title.into();
        let schema_hash = Self::compute_hash(&url, &title);

        Self {
            url,
            title,
            elements: Vec::new(),
            actions: Vec::new(),
            metadata: PageMetadata::default(),
            schema_hash,
            created_at: Utc::now(),
        }
    }

    /// Add an element to the schema
    pub fn with_element(mut self, element: ElementSchema) -> Self {
        self.elements.push(element);
        self
    }

    /// Add an action to the schema
    pub fn with_action(mut self, action: ActionSchema) -> Self {
        self.actions.push(action);
        self
    }

    /// Compute hash for cache key
    fn compute_hash(url: &str, title: &str) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        url.hash(&mut hasher);
        title.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }

    /// Find element by ID
    pub fn find_element(&self, id: &str) -> Option<&ElementSchema> {
        self.elements.iter().find(|e| e.id == id)
    }

    /// Find elements by type
    pub fn find_elements_by_type(&self, element_type: ElementType) -> Vec<&ElementSchema> {
        self.elements
            .iter()
            .filter(|e| e.element_type == element_type)
            .collect()
    }
}

/// Page-level metadata
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageMetadata {
    /// Detected application type (e.g., "google-sheets", "notion")
    pub app_type: Option<String>,
    /// Whether the page uses canvas rendering
    pub uses_canvas: bool,
    /// Whether the page has iframes
    pub has_iframes: bool,
    /// Viewport dimensions when captured
    pub viewport: Option<Viewport>,
    /// Additional properties
    pub properties: HashMap<String, serde_json::Value>,
}

/// Viewport dimensions
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

/// Represents an interactive element on the page
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementSchema {
    /// Unique identifier for this element
    pub id: String,
    /// Type of element
    pub element_type: ElementType,
    /// CSS selector (if available)
    pub selector: Option<String>,
    /// XPath selector (if available)
    pub xpath: Option<String>,
    /// Coordinates for CV-based clicking
    pub coordinates: Option<Coordinates>,
    /// Human-readable description
    pub description: String,
    /// Element text content (if any)
    pub text: Option<String>,
    /// Whether element is currently visible
    pub is_visible: bool,
    /// Whether element is interactive
    pub is_interactive: bool,
    /// Bounding box
    pub bounds: Option<BoundingBox>,
    /// Additional attributes
    pub attributes: HashMap<String, String>,
}

impl ElementSchema {
    /// Create a new element schema
    pub fn new(
        id: impl Into<String>,
        element_type: ElementType,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            element_type,
            selector: None,
            xpath: None,
            coordinates: None,
            description: description.into(),
            text: None,
            is_visible: true,
            is_interactive: true,
            bounds: None,
            attributes: HashMap::new(),
        }
    }

    /// Set CSS selector
    pub fn with_selector(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    /// Set coordinates
    pub fn with_coordinates(mut self, x: u32, y: u32) -> Self {
        self.coordinates = Some(Coordinates { x, y });
        self
    }

    /// Set text content
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }
}

/// Type of UI element
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElementType {
    Button,
    Input,
    TextArea,
    Link,
    Cell,
    Row,
    Column,
    Menu,
    MenuItem,
    Dropdown,
    Checkbox,
    Radio,
    Tab,
    Modal,
    Image,
    Icon,
    Container,
    Other,
}

impl Default for ElementType {
    fn default() -> Self {
        ElementType::Other
    }
}

/// Screen coordinates
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coordinates {
    pub x: u32,
    pub y: u32,
}

/// Bounding box
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl BoundingBox {
    /// Get center coordinates
    pub fn center(&self) -> Coordinates {
        Coordinates {
            x: self.x + self.width / 2,
            y: self.y + self.height / 2,
        }
    }
}

/// Represents an action that can be performed on the page
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionSchema {
    /// Action name (e.g., "click_cell", "enter_text")
    pub name: String,
    /// Target element ID
    pub target_element_id: String,
    /// Parameters this action accepts
    pub parameters: Vec<ActionParameter>,
    /// Description of what this action does
    pub description: String,
    /// Preconditions for this action
    pub preconditions: Vec<String>,
    /// Expected outcome
    pub expected_outcome: Option<String>,
}

impl ActionSchema {
    /// Create a new action schema
    pub fn new(
        name: impl Into<String>,
        target_element_id: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            target_element_id: target_element_id.into(),
            parameters: Vec::new(),
            description: description.into(),
            preconditions: Vec::new(),
            expected_outcome: None,
        }
    }

    /// Add a parameter
    pub fn with_parameter(mut self, param: ActionParameter) -> Self {
        self.parameters.push(param);
        self
    }
}

/// Action parameter definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionParameter {
    pub name: String,
    pub param_type: ParameterType,
    pub description: String,
    pub required: bool,
    pub default_value: Option<serde_json::Value>,
}

/// Parameter types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterType {
    String,
    Number,
    Boolean,
    Coordinates,
    Array,
    Object,
}

// ============================================================================
// Script Types (Layer 3 Output)
// ============================================================================

/// Generated automation script
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedScript {
    /// Unique script identifier
    pub id: String,
    /// Script type
    pub script_type: ScriptType,
    /// The actual script code
    pub code: String,
    /// Language of the script
    pub language: String,
    /// Hash of the schema this was generated from
    pub schema_hash: String,
    /// Task signature this script handles
    pub task_signature: String,
    /// Script metadata
    pub metadata: ScriptMetadata,
    /// When this script was generated
    pub created_at: DateTime<Utc>,
}

impl GeneratedScript {
    /// Create a new generated script
    pub fn new(
        script_type: ScriptType,
        code: impl Into<String>,
        language: impl Into<String>,
        schema_hash: impl Into<String>,
        task_signature: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            script_type,
            code: code.into(),
            language: language.into(),
            schema_hash: schema_hash.into(),
            task_signature: task_signature.into(),
            metadata: ScriptMetadata::default(),
            created_at: Utc::now(),
        }
    }
}

/// Type of automation script
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptType {
    /// Playwright browser automation
    Playwright,
    /// Selenium browser automation
    Selenium,
    /// Puppeteer browser automation
    Puppeteer,
    /// REST API calls
    RestApi,
    /// GraphQL API calls
    GraphQl,
    /// Native application automation
    Native,
}

impl Default for ScriptType {
    fn default() -> Self {
        ScriptType::Playwright
    }
}

/// Script metadata
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScriptMetadata {
    /// Estimated execution time in milliseconds
    pub estimated_duration_ms: Option<u64>,
    /// Whether script handles pagination
    pub handles_pagination: bool,
    /// Whether script has retry logic
    pub has_retry_logic: bool,
    /// Maximum retries
    pub max_retries: u32,
    /// Dependencies (npm packages, etc.)
    pub dependencies: Vec<String>,
    /// Environment variables required
    pub env_vars: Vec<String>,
    /// Additional properties
    pub properties: HashMap<String, serde_json::Value>,
}

// ============================================================================
// Asset Types (Layer 5)
// ============================================================================

/// Stored script asset for reuse
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptAsset {
    /// Unique asset identifier
    pub id: String,
    /// Task signature this asset handles
    pub task_signature: String,
    /// Hash of the schema this was generated from
    pub schema_hash: String,
    /// The script code
    pub code: String,
    /// Script type
    pub script_type: ScriptType,
    /// Script language
    pub language: String,
    /// When this asset was created
    pub created_at: DateTime<Utc>,
    /// When this asset was last used
    pub last_used_at: DateTime<Utc>,
    /// Number of times this asset has been used
    pub use_count: u64,
    /// Success rate (0.0 - 1.0)
    pub success_rate: f64,
    /// Total executions
    pub total_executions: u64,
    /// Successful executions
    pub successful_executions: u64,
    /// Target URL pattern
    pub url_pattern: Option<String>,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

impl ScriptAsset {
    /// Create from a generated script
    pub fn from_script(script: &GeneratedScript) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            task_signature: script.task_signature.clone(),
            schema_hash: script.schema_hash.clone(),
            code: script.code.clone(),
            script_type: script.script_type,
            language: script.language.clone(),
            created_at: Utc::now(),
            last_used_at: Utc::now(),
            use_count: 0,
            success_rate: 1.0,
            total_executions: 0,
            successful_executions: 0,
            url_pattern: None,
            metadata: HashMap::new(),
        }
    }

    /// Record a usage
    pub fn record_usage(&mut self, success: bool) {
        self.use_count += 1;
        self.total_executions += 1;
        if success {
            self.successful_executions += 1;
        }
        self.success_rate = self.successful_executions as f64 / self.total_executions as f64;
        self.last_used_at = Utc::now();
    }
}

/// Query parameters for finding assets
#[derive(Debug, Clone, Default)]
pub struct AssetQuery {
    /// Task signature to match
    pub task_signature: Option<String>,
    /// URL pattern to match
    pub url_pattern: Option<String>,
    /// Minimum success rate
    pub min_success_rate: Option<f64>,
    /// Maximum age in seconds
    pub max_age_secs: Option<u64>,
    /// Limit results
    pub limit: Option<usize>,
}

impl AssetQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_task_signature(mut self, sig: impl Into<String>) -> Self {
        self.task_signature = Some(sig.into());
        self
    }

    pub fn with_url_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.url_pattern = Some(pattern.into());
        self
    }

    pub fn with_min_success_rate(mut self, rate: f64) -> Self {
        self.min_success_rate = Some(rate);
        self
    }
}

/// Asset store statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssetStats {
    pub total_assets: u64,
    pub total_executions: u64,
    pub successful_executions: u64,
    pub average_success_rate: f64,
    pub most_used_assets: Vec<String>,
    pub tokens_saved: u64,
}

// ============================================================================
// Automation Path (Layer 1 Decision)
// ============================================================================

/// The optimal automation path determined by intent analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomationPath {
    /// Direct API call (has known API, ~500 tokens)
    DirectApi {
        api_type: String,
        api_endpoint: Option<String>,
    },

    /// CV exploration + script generation (~6000 tokens)
    ExploreAndGenerate {
        target_url: String,
        estimated_tokens: u64,
    },

    /// Pure CV operation for small data (<10 items, ~20000 tokens)
    PureComputerVision {
        max_items: usize,
        estimated_tokens: u64,
    },

    /// Reuse existing script asset (~500 tokens)
    ReuseScript {
        script_id: String,
        last_success_rate: f64,
    },

    /// Hybrid: CV for exploration, script for bulk (~6500 tokens)
    HybridApproach {
        explore_phase_tokens: u64,
        execute_phase_tokens: u64,
    },

    /// Manual intervention required
    RequiresHumanAssistance { reason: String },
}

impl AutomationPath {
    /// Get estimated token cost
    pub fn estimated_tokens(&self) -> u64 {
        match self {
            Self::DirectApi { .. } => 500,
            Self::ExploreAndGenerate {
                estimated_tokens, ..
            } => *estimated_tokens,
            Self::PureComputerVision {
                estimated_tokens, ..
            } => *estimated_tokens,
            Self::ReuseScript { .. } => 500,
            Self::HybridApproach {
                explore_phase_tokens,
                execute_phase_tokens,
            } => explore_phase_tokens + execute_phase_tokens,
            Self::RequiresHumanAssistance { .. } => 0,
        }
    }
}

/// Decision made by the router with explanation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathDecision {
    /// The chosen path
    pub path: AutomationPath,
    /// Confidence in this decision (0.0 - 1.0)
    pub confidence: f64,
    /// Reasoning for this decision
    pub reasoning: String,
    /// Alternative paths considered
    pub alternatives: Vec<AutomationPath>,
    /// Estimated savings compared to pure CV
    pub token_savings_percent: f64,
}

/// Full analysis result from intent routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteAnalysis {
    /// Task description
    pub task: String,
    /// Detected target system
    pub target_system: String,
    /// Estimated data volume
    pub data_volume: usize,
    /// Path decision
    pub decision: PathDecision,
    /// Analysis timestamp
    pub analyzed_at: DateTime<Utc>,
}

// ============================================================================
// Execution Types
// ============================================================================

/// Request for automation execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationRequest {
    /// Unique request ID
    pub id: String,
    /// Task description
    pub task: String,
    /// Target URL (if known)
    pub target_url: Option<String>,
    /// Data to process
    pub data: Vec<serde_json::Value>,
    /// Session ID for stateful operations
    pub session_id: Option<String>,
    /// Timeout in milliseconds
    pub timeout_ms: u64,
    /// Additional options
    pub options: HashMap<String, serde_json::Value>,
    /// When request was created
    pub created_at: DateTime<Utc>,
}

impl AutomationRequest {
    /// Create a new automation request
    pub fn new(task: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            task: task.into(),
            target_url: None,
            data: Vec::new(),
            session_id: None,
            timeout_ms: 300000, // 5 minutes default
            options: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    /// Set target URL
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.target_url = Some(url.into());
        self
    }

    /// Set data to process
    pub fn with_data(mut self, data: Vec<serde_json::Value>) -> Self {
        self.data = data;
        self
    }

    /// Set session ID
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Get data volume
    pub fn data_volume(&self) -> usize {
        self.data.len()
    }
}

/// Result of automation execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationResult {
    /// Request ID this result corresponds to
    pub request_id: String,
    /// Whether execution was successful
    pub success: bool,
    /// Output data
    pub output: Option<serde_json::Value>,
    /// Error message if failed
    pub error: Option<String>,
    /// Path that was used
    pub path_used: AutomationPath,
    /// Execution statistics
    pub stats: ExecutionStats,
    /// Generated/used script ID (for caching)
    pub script_id: Option<String>,
    /// When execution completed
    pub completed_at: DateTime<Utc>,
}

impl AutomationResult {
    /// Create a success result
    pub fn success(request_id: impl Into<String>, path_used: AutomationPath) -> Self {
        Self {
            request_id: request_id.into(),
            success: true,
            output: None,
            error: None,
            path_used,
            stats: ExecutionStats::default(),
            script_id: None,
            completed_at: Utc::now(),
        }
    }

    /// Create a failure result
    pub fn failure(
        request_id: impl Into<String>,
        path_used: AutomationPath,
        error: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            success: false,
            output: None,
            error: Some(error.into()),
            path_used,
            stats: ExecutionStats::default(),
            script_id: None,
            completed_at: Utc::now(),
        }
    }

    /// Set output
    pub fn with_output(mut self, output: serde_json::Value) -> Self {
        self.output = Some(output);
        self
    }

    /// Set stats
    pub fn with_stats(mut self, stats: ExecutionStats) -> Self {
        self.stats = stats;
        self
    }

    /// Set script ID
    pub fn with_script_id(mut self, id: impl Into<String>) -> Self {
        self.script_id = Some(id.into());
        self
    }
}

/// Execution statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionStats {
    /// Total execution duration in milliseconds
    pub duration_ms: u64,
    /// Tokens used in exploration phase
    pub exploration_tokens: u64,
    /// Tokens used in code generation phase
    pub generation_tokens: u64,
    /// Tokens used in execution phase (should be 0 for script execution)
    pub execution_tokens: u64,
    /// Total tokens used
    pub total_tokens: u64,
    /// Items processed
    pub items_processed: usize,
    /// Items failed
    pub items_failed: usize,
    /// Whether script was reused
    pub script_reused: bool,
    /// Estimated tokens if pure CV was used
    pub pure_cv_estimated_tokens: u64,
    /// Actual savings percentage
    pub savings_percent: f64,
}

impl ExecutionStats {
    /// Calculate savings
    pub fn calculate_savings(&mut self) {
        if self.pure_cv_estimated_tokens > 0 {
            let saved = self
                .pure_cv_estimated_tokens
                .saturating_sub(self.total_tokens);
            self.savings_percent = (saved as f64 / self.pure_cv_estimated_tokens as f64) * 100.0;
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
    fn test_page_schema_creation() {
        let schema = PageSchema::new("https://example.com", "Example Page")
            .with_element(
                ElementSchema::new("btn-1", ElementType::Button, "Submit button")
                    .with_coordinates(100, 200),
            )
            .with_action(ActionSchema::new(
                "click_submit",
                "btn-1",
                "Click the submit button",
            ));

        assert_eq!(schema.url, "https://example.com");
        assert_eq!(schema.elements.len(), 1);
        assert_eq!(schema.actions.len(), 1);
        assert!(!schema.schema_hash.is_empty());
    }

    #[test]
    fn test_element_schema() {
        let element = ElementSchema::new("input-1", ElementType::Input, "Name field")
            .with_selector("#name-input")
            .with_coordinates(150, 300)
            .with_text("John Doe");

        assert_eq!(element.id, "input-1");
        assert_eq!(element.element_type, ElementType::Input);
        assert_eq!(element.selector, Some("#name-input".to_string()));
        assert_eq!(element.coordinates, Some(Coordinates { x: 150, y: 300 }));
    }

    #[test]
    fn test_generated_script() {
        let script = GeneratedScript::new(
            ScriptType::Playwright,
            "const page = await browser.newPage();",
            "javascript",
            "abc123",
            "fill-form-task",
        );

        assert_eq!(script.script_type, ScriptType::Playwright);
        assert!(!script.id.is_empty());
        assert_eq!(script.task_signature, "fill-form-task");
    }

    #[test]
    fn test_script_asset_recording() {
        let script =
            GeneratedScript::new(ScriptType::Playwright, "code", "javascript", "hash", "sig");

        let mut asset = ScriptAsset::from_script(&script);
        assert_eq!(asset.use_count, 0);
        assert_eq!(asset.success_rate, 1.0);

        asset.record_usage(true);
        asset.record_usage(true);
        asset.record_usage(false);

        assert_eq!(asset.use_count, 3);
        assert_eq!(asset.total_executions, 3);
        assert_eq!(asset.successful_executions, 2);
        assert!((asset.success_rate - 0.666).abs() < 0.01);
    }

    #[test]
    fn test_automation_path_tokens() {
        assert_eq!(
            AutomationPath::DirectApi {
                api_type: "rest".to_string(),
                api_endpoint: None
            }
            .estimated_tokens(),
            500
        );

        assert_eq!(
            AutomationPath::ExploreAndGenerate {
                target_url: "https://example.com".to_string(),
                estimated_tokens: 6000,
            }
            .estimated_tokens(),
            6000
        );

        assert_eq!(
            AutomationPath::ReuseScript {
                script_id: "id".to_string(),
                last_success_rate: 0.95,
            }
            .estimated_tokens(),
            500
        );
    }

    #[test]
    fn test_automation_request() {
        let request = AutomationRequest::new("Fill Google Sheets with data")
            .with_url("https://sheets.google.com")
            .with_data(vec![
                serde_json::json!({"name": "Alice"}),
                serde_json::json!({"name": "Bob"}),
            ]);

        assert_eq!(request.task, "Fill Google Sheets with data");
        assert_eq!(request.data_volume(), 2);
    }

    #[test]
    fn test_execution_stats_savings() {
        let mut stats = ExecutionStats {
            total_tokens: 6000,
            pure_cv_estimated_tokens: 4_100_000,
            ..Default::default()
        };

        stats.calculate_savings();
        assert!(stats.savings_percent > 99.0);
    }

    #[test]
    fn test_bounding_box_center() {
        let bbox = BoundingBox {
            x: 100,
            y: 200,
            width: 50,
            height: 30,
        };

        let center = bbox.center();
        assert_eq!(center.x, 125);
        assert_eq!(center.y, 215);
    }
}
