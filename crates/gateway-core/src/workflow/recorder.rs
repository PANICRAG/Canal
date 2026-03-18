//! Workflow Recording and Template Learning
//!
//! This module provides capabilities to record user workflows and learn
//! reusable templates from them, supporting the Manus-style workflow automation.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::engine::{StepType, WorkflowDefinition, WorkflowStep};

// ============================================================================
// Recording Session
// ============================================================================

/// A workflow recording session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSession {
    /// Unique session ID
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Description
    pub description: Option<String>,
    /// Target application (e.g., "davinci_resolve", "premiere")
    pub application: Option<String>,
    /// Recording status
    pub status: RecordingStatus,
    /// Recorded actions
    pub actions: Vec<RecordedAction>,
    /// Session metadata
    pub metadata: RecordingMetadata,
    /// Started at
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Ended at
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Recording session status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingStatus {
    /// Session is active and recording
    Recording,
    /// Session is paused
    Paused,
    /// Session is stopped and ready for analysis
    Stopped,
    /// Session has been analyzed and template created
    Analyzed,
}

/// Metadata about the recording session
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecordingMetadata {
    /// User ID who created the recording
    pub user_id: Option<String>,
    /// Total duration in seconds
    pub duration_secs: u64,
    /// Number of tool calls recorded
    pub action_count: usize,
    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
    /// Custom metadata
    #[serde(default)]
    pub custom: HashMap<String, serde_json::Value>,
}

// ============================================================================
// Recorded Action
// ============================================================================

/// A single recorded action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedAction {
    /// Action ID
    pub id: String,
    /// Timestamp relative to session start (ms)
    pub timestamp_ms: u64,
    /// Tool/action name
    pub tool_name: String,
    /// Input parameters
    pub input: serde_json::Value,
    /// Output result
    pub output: Option<serde_json::Value>,
    /// Action type classification
    pub action_type: ActionType,
    /// Whether this action was successful
    pub success: bool,
    /// Duration of this action in ms
    pub duration_ms: Option<u64>,
    /// Detected patterns in this action
    #[serde(default)]
    pub patterns: Vec<DetectedPattern>,
}

/// Type of action for pattern recognition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    /// Read/query operation
    Read,
    /// Write/modify operation
    Write,
    /// Navigation/selection
    Navigate,
    /// Configuration change
    Configure,
    /// Render/export operation
    Export,
    /// Unknown action type
    Unknown,
}

impl ActionType {
    /// Infer action type from tool name
    pub fn from_tool_name(tool_name: &str) -> Self {
        let name_lower = tool_name.to_lowercase();

        if name_lower.contains("read") || name_lower.contains("get") || name_lower.contains("list")
        {
            Self::Read
        } else if name_lower.contains("write")
            || name_lower.contains("edit")
            || name_lower.contains("set")
        {
            Self::Write
        } else if name_lower.contains("navigate")
            || name_lower.contains("select")
            || name_lower.contains("open")
        {
            Self::Navigate
        } else if name_lower.contains("config")
            || name_lower.contains("setting")
            || name_lower.contains("option")
        {
            Self::Configure
        } else if name_lower.contains("render")
            || name_lower.contains("export")
            || name_lower.contains("save")
        {
            Self::Export
        } else {
            Self::Unknown
        }
    }
}

/// A detected pattern in an action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedPattern {
    /// Pattern type
    pub pattern_type: PatternType,
    /// Pattern key (what was detected)
    pub key: String,
    /// Pattern value (the detected value)
    pub value: serde_json::Value,
    /// Whether this can be parameterized
    pub parameterizable: bool,
    /// Suggested parameter name
    pub suggested_param: Option<String>,
}

/// Type of detected pattern
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternType {
    /// File path pattern
    FilePath,
    /// Numeric value pattern
    NumericValue,
    /// Text content pattern
    TextContent,
    /// Time/duration pattern
    Duration,
    /// Color value pattern
    Color,
    /// Repeated sequence pattern
    Sequence,
}

// ============================================================================
// Workflow Template
// ============================================================================

