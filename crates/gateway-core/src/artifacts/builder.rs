//! Artifact Builder
//!
//! Creates artifacts from MCP tool results and LLM responses.

use super::types::*;
use crate::mcp::protocol::ToolContent;
use crate::mcp::ToolCallResult;
use chrono::Utc;
use uuid::Uuid;

/// Artifact builder for creating artifacts from tool results
pub struct ArtifactBuilder {
    session_id: String,
    message_id: String,
}

impl ArtifactBuilder {
    /// Create a new artifact builder
    pub fn new(session_id: String, message_id: String) -> Self {
        Self {
            session_id,
            message_id,
        }
    }

    /// Create an artifact from an MCP tool result
    pub fn from_tool_result(&self, tool_name: &str, result: &ToolCallResult) -> Option<Artifact> {
        // Determine artifact type based on tool name
        match tool_name {
            // VideoCLI tools
            name if name.starts_with("videocli_") => self.build_videocli_artifact(name, result),
            // Filesystem tools
            name if name.starts_with("filesystem_") => self.build_filesystem_artifact(name, result),
            _ => None,
        }
    }

    fn build_videocli_artifact(
        &self,
        tool_name: &str,
        result: &ToolCallResult,
    ) -> Option<Artifact> {
        let data: serde_json::Value = result.content.first().and_then(|c| match c {
            ToolContent::Text { text } => serde_json::from_str(text).ok(),
            _ => None,
        })?;

        match tool_name {
            "videocli_create_idea" | "videocli_get_idea" => Some(self.build_video_preview(&data)),
            "videocli_generate_script" | "videocli_get_script" => {
                Some(self.build_video_preview(&data))
            }
            "videocli_list_ideas" => Some(self.build_ideas_table(&data)),
            "videocli_publish_video" => Some(self.build_publish_confirm(&data)),
            "videocli_get_video_analytics" | "videocli_get_overall_analytics" => {
                Some(self.build_analytics_report(&data))
            }
            _ => None,
        }
    }

