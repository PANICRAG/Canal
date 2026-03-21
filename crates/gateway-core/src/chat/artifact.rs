//! Artifact types and definitions
//!
//! Artifacts are rich content objects that can be generated during chat interactions.
//! They include documents, code blocks, charts, tables, images, and timelines.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Artifact type enumeration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    /// Markdown or text documents
    Document,
    /// Code snippets with language specification
    #[serde(alias = "code")]
    CodeBlock,
    /// Data visualizations (charts)
    Chart,
    /// Tabular data
    Table,
    /// Generated or referenced images
    Image,
    /// Event timelines
    Timeline,
    /// Image gallery (multiple images)
    ImageGallery,
    /// Video preview
    VideoPreview,
    /// Publish confirmation
    PublishConfirm,
    /// Analytics report
    #[serde(alias = "analysis_report")]
    AnalyticsReport,
    /// Approval request
    ApprovalRequest,
    /// Generic JSON data
    Json,
}

impl std::fmt::Display for ArtifactType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArtifactType::Document => write!(f, "document"),
            ArtifactType::CodeBlock => write!(f, "code_block"),
            ArtifactType::Chart => write!(f, "chart"),
            ArtifactType::Table => write!(f, "table"),
            ArtifactType::Image => write!(f, "image"),
            ArtifactType::Timeline => write!(f, "timeline"),
            ArtifactType::ImageGallery => write!(f, "image_gallery"),
            ArtifactType::VideoPreview => write!(f, "video_preview"),
            ArtifactType::PublishConfirm => write!(f, "publish_confirm"),
            ArtifactType::AnalyticsReport => write!(f, "analytics_report"),
            ArtifactType::ApprovalRequest => write!(f, "approval_request"),
            ArtifactType::Json => write!(f, "json"),
        }
    }
}

/// Artifact content variants
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArtifactContent {
    /// Document content (markdown, html, plain text)
    Document(DocumentContent),
    /// Code block content
    CodeBlock(CodeBlockContent),
    /// Chart content
    Chart(ChartContent),
    /// Table content
    Table(TableContent),
    /// Image content
    Image(ImageContent),
    /// Timeline content
    Timeline(TimelineContent),
    /// Image gallery content
    ImageGallery(ImageGalleryContent),
    /// Video preview content
    VideoPreview(VideoPreviewContent),
    /// Publish confirmation content
    PublishConfirm(PublishConfirmContent),
    /// Analytics report content
    AnalyticsReport(AnalyticsReportContent),
    /// Approval request content
    ApprovalRequest(ApprovalRequestContent),
}

/// Document content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentContent {
    /// Document format
    pub format: DocumentFormat,
    /// Document content
    pub content: String,
    /// Document sections (optional)
    #[serde(default)]
    pub sections: Vec<DocumentSection>,
}

/// Document format
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFormat {
    Markdown,
    Html,
    PlainText,
}

/// Document section
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSection {
    pub id: String,
    pub title: String,
    pub content: String,
    pub level: u8,
}

/// Code block content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeBlockContent {
    /// Programming language
    pub language: String,
    /// Code content
    pub code: String,
    /// Optional filename
    #[serde(default)]
    pub filename: Option<String>,
    /// Code highlights
    #[serde(default)]
    pub highlights: Vec<CodeHighlight>,
}

/// Code highlight
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeHighlight {
    pub start_line: u32,
    pub end_line: u32,
    pub color: String,
    #[serde(default)]
    pub message: Option<String>,
}

/// Chart content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartContent {
    /// Chart type
    pub chart_type: ChartType,
    /// Chart data
    pub data: serde_json::Value,
    /// Chart options
    pub options: ChartOptions,
}

/// Chart type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ChartType {
    Line,
    Bar,
    Pie,
    Donut,
    Area,
    Scatter,
    Radar,
}

/// Chart options
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChartOptions {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub x_axis_label: Option<String>,
    #[serde(default)]
    pub y_axis_label: Option<String>,
    #[serde(default)]
    pub show_legend: bool,
    #[serde(default)]
    pub colors: Option<Vec<String>>,
}