/// A reusable workflow template generated from recordings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplate {
    /// Unique template ID
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Natural language description (for LLM matching)
    pub description: String,
    /// Source recording session ID
    pub source_recording_id: Option<String>,
    /// Parameterizable steps
    pub steps: Vec<TemplateStep>,
    /// Template parameters
    pub parameters: Vec<TemplateParameter>,
    /// Recommended conditions for using this template
    #[serde(default)]
    pub recommended_conditions: Vec<String>,
    /// Conditions where this template should not be used
    #[serde(default)]
    pub not_recommended_conditions: Vec<String>,
    /// Success rate from past executions
    pub success_rate: f32,
    /// Number of times executed
    pub execution_count: u64,
    /// Average execution time in seconds
    pub avg_duration_secs: f64,
    /// Created at
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last used at
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A step in a workflow template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateStep {
    /// Step ID
    pub id: String,
    /// Step name
    pub name: String,
    /// Tool to call
    pub tool: String,
    /// Parameter template with placeholders
    /// e.g., {"path": "{{input_file}}", "level": "{{volume_db}}"}
    pub params: serde_json::Value,
    /// Conditions for this step
    #[serde(default)]
    pub conditions: Vec<StepCondition>,
    /// Expected outcomes
    #[serde(default)]
    pub expected_outcomes: Vec<ExpectedOutcome>,
    /// Dependencies on other steps
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Whether this step is optional
    #[serde(default)]
    pub optional: bool,
}

/// Condition for executing a template step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepCondition {
    /// Condition type
    pub condition_type: ConditionType,
    /// Field to check
    pub field: String,
    /// Expected value or pattern
    pub value: serde_json::Value,
}

/// Type of step condition
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionType {
    /// Value equals expected
    Equals,
    /// Value does not equal expected
    NotEquals,
    /// Value contains expected (for strings/arrays)
    Contains,
    /// Value is greater than expected
    GreaterThan,
    /// Value is less than expected
    LessThan,
    /// Value exists (not null)
    Exists,
    /// Previous step succeeded
    PreviousSuccess,
}

/// Expected outcome from a step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedOutcome {
    /// Outcome type
    pub outcome_type: OutcomeType,
    /// Description
    pub description: String,
    /// Field to check in output
    pub field: Option<String>,
    /// Expected value pattern
    pub expected: Option<serde_json::Value>,
}

/// Type of expected outcome
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeType {
    /// Step should succeed
    Success,
    /// Specific field should have value
    FieldValue,
    /// File should be created/modified
    FileChange,
    /// State should change
    StateChange,
}

/// A parameter that can be customized when running the template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateParameter {
    /// Parameter name (used in template placeholders)
    pub name: String,
    /// Human-readable label
    pub label: String,
    /// Description
    pub description: Option<String>,
    /// Parameter type
    pub param_type: ParameterType,
    /// Default value
    pub default: Option<serde_json::Value>,
    /// Whether this parameter is required
    #[serde(default)]
    pub required: bool,
    /// Validation constraints
    #[serde(default)]
    pub validation: Option<ParameterValidation>,
}

/// Type of template parameter
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParameterType {
    /// String value
    String,
    /// Integer value
    Integer { min: Option<i64>, max: Option<i64> },
    /// Float value
    Float { min: Option<f64>, max: Option<f64> },
    /// Boolean value
    Boolean,
    /// File path
    FilePath { extensions: Option<Vec<String>> },
    /// Selection from options
    Select { options: Vec<String> },
    /// Duration in seconds
    Duration,
    /// Color value
    Color,
}

/// Validation rules for a parameter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterValidation {
    /// Regex pattern for validation
    pub pattern: Option<String>,
    /// Minimum length (for strings)
    pub min_length: Option<usize>,
    /// Maximum length (for strings)
    pub max_length: Option<usize>,
    /// Custom validation message
    pub message: Option<String>,
}

// ============================================================================
// Workflow Recorder
// ============================================================================

/// Records user actions and generates workflow templates
pub struct WorkflowRecorder {
    /// Current active recording session
    session: Arc<RwLock<Option<RecordingSession>>>,
    /// Pattern recognition engine
    pattern_engine: PatternEngine,
    /// Saved templates
    templates: Arc<RwLock<HashMap<String, WorkflowTemplate>>>,
}