    fn build_video_preview(&self, data: &serde_json::Value) -> Artifact {
        let script = data
            .get("script")
            .map(|s| VideoScript {
                hook: s
                    .get("hook")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                body: s
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                call_to_action: s
                    .get("callToAction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
            .unwrap_or_default();

        let status = data
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "draft" => VideoStatus::Draft,
                "scripted" => VideoStatus::ReadyToPublish,
                "published" => VideoStatus::Published,
                _ => VideoStatus::Draft,
            })
            .unwrap_or(VideoStatus::Draft);

        Artifact {
            id: Uuid::new_v4(),
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            artifact_type: ArtifactType::VideoPreview,
            title: data
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Video Preview")
                .to_string(),
            content: ArtifactContent::VideoPreview {
                idea_id: data
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                title: data
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                thumbnail_url: data
                    .get("thumbnailUrl")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                script,
                duration: data.get("duration").and_then(|v| v.as_u64()).unwrap_or(60) as u32,
                status,
            },
            metadata: ArtifactMetadata {
                source_tool: Some("videocli".to_string()),
                source_mcp_server: Some("videocli".to_string()),
                version: 1,
                is_editable: true,
                is_downloadable: false,
                is_shareable: false,
                custom: serde_json::json!({}),
            },
            actions: vec![
                ArtifactAction {
                    id: "edit_script".to_string(),
                    label: "Edit Script".to_string(),
                    icon: Some("edit".to_string()),
                    action_type: ActionType::Edit,
                    requires_confirmation: false,
                    payload: serde_json::json!({}),
                },
                ArtifactAction {
                    id: "publish".to_string(),
                    label: "Publish".to_string(),
                    icon: Some("send".to_string()),
                    action_type: ActionType::Custom,
                    requires_confirmation: true,
                    payload: serde_json::json!({ "action": "publish" }),
                },
            ],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn build_ideas_table(&self, data: &serde_json::Value) -> Artifact {
        let empty_vec = vec![];
        let ideas = data.as_array().unwrap_or(&empty_vec);

        let rows: Vec<serde_json::Value> = ideas
            .iter()
            .map(|idea| {
                serde_json::json!({
                    "id": idea.get("id"),
                    "title": idea.get("title"),
                    "topic": idea.get("topic"),
                    "status": idea.get("status"),
                    "createdAt": idea.get("createdAt"),
                })
            })
            .collect();

        Artifact {
            id: Uuid::new_v4(),
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            artifact_type: ArtifactType::Table,
            title: "Video Ideas".to_string(),
            content: ArtifactContent::Table {
                columns: vec![
                    TableColumn {
                        key: "title".to_string(),
                        label: "Title".to_string(),
                        data_type: TableDataType::String,
                        width: None,
                    },
                    TableColumn {
                        key: "topic".to_string(),
                        label: "Topic".to_string(),
                        data_type: TableDataType::String,
                        width: None,
                    },
                    TableColumn {
                        key: "status".to_string(),
                        label: "Status".to_string(),
                        data_type: TableDataType::Badge,
                        width: Some("100px".to_string()),
                    },
                    TableColumn {
                        key: "createdAt".to_string(),
                        label: "Created".to_string(),
                        data_type: TableDataType::Date,
                        width: Some("150px".to_string()),
                    },
                ],
                rows,
                sortable: true,
                filterable: true,
            },
            metadata: ArtifactMetadata {
                source_tool: Some("videocli_list_ideas".to_string()),
                source_mcp_server: Some("videocli".to_string()),
                version: 1,
                is_editable: false,
                is_downloadable: true,
                is_shareable: false,
                custom: serde_json::json!({}),
            },
            actions: vec![ArtifactAction {
                id: "export_csv".to_string(),
                label: "Export CSV".to_string(),
                icon: Some("download".to_string()),
                action_type: ActionType::Download,
                requires_confirmation: false,
                payload: serde_json::json!({ "format": "csv" }),
            }],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn build_publish_confirm(&self, data: &serde_json::Value) -> Artifact {
        Artifact {
            id: Uuid::new_v4(),
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            artifact_type: ArtifactType::PublishConfirm,
            title: "Published Successfully".to_string(),
            content: ArtifactContent::PublishConfirm {
                platform: data
                    .get("platform")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tiktok")
                    .to_string(),
                platform_icon: "tiktok".to_string(),
                published_url: data
                    .get("publishedUrl")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                published_at: Utc::now(),
                metrics: PublishMetrics {
                    initial_views: None,
                    platform_id: data
                        .get("platformId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                },
            },
            metadata: ArtifactMetadata {
                source_tool: Some("videocli_publish_video".to_string()),
                source_mcp_server: Some("videocli".to_string()),
                version: 1,
                is_editable: false,
                is_downloadable: false,
                is_shareable: true,
                custom: serde_json::json!({}),
            },
            actions: vec![
                ArtifactAction {
                    id: "open_video".to_string(),
                    label: "View on Platform".to_string(),
                    icon: Some("external_link".to_string()),
                    action_type: ActionType::OpenExternal,
                    requires_confirmation: false,
                    payload: serde_json::json!({ "url": data.get("publishedUrl") }),
                },
                ArtifactAction {
                    id: "share".to_string(),
                    label: "Share".to_string(),
                    icon: Some("share".to_string()),
                    action_type: ActionType::Share,
                    requires_confirmation: false,
                    payload: serde_json::json!({}),
                },
            ],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn build_analytics_report(&self, data: &serde_json::Value) -> Artifact {
        let summary = AnalyticsSummary {
            total_views: data.get("views").and_then(|v| v.as_u64()).unwrap_or(0),
            total_likes: data.get("likes").and_then(|v| v.as_u64()).unwrap_or(0),
            total_comments: data.get("comments").and_then(|v| v.as_u64()).unwrap_or(0),
            total_shares: data.get("shares").and_then(|v| v.as_u64()).unwrap_or(0),
            avg_watch_time: data
                .get("watchTime")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            engagement_rate: data
                .get("engagementRate")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
        };

        Artifact {
            id: Uuid::new_v4(),
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            artifact_type: ArtifactType::AnalyticsReport,
            title: "Analytics Report".to_string(),
            content: ArtifactContent::AnalyticsReport {
                period: data
                    .get("period")
                    .and_then(|v| v.as_str())
                    .unwrap_or("7d")
                    .to_string(),
                summary,
                charts: vec![
                    AnalyticsChart {
                        title: "Views Over Time".to_string(),
                        chart_type: ChartType::Line,
                        data: serde_json::json!([]),
                    },
                    AnalyticsChart {
                        title: "Engagement Breakdown".to_string(),
                        chart_type: ChartType::Pie,
                        data: serde_json::json!([]),
                    },
                ],
                insights: vec![
                    "Your videos perform best when posted between 6-8 PM".to_string(),
                    "Educational content has 20% higher engagement".to_string(),
                ],
            },
            metadata: ArtifactMetadata {
                source_tool: Some("videocli_analytics".to_string()),
                source_mcp_server: Some("videocli".to_string()),
                version: 1,
                is_editable: false,
                is_downloadable: true,
                is_shareable: true,
                custom: serde_json::json!({}),
            },
            actions: vec![ArtifactAction {
                id: "export_pdf".to_string(),
                label: "Export PDF".to_string(),
                icon: Some("download".to_string()),
                action_type: ActionType::Download,
                requires_confirmation: false,
                payload: serde_json::json!({ "format": "pdf" }),
            }],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn build_filesystem_artifact(
        &self,
        tool_name: &str,
        result: &ToolCallResult,
    ) -> Option<Artifact> {
        match tool_name {
            "filesystem_read_file" => {
                let content = result.content.first().and_then(|c| match c {
                    ToolContent::Text { text } => Some(text.clone()),
                    _ => None,
                })?;

                Some(Artifact {
                    id: Uuid::new_v4(),
                    session_id: self.session_id.clone(),
                    message_id: self.message_id.clone(),
                    artifact_type: ArtifactType::Document,
                    title: "File Content".to_string(),
                    content: ArtifactContent::Document {
                        format: DocumentFormat::PlainText,
                        content,
                        sections: vec![],
                    },
                    metadata: ArtifactMetadata {
                        source_tool: Some(tool_name.to_string()),
                        source_mcp_server: Some("filesystem".to_string()),
                        version: 1,
                        is_editable: true,
                        is_downloadable: true,
                        is_shareable: false,
                        custom: serde_json::json!({}),
                    },
                    actions: vec![],
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                })
            }
            _ => None,
        }
    }

    /// Create an approval request artifact
    pub fn build_approval_request(
        &self,
        action: &str,
        description: &str,
        details: serde_json::Value,
    ) -> Artifact {
        Artifact {
            id: Uuid::new_v4(),
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            artifact_type: ArtifactType::ApprovalRequest,
            title: format!("Approval Required: {}", action),
            content: ArtifactContent::ApprovalRequest {
                action: action.to_string(),
                description: description.to_string(),
                details,
                deadline: None,
                approved: None,
            },
            metadata: ArtifactMetadata {
                source_tool: None,
                source_mcp_server: None,
                version: 1,
                is_editable: false,
                is_downloadable: false,
                is_shareable: false,
                custom: serde_json::json!({}),
            },
            actions: vec![
                ArtifactAction {
                    id: "approve".to_string(),
                    label: "Approve".to_string(),
                    icon: Some("check".to_string()),
                    action_type: ActionType::Approve,
                    requires_confirmation: false,
                    payload: serde_json::json!({}),
                },
                ArtifactAction {
                    id: "reject".to_string(),
                    label: "Reject".to_string(),
                    icon: Some("x".to_string()),
                    action_type: ActionType::Reject,
                    requires_confirmation: false,
                    payload: serde_json::json!({}),
                },
            ],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Create a document artifact
    pub fn build_document(&self, title: &str, content: &str, format: DocumentFormat) -> Artifact {
        Artifact {
            id: Uuid::new_v4(),
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            artifact_type: ArtifactType::Document,
            title: title.to_string(),
            content: ArtifactContent::Document {
                format,
                content: content.to_string(),
                sections: vec![],
            },
            metadata: ArtifactMetadata::default(),
            actions: vec![
                ArtifactAction {
                    id: "edit".to_string(),
                    label: "Edit".to_string(),
                    icon: Some("edit".to_string()),
                    action_type: ActionType::Edit,
                    requires_confirmation: false,
                    payload: serde_json::json!({}),
                },
                ArtifactAction {
                    id: "download".to_string(),
                    label: "Download".to_string(),
                    icon: Some("download".to_string()),
                    action_type: ActionType::Download,
                    requires_confirmation: false,
                    payload: serde_json::json!({}),
                },
            ],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Create a code block artifact
    pub fn build_code_block(
        &self,
        code: &str,
        language: &str,
        filename: Option<String>,
    ) -> Artifact {
        Artifact {
            id: Uuid::new_v4(),
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            artifact_type: ArtifactType::CodeBlock,
            title: filename.clone().unwrap_or_else(|| "Code".to_string()),
            content: ArtifactContent::CodeBlock {
                language: language.to_string(),
                code: code.to_string(),
                filename,
                highlights: vec![],
            },
            metadata: ArtifactMetadata {
                is_editable: true,
                is_downloadable: true,
                ..Default::default()
            },
            actions: vec![
                ArtifactAction {
                    id: "copy".to_string(),
                    label: "Copy".to_string(),
                    icon: Some("copy".to_string()),
                    action_type: ActionType::Custom,
                    requires_confirmation: false,
                    payload: serde_json::json!({ "action": "copy" }),
                },
                ArtifactAction {
                    id: "download".to_string(),
                    label: "Download".to_string(),
                    icon: Some("download".to_string()),
                    action_type: ActionType::Download,
                    requires_confirmation: false,
                    payload: serde_json::json!({}),
                },
            ],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Create a chart artifact
    pub fn build_chart(
        &self,
        title: &str,
        chart_type: ChartType,
        data: serde_json::Value,
        options: ChartOptions,
    ) -> Artifact {
        Artifact {
            id: Uuid::new_v4(),
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            artifact_type: ArtifactType::Chart,
            title: title.to_string(),
            content: ArtifactContent::Chart {
                chart_type,
                data,
                options,
            },
            metadata: ArtifactMetadata {
                is_downloadable: true,
                ..Default::default()
            },
            actions: vec![ArtifactAction {
                id: "export_image".to_string(),
                label: "Export as Image".to_string(),
                icon: Some("image".to_string()),
                action_type: ActionType::Download,
                requires_confirmation: false,
                payload: serde_json::json!({ "format": "png" }),
            }],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_new() {
        let builder = ArtifactBuilder::new("session-123".to_string(), "msg-456".to_string());
        assert_eq!(builder.session_id, "session-123");
        assert_eq!(builder.message_id, "msg-456");
    }

    #[test]
    fn test_build_approval_request() {
        let builder = ArtifactBuilder::new("session-123".to_string(), "msg-456".to_string());
        let artifact = builder.build_approval_request(
            "publish",
            "Publish video to TikTok",
            serde_json::json!({ "video_id": "v123" }),
        );

        assert_eq!(artifact.artifact_type, ArtifactType::ApprovalRequest);
        assert_eq!(artifact.session_id, "session-123");
        assert_eq!(artifact.actions.len(), 2);

        if let ArtifactContent::ApprovalRequest {
            action,
            description,
            approved,
            ..
        } = artifact.content
        {
            assert_eq!(action, "publish");
            assert_eq!(description, "Publish video to TikTok");
            assert!(approved.is_none());
        } else {
            panic!("Expected ApprovalRequest content");
        }
    }

    #[test]
    fn test_build_document() {
        let builder = ArtifactBuilder::new("session-123".to_string(), "msg-456".to_string());
        let artifact = builder.build_document("README", "# Hello World", DocumentFormat::Markdown);

        assert_eq!(artifact.artifact_type, ArtifactType::Document);
        assert_eq!(artifact.title, "README");

        if let ArtifactContent::Document {
            format, content, ..
        } = artifact.content
        {
            assert_eq!(format, DocumentFormat::Markdown);
            assert_eq!(content, "# Hello World");
        } else {
            panic!("Expected Document content");
        }
    }

    #[test]
    fn test_build_code_block() {
        let builder = ArtifactBuilder::new("session-123".to_string(), "msg-456".to_string());
        let artifact = builder.build_code_block(
            "fn main() { println!(\"Hello\"); }",
            "rust",
            Some("main.rs".to_string()),
        );

        assert_eq!(artifact.artifact_type, ArtifactType::CodeBlock);
        assert_eq!(artifact.title, "main.rs");

        if let ArtifactContent::CodeBlock {
            language,
            code,
            filename,
            ..
        } = artifact.content
        {
            assert_eq!(language, "rust");
            assert!(code.contains("println!"));
            assert_eq!(filename, Some("main.rs".to_string()));
        } else {
            panic!("Expected CodeBlock content");
        }
    }

    #[test]
    fn test_build_chart() {
        let builder = ArtifactBuilder::new("session-123".to_string(), "msg-456".to_string());
        let artifact = builder.build_chart(
            "Revenue Chart",
            ChartType::Line,
            serde_json::json!([{"x": 1, "y": 100}]),
            ChartOptions::default(),
        );

        assert_eq!(artifact.artifact_type, ArtifactType::Chart);
        assert_eq!(artifact.title, "Revenue Chart");

        if let ArtifactContent::Chart { chart_type, .. } = artifact.content {
            assert_eq!(chart_type, ChartType::Line);
        } else {
            panic!("Expected Chart content");
        }
    }

    #[test]
    fn test_from_tool_result_videocli() {
        let builder = ArtifactBuilder::new("session-123".to_string(), "msg-456".to_string());
        let result = ToolCallResult::text(
            serde_json::to_string(&serde_json::json!({
                "id": "idea-1",
                "title": "Test Video",
                "status": "draft",
                "duration": 30
            }))
            .unwrap(),
        );

        let artifact = builder.from_tool_result("videocli_create_idea", &result);
        assert!(artifact.is_some());

        let artifact = artifact.unwrap();
        assert_eq!(artifact.artifact_type, ArtifactType::VideoPreview);
    }

    #[test]
    fn test_from_tool_result_unknown() {
        let builder = ArtifactBuilder::new("session-123".to_string(), "msg-456".to_string());
        let result = ToolCallResult::text("some result");

        let artifact = builder.from_tool_result("unknown_tool", &result);
        assert!(artifact.is_none());
    }
}
