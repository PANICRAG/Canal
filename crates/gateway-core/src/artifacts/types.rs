//! Artifact type definitions
//!
//! Defines all artifact types for visual result display in the Canal UI.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Artifact unique identifier
pub type ArtifactId = Uuid;

/// Artifact main structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub session_id: String,
    pub message_id: String,
    pub artifact_type: ArtifactType,
    pub title: String,
    pub content: ArtifactContent,
    pub metadata: ArtifactMetadata,
    pub actions: Vec<ArtifactAction>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Artifact type enumeration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Document,
    VideoPreview,
    Chart,
    Table,
    PublishConfirm,
    AnalyticsReport,
    ApprovalRequest,
    CodeBlock,
    ImageGallery,
    Timeline,
}

/// Artifact content - different structures based on type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArtifactContent {
    /// Document type
    Document {
        format: DocumentFormat,
        content: String,
        sections: Vec<DocumentSection>,
    },

    /// Video preview
    VideoPreview {
        idea_id: String,
        title: String,
        thumbnail_url: Option<String>,
        script: VideoScript,
        duration: u32,
        status: VideoStatus,
    },

    /// Chart
    Chart {
        chart_type: ChartType,
        data: serde_json::Value,
        options: ChartOptions,
    },

    /// Table
    Table {
        columns: Vec<TableColumn>,
        rows: Vec<serde_json::Value>,
        sortable: bool,
        filterable: bool,
    },

    /// Publish confirmation
    PublishConfirm {
        platform: String,
        platform_icon: String,
        published_url: String,
        published_at: DateTime<Utc>,
        metrics: PublishMetrics,
    },

    /// Analytics report
    AnalyticsReport {
        period: String,
        summary: AnalyticsSummary,
        charts: Vec<AnalyticsChart>,
        insights: Vec<String>,
    },

    /// Approval request
    ApprovalRequest {
        action: String,
        description: String,
        details: serde_json::Value,
        deadline: Option<DateTime<Utc>>,
        approved: Option<bool>,
    },

    /// Code block
    CodeBlock {
        language: String,
        code: String,
        filename: Option<String>,
        highlights: Vec<CodeHighlight>,
    },

    /// Image gallery
    ImageGallery {
        images: Vec<GalleryImage>,
        layout: GalleryLayout,
    },

    /// Timeline
    Timeline {
        events: Vec<TimelineEvent>,
        orientation: TimelineOrientation,
    },
}

// ============ Sub-type definitions ============

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFormat {
    Markdown,
    Html,
    PlainText,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSection {
    pub id: String,
    pub title: String,
    pub content: String,
    pub level: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VideoScript {
    pub hook: String,
    pub body: String,
    pub call_to_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VideoStatus {
    #[default]
    Draft,
    ReadyToPublish,
    Publishing,
    Published,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChartOptions {
    pub title: Option<String>,
    pub x_axis_label: Option<String>,
    pub y_axis_label: Option<String>,
    pub show_legend: bool,
    pub colors: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableColumn {
    pub key: String,
    pub label: String,
    pub data_type: TableDataType,
    pub width: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TableDataType {
    String,
    Number,
    Date,
    Currency,
    Percentage,
    Link,
    Badge,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PublishMetrics {
    pub initial_views: Option<u64>,
    pub platform_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnalyticsSummary {
    pub total_views: u64,
    pub total_likes: u64,
    pub total_comments: u64,
    pub total_shares: u64,
    pub avg_watch_time: f64,
    pub engagement_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsChart {
    pub title: String,
    pub chart_type: ChartType,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeHighlight {
    pub start_line: u32,
    pub end_line: u32,
    pub color: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GalleryImage {
    pub url: String,
    pub thumbnail_url: Option<String>,
    pub alt: String,
    pub caption: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GalleryLayout {
    #[default]
    Grid,
    Masonry,
    Carousel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub timestamp: DateTime<Utc>,
    pub title: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimelineOrientation {
    #[default]
    Vertical,
    Horizontal,
}

// ============ Metadata & Actions ============

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    pub source_tool: Option<String>,
    pub source_mcp_server: Option<String>,
    pub version: u32,
    pub is_editable: bool,
    pub is_downloadable: bool,
    pub is_shareable: bool,
    pub custom: serde_json::Value,
}

impl Default for ArtifactMetadata {
    fn default() -> Self {
        Self {
            source_tool: None,
            source_mcp_server: None,
            version: 1,
            is_editable: false,
            is_downloadable: false,
            is_shareable: false,
            custom: serde_json::json!({}),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactAction {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub action_type: ActionType,
    pub requires_confirmation: bool,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artifact_type_serialization() {
        let artifact_type = ArtifactType::VideoPreview;
        let json = serde_json::to_string(&artifact_type).unwrap();
        assert_eq!(json, "\"video_preview\"");

        let parsed: ArtifactType = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ArtifactType::VideoPreview);
    }

    #[test]
    fn test_artifact_content_document() {
        let content = ArtifactContent::Document {
            format: DocumentFormat::Markdown,
            content: "# Hello".to_string(),
            sections: vec![],
        };

        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "document");
        assert_eq!(json["format"], "markdown");
    }

    #[test]
    fn test_artifact_content_video_preview() {
        let content = ArtifactContent::VideoPreview {
            idea_id: "idea-123".to_string(),
            title: "Test Video".to_string(),
            thumbnail_url: Some("https://example.com/thumb.jpg".to_string()),
            script: VideoScript {
                hook: "Hook text".to_string(),
                body: "Body text".to_string(),
                call_to_action: "CTA text".to_string(),
            },
            duration: 60,
            status: VideoStatus::Draft,
        };

        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "video_preview");
        assert_eq!(json["status"], "draft");
    }

    #[test]
    fn test_action_type_serialization() {
        let action_type = ActionType::OpenExternal;
        let json = serde_json::to_string(&action_type).unwrap();
        assert_eq!(json, "\"open_external\"");
    }

    #[test]
    fn test_artifact_metadata_default() {
        let metadata = ArtifactMetadata::default();
        assert_eq!(metadata.version, 1);
        assert!(!metadata.is_editable);
        assert!(metadata.source_tool.is_none());
    }
}