/// Table content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableContent {
    /// Table columns
    pub columns: Vec<TableColumn>,
    /// Table rows
    pub rows: Vec<serde_json::Value>,
    /// Whether the table is sortable
    #[serde(default)]
    pub sortable: bool,
    /// Whether the table is filterable
    #[serde(default)]
    pub filterable: bool,
}

/// Table column definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableColumn {
    pub key: String,
    pub label: String,
    pub data_type: ColumnDataType,
    #[serde(default)]
    pub width: Option<String>,
}

/// Column data type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ColumnDataType {
    String,
    Number,
    Date,
    Currency,
    Percentage,
    Link,
    Badge,
}

/// Image content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageContent {
    /// Image URL or base64 data
    pub url: String,
    /// Alternative text
    pub alt: String,
    /// Caption
    #[serde(default)]
    pub caption: Option<String>,
    /// Image width
    #[serde(default)]
    pub width: Option<u32>,
    /// Image height
    #[serde(default)]
    pub height: Option<u32>,
}

/// Timeline content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineContent {
    /// Timeline events
    pub events: Vec<TimelineEvent>,
    /// Timeline orientation
    #[serde(default = "default_timeline_orientation")]
    pub orientation: TimelineOrientation,
}

fn default_timeline_orientation() -> TimelineOrientation {
    TimelineOrientation::Vertical
}

/// Timeline event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub timestamp: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// Timeline orientation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TimelineOrientation {
    Vertical,
    Horizontal,
}

/// Image gallery content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGalleryContent {
    /// Gallery images
    pub images: Vec<GalleryImage>,
    /// Gallery layout
    #[serde(default = "default_gallery_layout")]
    pub layout: GalleryLayout,
}

fn default_gallery_layout() -> GalleryLayout {
    GalleryLayout::Grid
}

/// Gallery image
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GalleryImage {
    pub url: String,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    pub alt: String,
    #[serde(default)]
    pub caption: Option<String>,
}

/// Gallery layout
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GalleryLayout {
    Grid,
    Masonry,
    Carousel,
}

/// Video preview content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoPreviewContent {
    pub idea_id: String,
    pub title: String,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    pub script: VideoScript,
    pub duration: u32,
    pub status: VideoStatus,
}

/// Video script
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoScript {
    pub hook: String,
    pub body: String,
    pub call_to_action: String,
}

/// Video status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum VideoStatus {
    Draft,
    ReadyToPublish,
    Publishing,
    Published,
    Failed,
}

/// Publish confirmation content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishConfirmContent {
    pub platform: String,
    pub platform_icon: String,
    pub published_url: String,
    pub published_at: String,
    pub metrics: PublishMetrics,
}

/// Publish metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishMetrics {
    #[serde(default)]
    pub initial_views: Option<u64>,
    pub platform_id: String,
}

/// Analytics report content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsReportContent {
    pub period: String,
    pub summary: AnalyticsSummary,
    pub charts: Vec<AnalyticsChart>,
    pub insights: Vec<String>,
}

/// Analytics summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsSummary {
    pub total_views: u64,
    pub total_likes: u64,
    pub total_comments: u64,
    pub total_shares: u64,
    pub avg_watch_time: f64,
    pub engagement_rate: f64,
}

/// Analytics chart
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsChart {
    pub title: String,
    pub chart_type: ChartType,
    pub data: serde_json::Value,
}

/// Approval request content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequestContent {
    pub action: String,
    pub description: String,
    pub details: serde_json::Value,
    #[serde(default)]
    pub deadline: Option<String>,
    #[serde(default)]
    pub approved: Option<bool>,
}

/// Artifact metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArtifactMetadata {
    /// Source tool that created the artifact
    #[serde(default)]
    pub source_tool: Option<String>,
    /// Source MCP server
    #[serde(default)]
    pub source_mcp_server: Option<String>,
    /// Version number
    #[serde(default = "default_version")]
    pub version: u32,
    /// Whether the artifact is editable
    #[serde(default)]
    pub is_editable: bool,
    /// Whether the artifact is downloadable
    #[serde(default = "default_true")]
    pub is_downloadable: bool,
    /// Whether the artifact is shareable
    #[serde(default)]
    pub is_shareable: bool,
    /// Custom metadata
    #[serde(default)]
    pub custom: HashMap<String, serde_json::Value>,
}