impl Default for WorkflowRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkflowRecorder {
    /// Create a new workflow recorder
    pub fn new() -> Self {
        Self {
            session: Arc::new(RwLock::new(None)),
            pattern_engine: PatternEngine::new(),
            templates: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start a new recording session
    pub async fn start_recording(&self, name: String, application: Option<String>) -> String {
        let session = RecordingSession {
            id: format!("rec_{}", Uuid::new_v4().to_string().replace("-", "")),
            name,
            description: None,
            application,
            status: RecordingStatus::Recording,
            actions: Vec::new(),
            metadata: RecordingMetadata::default(),
            started_at: chrono::Utc::now(),
            ended_at: None,
        };

        let id = session.id.clone();
        *self.session.write().await = Some(session);

        tracing::info!(recording_id = %id, "Started workflow recording");
        id
    }

    /// Record an action
    pub async fn record_action(
        &self,
        tool_name: String,
        input: serde_json::Value,
        output: Option<serde_json::Value>,
        success: bool,
        duration_ms: Option<u64>,
    ) -> Option<String> {
        let mut session_guard = self.session.write().await;
        let session = session_guard.as_mut()?;

        if session.status != RecordingStatus::Recording {
            return None;
        }

        let timestamp_ms = (chrono::Utc::now() - session.started_at).num_milliseconds() as u64;
        let action_type = ActionType::from_tool_name(&tool_name);

        // Detect patterns in the input
        let patterns = self.pattern_engine.detect_patterns(&input);

        let action = RecordedAction {
            id: format!(
                "act_{}",
                Uuid::new_v4().to_string().replace("-", "")[..8].to_string()
            ),
            timestamp_ms,
            tool_name,
            input,
            output,
            action_type,
            success,
            duration_ms,
            patterns,
        };

        let action_id = action.id.clone();
        session.actions.push(action);
        session.metadata.action_count = session.actions.len();

        Some(action_id)
    }

    /// Pause the recording
    pub async fn pause_recording(&self) -> bool {
        let mut session_guard = self.session.write().await;
        if let Some(session) = session_guard.as_mut() {
            if session.status == RecordingStatus::Recording {
                session.status = RecordingStatus::Paused;
                return true;
            }
        }
        false
    }

    /// Resume the recording
    pub async fn resume_recording(&self) -> bool {
        let mut session_guard = self.session.write().await;
        if let Some(session) = session_guard.as_mut() {
            if session.status == RecordingStatus::Paused {
                session.status = RecordingStatus::Recording;
                return true;
            }
        }
        false
    }

    /// Stop the recording and return the session
    pub async fn stop_recording(&self) -> Option<RecordingSession> {
        let mut session_guard = self.session.write().await;
        if let Some(session) = session_guard.as_mut() {
            session.status = RecordingStatus::Stopped;
            session.ended_at = Some(chrono::Utc::now());
            session.metadata.duration_secs =
                (chrono::Utc::now() - session.started_at).num_seconds() as u64;
        }
        session_guard.take()
    }

    /// Get the current recording session
    pub async fn get_session(&self) -> Option<RecordingSession> {
        self.session.read().await.clone()
    }

    /// Analyze a recording session and generate a workflow template
    pub async fn analyze_and_create_template(
        &self,
        session: &RecordingSession,
    ) -> WorkflowTemplate {
        // Extract steps from recorded actions
        let steps: Vec<TemplateStep> = session
            .actions
            .iter()
            .enumerate()
            .filter(|(_, action)| action.success) // Only include successful actions
            .map(|(idx, action)| {
                let params = self.pattern_engine.parameterize_input(&action.input, &action.patterns);

                TemplateStep {
                    id: format!("step_{}", idx + 1),
                    name: action.tool_name.clone(),
                    tool: action.tool_name.clone(),
                    params,
                    conditions: Vec::new(),
                    expected_outcomes: vec![ExpectedOutcome {
                        outcome_type: OutcomeType::Success,
                        description: "Step completes successfully".to_string(),
                        field: None,
                        expected: None,
                    }],
                    depends_on: if idx > 0 {
                        vec![format!("step_{}", idx)]
                    } else {
                        Vec::new()
                    },
                    optional: false,
                }
            })
            .collect();

        // Extract parameters from detected patterns
        let parameters = self.extract_parameters(&session.actions);

        // Analyze conditions
        let (recommended, not_recommended) = self.analyze_conditions(session);

        WorkflowTemplate {
            id: format!("wft_{}", Uuid::new_v4().to_string().replace("-", "")),
            name: session.name.clone(),
            description: session.description.clone().unwrap_or_else(|| {
                format!("Workflow recorded from {} actions", session.actions.len())
            }),
            source_recording_id: Some(session.id.clone()),
            steps,
            parameters,
            recommended_conditions: recommended,
            not_recommended_conditions: not_recommended,
            success_rate: 1.0, // Initial value
            execution_count: 0,
            avg_duration_secs: session.metadata.duration_secs as f64,
            created_at: chrono::Utc::now(),
            last_used_at: None,
            tags: session.metadata.tags.clone(),
        }
    }

    /// Extract parameters from recorded actions
    fn extract_parameters(&self, actions: &[RecordedAction]) -> Vec<TemplateParameter> {
        let mut params = HashMap::new();

        for action in actions {
            for pattern in &action.patterns {
                if pattern.parameterizable {
                    let name = pattern
                        .suggested_param
                        .clone()
                        .unwrap_or_else(|| pattern.key.clone());

                    if !params.contains_key(&name) {
                        let param_type = match pattern.pattern_type {
                            PatternType::FilePath => ParameterType::FilePath { extensions: None },
                            PatternType::NumericValue => {
                                if pattern.value.is_i64() {
                                    ParameterType::Integer {
                                        min: None,
                                        max: None,
                                    }
                                } else {
                                    ParameterType::Float {
                                        min: None,
                                        max: None,
                                    }
                                }
                            }
                            PatternType::Duration => ParameterType::Duration,
                            PatternType::Color => ParameterType::Color,
                            _ => ParameterType::String,
                        };

                        params.insert(
                            name.clone(),
                            TemplateParameter {
                                name: name.clone(),
                                label: Self::humanize_name(&name),
                                description: None,
                                param_type,
                                default: Some(pattern.value.clone()),
                                required: false,
                                validation: None,
                            },
                        );
                    }
                }
            }
        }

        params.into_values().collect()
    }

    /// Analyze conditions from recorded session
    fn analyze_conditions(&self, session: &RecordingSession) -> (Vec<String>, Vec<String>) {
        let mut recommended = Vec::new();
        let mut not_recommended = Vec::new();

        // Simple heuristics based on recorded actions
        let action_types: Vec<ActionType> = session.actions.iter().map(|a| a.action_type).collect();

        // Check for configuration-heavy workflows
        let config_count = action_types
            .iter()
            .filter(|t| matches!(t, ActionType::Configure))
            .count();

        if config_count > 2 {
            recommended.push("Requires specific configuration setup".to_string());
        }

        // Check for export workflows
        if action_types.iter().any(|t| matches!(t, ActionType::Export)) {
            not_recommended.push("Not suitable for preview-only tasks".to_string());
        }

        // Add application-specific recommendations
        if let Some(app) = &session.application {
            recommended.push(format!("Best used with {}", app));
        }

        (recommended, not_recommended)
    }

    /// Convert a name to human-readable label
    fn humanize_name(name: &str) -> String {
        name.split('_')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().chain(chars).collect(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Convert a workflow template to a WorkflowDefinition
    pub fn template_to_definition(&self, template: &WorkflowTemplate) -> WorkflowDefinition {
        WorkflowDefinition {
            id: template.id.clone(),
            name: template.name.clone(),
            description: template.description.clone(),
            steps: template
                .steps
                .iter()
                .map(|step| WorkflowStep {
                    id: step.id.clone(),
                    name: step.name.clone(),
                    step_type: StepType::ToolCall,
                    config: serde_json::json!({
                        "tool": step.tool,
                        "arguments": step.params
                    }),
                    depends_on: step.depends_on.clone(),
                })
                .collect(),
        }
    }

    /// Save a template
    pub async fn save_template(&self, template: WorkflowTemplate) {
        let id = template.id.clone();
        self.templates.write().await.insert(id, template);
    }

    /// Get a template by ID
    pub async fn get_template(&self, id: &str) -> Option<WorkflowTemplate> {
        self.templates.read().await.get(id).cloned()
    }

    /// List all templates
    pub async fn list_templates(&self) -> Vec<WorkflowTemplate> {
        self.templates.read().await.values().cloned().collect()
    }

    /// Find templates matching a natural language query
    pub async fn find_matching_templates(&self, query: &str) -> Vec<WorkflowTemplate> {
        let query_lower = query.to_lowercase();
        let templates = self.templates.read().await;

        templates
            .values()
            .filter(|t| {
                t.name.to_lowercase().contains(&query_lower)
                    || t.description.to_lowercase().contains(&query_lower)
                    || t.tags
                        .iter()
                        .any(|tag| tag.to_lowercase().contains(&query_lower))
            })
            .cloned()
            .collect()
    }

    /// Update template execution statistics
    pub async fn update_template_stats(
        &self,
        template_id: &str,
        success: bool,
        duration_secs: f64,
    ) {
        let mut templates = self.templates.write().await;
        if let Some(template) = templates.get_mut(template_id) {
            template.execution_count += 1;

            // R2-M: Use incremental mean formula to avoid f32→u64 truncation precision drift
            let n = template.execution_count as f32;
            let success_val = if success { 1.0_f32 } else { 0.0_f32 };
            template.success_rate += (success_val - template.success_rate) / n;

            // Update average duration
            template.avg_duration_secs = (template.avg_duration_secs
                * (template.execution_count - 1) as f64
                + duration_secs)
                / template.execution_count as f64;

            template.last_used_at = Some(chrono::Utc::now());
        }
    }
}

// ============================================================================
// Pattern Engine
// ============================================================================

/// Engine for detecting patterns in recorded actions
#[derive(Debug, Clone)]
pub struct PatternEngine {
    /// Keywords that indicate duration values
    duration_keywords: Vec<&'static str>,
}

impl Default for PatternEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl PatternEngine {
    /// Create a new pattern engine
    pub fn new() -> Self {
        Self {
            duration_keywords: vec!["duration", "time", "timeout", "delay", "wait"],
        }
    }

    /// Check if a string looks like a file path
    fn is_file_path(s: &str) -> bool {
        // Unix absolute path
        if s.starts_with('/') {
            return !s.contains('\0') && !s.contains('<') && !s.contains('>');
        }
        // Windows absolute path (e.g., C:\ or C:/)
        if s.len() >= 3 {
            let bytes = s.as_bytes();
            if bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && (bytes[2] == b'\\' || bytes[2] == b'/')
            {
                return !s.contains('\0') && !s.contains('<') && !s.contains('>');
            }
        }
        // Home directory path
        if s.starts_with("~/") {
            return !s.contains('\0') && !s.contains('<') && !s.contains('>');
        }
        false
    }

    /// Check if a string looks like a hex color
    fn is_hex_color(s: &str) -> bool {
        if !s.starts_with('#') || s.len() != 7 {
            return false;
        }
        s[1..].chars().all(|c| c.is_ascii_hexdigit())
    }

    /// Check if a string looks like an RGB color
    fn is_rgb_color(s: &str) -> bool {
        if !s.starts_with("rgb(") || !s.ends_with(')') {
            return false;
        }
        let inner = &s[4..s.len() - 1];
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() != 3 {
            return false;
        }
        parts.iter().all(|p| p.trim().parse::<u8>().is_ok())
    }

    /// Detect patterns in input parameters
    pub fn detect_patterns(&self, input: &serde_json::Value) -> Vec<DetectedPattern> {
        let mut patterns = Vec::new();
        self.detect_patterns_recursive(input, "", &mut patterns);
        patterns
    }

    fn detect_patterns_recursive(
        &self,
        value: &serde_json::Value,
        path: &str,
        patterns: &mut Vec<DetectedPattern>,
    ) {
        match value {
            serde_json::Value::String(s) => {
                // Check for file path
                if Self::is_file_path(s) {
                    patterns.push(DetectedPattern {
                        pattern_type: PatternType::FilePath,
                        key: path.to_string(),
                        value: value.clone(),
                        parameterizable: true,
                        suggested_param: Some(self.suggest_param_name(path, "file")),
                    });
                }
                // Check for color
                else if Self::is_hex_color(s) || Self::is_rgb_color(s) {
                    patterns.push(DetectedPattern {
                        pattern_type: PatternType::Color,
                        key: path.to_string(),
                        value: value.clone(),
                        parameterizable: true,
                        suggested_param: Some(self.suggest_param_name(path, "color")),
                    });
                }
            }
            serde_json::Value::Number(_) => {
                // Check if this looks like a duration
                let is_duration = self
                    .duration_keywords
                    .iter()
                    .any(|kw| path.to_lowercase().contains(kw));

                if is_duration {
                    patterns.push(DetectedPattern {
                        pattern_type: PatternType::Duration,
                        key: path.to_string(),
                        value: value.clone(),
                        parameterizable: true,
                        suggested_param: Some(self.suggest_param_name(path, "duration")),
                    });
                } else {
                    patterns.push(DetectedPattern {
                        pattern_type: PatternType::NumericValue,
                        key: path.to_string(),
                        value: value.clone(),
                        parameterizable: true,
                        suggested_param: Some(self.suggest_param_name(path, "value")),
                    });
                }
            }
            serde_json::Value::Object(map) => {
                for (key, val) in map {
                    let new_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", path, key)
                    };
                    self.detect_patterns_recursive(val, &new_path, patterns);
                }
            }
            serde_json::Value::Array(arr) => {
                for (idx, val) in arr.iter().enumerate() {
                    let new_path = format!("{}[{}]", path, idx);
                    self.detect_patterns_recursive(val, &new_path, patterns);
                }
            }
            _ => {}
        }
    }

    /// Suggest a parameter name based on path and type
    fn suggest_param_name(&self, path: &str, type_hint: &str) -> String {
        let base = path
            .split('.')
            .last()
            .unwrap_or(path)
            .replace('[', "_")
            .replace(']', "")
            .to_lowercase();

        if base.is_empty() {
            type_hint.to_string()
        } else if base.contains(type_hint) {
            base
        } else {
            format!("{}_{}", base, type_hint)
        }
    }

    /// Convert detected patterns to parameterized input
    pub fn parameterize_input(
        &self,
        input: &serde_json::Value,
        patterns: &[DetectedPattern],
    ) -> serde_json::Value {
        let mut result = input.clone();

        for pattern in patterns {
            if pattern.parameterizable {
                if let Some(param_name) = &pattern.suggested_param {
                    let placeholder = format!("{{{{{}}}}}", param_name);
                    Self::set_json_path(&mut result, &pattern.key, serde_json::json!(placeholder));
                }
            }
        }

        result
    }

    /// Set a value at a JSON path
    fn set_json_path(root: &mut serde_json::Value, path: &str, value: serde_json::Value) {
        let parts: Vec<&str> = path.split('.').collect();

        if parts.is_empty() {
            return;
        }

        // Handle single-level path
        if parts.len() == 1 {
            if let Some(obj) = root.as_object_mut() {
                obj.insert(parts[0].to_string(), value);
            }
            return;
        }

        // Navigate to parent and set the final value
        let mut current = root;
        for part in parts.iter().take(parts.len() - 1) {
            if let Some(obj) = current.as_object_mut() {
                current = obj.entry(part.to_string()).or_insert(serde_json::json!({}));
            } else {
                return;
            }
        }

        // Set the final value
        if let Some(obj) = current.as_object_mut() {
            if let Some(last_part) = parts.last() {
                obj.insert(last_part.to_string(), value);
            }
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
    fn test_action_type_from_tool_name() {
        assert_eq!(ActionType::from_tool_name("Read"), ActionType::Read);
        assert_eq!(ActionType::from_tool_name("file_write"), ActionType::Write);
        assert_eq!(
            ActionType::from_tool_name("navigate_to"),
            ActionType::Navigate
        );
        assert_eq!(
            ActionType::from_tool_name("render_output"),
            ActionType::Export
        );
        assert_eq!(
            ActionType::from_tool_name("unknown_tool"),
            ActionType::Unknown
        );
    }

    #[test]
    fn test_pattern_detection_file_path() {
        let engine = PatternEngine::new();
        let input = serde_json::json!({
            "path": "/home/user/project/file.txt"
        });

        let patterns = engine.detect_patterns(&input);
        assert!(!patterns.is_empty());
        assert_eq!(patterns[0].pattern_type, PatternType::FilePath);
    }

    #[test]
    fn test_pattern_detection_color() {
        let engine = PatternEngine::new();
        let input = serde_json::json!({
            "fill_color": "#FF5500"
        });

        let patterns = engine.detect_patterns(&input);
        assert!(!patterns.is_empty());
        assert_eq!(patterns[0].pattern_type, PatternType::Color);
    }

    #[test]
    fn test_pattern_detection_numeric() {
        let engine = PatternEngine::new();
        let input = serde_json::json!({
            "volume": -6.5,
            "duration_ms": 1000
        });

        let patterns = engine.detect_patterns(&input);
        assert_eq!(patterns.len(), 2);

        let duration_pattern = patterns
            .iter()
            .find(|p| p.key.contains("duration"))
            .unwrap();
        assert_eq!(duration_pattern.pattern_type, PatternType::Duration);
    }

    #[test]
    fn test_parameterize_input() {
        let engine = PatternEngine::new();
        let input = serde_json::json!({
            "path": "/home/user/file.txt",
            "volume": -6
        });

        let patterns = engine.detect_patterns(&input);
        let parameterized = engine.parameterize_input(&input, &patterns);

        // Values should be replaced with placeholders
        let path_val = parameterized.get("path").unwrap().as_str().unwrap();
        assert!(path_val.contains("{{"));
    }

    #[tokio::test]
    async fn test_workflow_recorder_session() {
        let recorder = WorkflowRecorder::new();

        // Start recording
        let session_id = recorder
            .start_recording("Test Workflow".to_string(), Some("test_app".to_string()))
            .await;
        assert!(!session_id.is_empty());

        // Record an action
        let action_id = recorder
            .record_action(
                "read_file".to_string(),
                serde_json::json!({"path": "/tmp/test.txt"}),
                Some(serde_json::json!({"content": "test"})),
                true,
                Some(100),
            )
            .await;
        assert!(action_id.is_some());

        // Stop recording
        let session = recorder.stop_recording().await;
        assert!(session.is_some());
        let session = session.unwrap();
        assert_eq!(session.actions.len(), 1);
        assert_eq!(session.status, RecordingStatus::Stopped);
    }

    #[tokio::test]
    async fn test_analyze_and_create_template() {
        let recorder = WorkflowRecorder::new();

        // Create a mock session
        let session = RecordingSession {
            id: "test_session".to_string(),
            name: "Test Workflow".to_string(),
            description: Some("A test workflow".to_string()),
            application: Some("test_app".to_string()),
            status: RecordingStatus::Stopped,
            actions: vec![
                RecordedAction {
                    id: "act_1".to_string(),
                    timestamp_ms: 0,
                    tool_name: "read_file".to_string(),
                    input: serde_json::json!({"path": "/tmp/input.txt"}),
                    output: Some(serde_json::json!({"content": "data"})),
                    action_type: ActionType::Read,
                    success: true,
                    duration_ms: Some(50),
                    patterns: vec![],
                },
                RecordedAction {
                    id: "act_2".to_string(),
                    timestamp_ms: 100,
                    tool_name: "write_file".to_string(),
                    input: serde_json::json!({"path": "/tmp/output.txt", "content": "result"}),
                    output: Some(serde_json::json!({"success": true})),
                    action_type: ActionType::Write,
                    success: true,
                    duration_ms: Some(30),
                    patterns: vec![],
                },
            ],
            metadata: RecordingMetadata {
                user_id: None,
                duration_secs: 10,
                action_count: 2,
                tags: vec!["test".to_string()],
                custom: HashMap::new(),
            },
            started_at: chrono::Utc::now(),
            ended_at: Some(chrono::Utc::now()),
        };

        let template = recorder.analyze_and_create_template(&session).await;

        assert_eq!(template.steps.len(), 2);
        assert!(template
            .recommended_conditions
            .iter()
            .any(|c| c.contains("test_app")));
    }

    #[test]
    fn test_humanize_name() {
        assert_eq!(WorkflowRecorder::humanize_name("input_file"), "Input File");
        assert_eq!(WorkflowRecorder::humanize_name("volume_db"), "Volume Db");
        assert_eq!(WorkflowRecorder::humanize_name("single"), "Single");
    }
}