fn default_version() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

/// Artifact action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactActionDef {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub action_type: ArtifactActionType,
    #[serde(default)]
    pub requires_confirmation: bool,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Artifact action type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactActionType {
    Edit,
    Download,
    Share,
    Approve,
    Reject,
    Retry,
    Delete,
    OpenExternal,
    Custom,
}

/// Full artifact structure with storage metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredArtifact {
    /// Unique artifact ID
    pub id: Uuid,
    /// Session ID this artifact belongs to
    #[serde(default)]
    pub session_id: Option<Uuid>,
    /// Message ID that generated this artifact
    #[serde(default)]
    pub message_id: Option<Uuid>,
    /// Artifact type
    pub artifact_type: ArtifactType,
    /// Artifact title
    pub title: String,
    /// Artifact content
    pub content: ArtifactContent,
    /// Artifact metadata
    #[serde(default)]
    pub metadata: ArtifactMetadata,
    /// Available actions
    #[serde(default)]
    pub actions: Vec<ArtifactActionDef>,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Last update timestamp
    pub updated_at: DateTime<Utc>,
}

impl StoredArtifact {
    /// Create a new stored artifact
    pub fn new(
        artifact_type: ArtifactType,
        title: impl Into<String>,
        content: ArtifactContent,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            session_id: None,
            message_id: None,
            artifact_type,
            title: title.into(),
            content,
            metadata: ArtifactMetadata::default(),
            actions: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Create a document artifact
    pub fn document(title: impl Into<String>, content: impl Into<String>) -> Self {
        let content = ArtifactContent::Document(DocumentContent {
            format: DocumentFormat::Markdown,
            content: content.into(),
            sections: Vec::new(),
        });
        let mut artifact = Self::new(ArtifactType::Document, title, content);
        artifact.metadata.is_editable = true;
        artifact.actions = vec![
            ArtifactActionDef {
                id: "edit".into(),
                label: "Edit".into(),
                icon: Some("pencil".into()),
                action_type: ArtifactActionType::Edit,
                requires_confirmation: false,
                payload: serde_json::json!({}),
            },
            ArtifactActionDef {
                id: "download".into(),
                label: "Download".into(),
                icon: Some("download".into()),
                action_type: ArtifactActionType::Download,
                requires_confirmation: false,
                payload: serde_json::json!({}),
            },
        ];
        artifact
    }

    /// Create a code block artifact
    pub fn code_block(
        title: impl Into<String>,
        language: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        let content = ArtifactContent::CodeBlock(CodeBlockContent {
            language: language.into(),
            code: code.into(),
            filename: None,
            highlights: Vec::new(),
        });
        let mut artifact = Self::new(ArtifactType::CodeBlock, title, content);
        artifact.metadata.is_editable = true;
        artifact.actions = vec![
            ArtifactActionDef {
                id: "edit".into(),
                label: "Edit".into(),
                icon: Some("pencil".into()),
                action_type: ArtifactActionType::Edit,
                requires_confirmation: false,
                payload: serde_json::json!({}),
            },
            ArtifactActionDef {
                id: "download".into(),
                label: "Download".into(),
                icon: Some("download".into()),
                action_type: ArtifactActionType::Download,
                requires_confirmation: false,
                payload: serde_json::json!({}),
            },
        ];
        artifact
    }

    /// Create a chart artifact
    pub fn chart(title: impl Into<String>, chart_type: ChartType, data: serde_json::Value) -> Self {
        let content = ArtifactContent::Chart(ChartContent {
            chart_type,
            data,
            options: ChartOptions::default(),
        });
        Self::new(ArtifactType::Chart, title, content)
    }

    /// Create a table artifact
    pub fn table(
        title: impl Into<String>,
        columns: Vec<TableColumn>,
        rows: Vec<serde_json::Value>,
    ) -> Self {
        let content = ArtifactContent::Table(TableContent {
            columns,
            rows,
            sortable: true,
            filterable: true,
        });
        Self::new(ArtifactType::Table, title, content)
    }

    /// Create an image artifact
    pub fn image(title: impl Into<String>, url: impl Into<String>, alt: impl Into<String>) -> Self {
        let content = ArtifactContent::Image(ImageContent {
            url: url.into(),
            alt: alt.into(),
            caption: None,
            width: None,
            height: None,
        });
        Self::new(ArtifactType::Image, title, content)
    }

    /// Create a timeline artifact
    pub fn timeline(title: impl Into<String>, events: Vec<TimelineEvent>) -> Self {
        let content = ArtifactContent::Timeline(TimelineContent {
            events,
            orientation: TimelineOrientation::Vertical,
        });
        Self::new(ArtifactType::Timeline, title, content)
    }

    /// Set session ID
    pub fn with_session_id(mut self, session_id: Uuid) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Set message ID
    pub fn with_message_id(mut self, message_id: Uuid) -> Self {
        self.message_id = Some(message_id);
        self
    }

    /// Set metadata
    pub fn with_metadata(mut self, metadata: ArtifactMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Add action
    pub fn with_action(mut self, action: ArtifactActionDef) -> Self {
        self.actions.push(action);
        self
    }

    /// Mark as updated
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_document_artifact() {
        let artifact = StoredArtifact::document("Test Doc", "# Hello\n\nWorld");
        assert_eq!(artifact.artifact_type, ArtifactType::Document);
        assert_eq!(artifact.title, "Test Doc");
        assert!(artifact.metadata.is_editable);
        assert_eq!(artifact.actions.len(), 2);
    }

    #[test]
    fn test_create_code_block_artifact() {
        let artifact = StoredArtifact::code_block("Test Code", "rust", "fn main() {}");
        assert_eq!(artifact.artifact_type, ArtifactType::CodeBlock);

        if let ArtifactContent::CodeBlock(content) = &artifact.content {
            assert_eq!(content.language, "rust");
            assert_eq!(content.code, "fn main() {}");
        } else {
            panic!("Expected CodeBlock content");
        }
    }

    #[test]
    fn test_artifact_with_session() {
        let session_id = Uuid::new_v4();
        let message_id = Uuid::new_v4();

        let artifact = StoredArtifact::document("Test", "Content")
            .with_session_id(session_id)
            .with_message_id(message_id);

        assert_eq!(artifact.session_id, Some(session_id));
        assert_eq!(artifact.message_id, Some(message_id));
    }

    #[test]
    fn test_create_chart_artifact() {
        let data = serde_json::json!({
            "labels": ["A", "B", "C"],
            "values": [10, 20, 30]
        });
        let artifact = StoredArtifact::chart("Test Chart", ChartType::Bar, data);
        assert_eq!(artifact.artifact_type, ArtifactType::Chart);
    }

    #[test]
    fn test_create_table_artifact() {
        let columns = vec![TableColumn {
            key: "name".into(),
            label: "Name".into(),
            data_type: ColumnDataType::String,
            width: None,
        }];
        let rows = vec![serde_json::json!({"name": "Alice"})];
        let artifact = StoredArtifact::table("Test Table", columns, rows);
        assert_eq!(artifact.artifact_type, ArtifactType::Table);
    }

    #[test]
    fn test_artifact_type_display() {
        assert_eq!(ArtifactType::Document.to_string(), "document");
        assert_eq!(ArtifactType::CodeBlock.to_string(), "code_block");
        assert_eq!(ArtifactType::Chart.to_string(), "chart");
    }

    #[test]
    fn test_artifact_serialization() {
        let artifact = StoredArtifact::document("Test", "Content");
        let json = serde_json::to_string(&artifact).unwrap();
        let deserialized: StoredArtifact = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.title, "Test");
        assert_eq!(deserialized.artifact_type, ArtifactType::Document);
    }
}
