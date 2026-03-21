//! WorkflowStore — trait + JSON file backend for workflow template persistence.
//!
//! Templates are stored as individual JSON files in a configurable directory.
//! The store supports CRUD operations and context-based search.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::types::ContextInfo;
use crate::workflow_template::{WorkflowTemplate, WorkflowTemplateSummary};

/// Trait for workflow template persistence.
#[async_trait]
pub trait WorkflowStore: Send + Sync {
    /// Save a template. Returns the template ID.
    async fn save(&self, template: &WorkflowTemplate) -> anyhow::Result<String>;

    /// Load a template by ID.
    async fn load(&self, id: &str) -> anyhow::Result<Option<WorkflowTemplate>>;

    /// List all saved templates (summaries only).
    async fn list(&self) -> anyhow::Result<Vec<WorkflowTemplateSummary>>;

    /// Delete a template by ID.
    async fn delete(&self, id: &str) -> anyhow::Result<()>;

    /// Find templates matching a context (app name, title pattern).
    async fn find_by_context(&self, context: &ContextInfo)
        -> anyhow::Result<Vec<WorkflowTemplate>>;
}

/// JSON file-based workflow store.
///
/// Each template is stored as `{storage_dir}/{id}.json`.
pub struct JsonWorkflowStore {
    storage_dir: PathBuf,
}

impl JsonWorkflowStore {
    /// Create a new JSON store.
    pub fn new(storage_dir: &str) -> Self {
        Self {
            storage_dir: PathBuf::from(storage_dir),
        }
    }

    /// Get the path for a template file.
    fn template_path(&self, id: &str) -> PathBuf {
        self.storage_dir.join(format!("{id}.json"))
    }
}

#[async_trait]
impl WorkflowStore for JsonWorkflowStore {
    async fn save(&self, template: &WorkflowTemplate) -> anyhow::Result<String> {
        tokio::fs::create_dir_all(&self.storage_dir).await?;
        let path = self.template_path(&template.id);
        let json = serde_json::to_string_pretty(template)?;
        tokio::fs::write(&path, json).await?;
        tracing::debug!(id = %template.id, path = ?path, "Saved workflow template");
        Ok(template.id.clone())
    }

    async fn load(&self, id: &str) -> anyhow::Result<Option<WorkflowTemplate>> {
        let path = self.template_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let json = tokio::fs::read_to_string(&path).await?;
        let template: WorkflowTemplate = serde_json::from_str(&json)?;
        Ok(Some(template))
    }

    async fn list(&self) -> anyhow::Result<Vec<WorkflowTemplateSummary>> {
        let mut summaries = Vec::new();

        if !self.storage_dir.exists() {
            return Ok(summaries);
        }

        let mut entries = tokio::fs::read_dir(&self.storage_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                match tokio::fs::read_to_string(&path).await {
                    Ok(json) => {
                        if let Ok(template) = serde_json::from_str::<WorkflowTemplate>(&json) {
                            summaries.push(WorkflowTemplateSummary {
                                id: template.id,
                                name: template.name,
                                step_count: template.steps.len(),
                                parameter_count: template.parameters.len(),
                                use_count: template.use_count,
                                success_rate: template.success_rate,
                            });
                        }
                    }
                    Err(e) => {
                        tracing::warn!(path = ?path, error = %e, "Failed to read template file");
                    }
                }
            }
        }

        Ok(summaries)
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        let path = self.template_path(id);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
            tracing::debug!(id = id, "Deleted workflow template");
        }
        Ok(())
    }

    async fn find_by_context(
        &self,
        context: &ContextInfo,
    ) -> anyhow::Result<Vec<WorkflowTemplate>> {
        let mut matches = Vec::new();

        if !self.storage_dir.exists() {
            return Ok(matches);
        }

        let mut entries = tokio::fs::read_dir(&self.storage_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(json) = tokio::fs::read_to_string(&path).await {
                    if let Ok(template) = serde_json::from_str::<WorkflowTemplate>(&json) {
                        if let Some(ref pattern) = template.context_pattern {
                            let ctx_match = context
                                .app_name
                                .as_deref()
                                .map(|a| a.contains(pattern))
                                .unwrap_or(false)
                                || context
                                    .title
                                    .as_deref()
                                    .map(|t| t.contains(pattern))
                                    .unwrap_or(false);
                            if ctx_match {
                                matches.push(template);
                            }
                        }
                    }
                }
            }
        }

        Ok(matches)
    }
}

/// In-memory workflow store for testing.
pub struct InMemoryWorkflowStore {
    templates: tokio::sync::RwLock<Vec<WorkflowTemplate>>,
}

impl InMemoryWorkflowStore {
    /// Create an empty in-memory store.
    pub fn new() -> Self {
        Self {
            templates: tokio::sync::RwLock::new(Vec::new()),
        }
    }
}

impl Default for InMemoryWorkflowStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WorkflowStore for InMemoryWorkflowStore {
    async fn save(&self, template: &WorkflowTemplate) -> anyhow::Result<String> {
        let mut templates = self.templates.write().await;
        templates.retain(|t| t.id != template.id);
        templates.push(template.clone());
        Ok(template.id.clone())
    }

    async fn load(&self, id: &str) -> anyhow::Result<Option<WorkflowTemplate>> {
        let templates = self.templates.read().await;
        Ok(templates.iter().find(|t| t.id == id).cloned())
    }

    async fn list(&self) -> anyhow::Result<Vec<WorkflowTemplateSummary>> {
        let templates = self.templates.read().await;
        Ok(templates
            .iter()
            .map(|t| WorkflowTemplateSummary {
                id: t.id.clone(),
                name: t.name.clone(),
                step_count: t.steps.len(),
                parameter_count: t.parameters.len(),
                use_count: t.use_count,
                success_rate: t.success_rate,
            })
            .collect())
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        let mut templates = self.templates.write().await;
        templates.retain(|t| t.id != id);
        Ok(())
    }

    async fn find_by_context(
        &self,
        context: &ContextInfo,
    ) -> anyhow::Result<Vec<WorkflowTemplate>> {
        let templates = self.templates.read().await;
        Ok(templates
            .iter()
            .filter(|t| {
                if let Some(ref pattern) = t.context_pattern {
                    context
                        .app_name
                        .as_deref()
                        .map(|a| a.contains(pattern))
                        .unwrap_or(false)
                        || context
                            .title
                            .as_deref()
                            .map(|t| t.contains(pattern))
                            .unwrap_or(false)
                } else {
                    false
                }
            })
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow_template::{TemplateAction, TemplateStep, WorkflowParameter};

    fn make_template(id: &str, name: &str, context_pattern: Option<&str>) -> WorkflowTemplate {
        WorkflowTemplate {
            id: id.into(),
            name: name.into(),
            description: "Test template".into(),
            steps: vec![TemplateStep {
                index: 0,
                action: TemplateAction::Click {
                    target_description: "Submit".into(),
                },
                detection_hint: "exact".into(),
            }],
            parameters: vec![WorkflowParameter {
                name: "Name".into(),
                default_value: "John".into(),
                step_index: 0,
            }],
            context_pattern: context_pattern.map(|s| s.to_string()),
            use_count: 0,
            last_used: chrono::Utc::now(),
            success_rate: 1.0,
            source_recording_id: "rec-1".into(),
        }
    }

    #[tokio::test]
    async fn test_in_memory_save_and_load() {
        let store = InMemoryWorkflowStore::new();
        let template = make_template("t1", "Test 1", None);
        store.save(&template).await.unwrap();

        let loaded = store.load("t1").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().name, "Test 1");
    }

    #[tokio::test]
    async fn test_in_memory_load_not_found() {
        let store = InMemoryWorkflowStore::new();
        let loaded = store.load("nonexistent").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_list() {
        let store = InMemoryWorkflowStore::new();
        store
            .save(&make_template("t1", "Test 1", None))
            .await
            .unwrap();
        store
            .save(&make_template("t2", "Test 2", None))
            .await
            .unwrap();

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_in_memory_delete() {
        let store = InMemoryWorkflowStore::new();
        store
            .save(&make_template("t1", "Test 1", None))
            .await
            .unwrap();
        store.delete("t1").await.unwrap();

        let loaded = store.load("t1").await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_find_by_context() {
        let store = InMemoryWorkflowStore::new();
        store
            .save(&make_template("t1", "Browser Workflow", Some("Safari")))
            .await
            .unwrap();
        store
            .save(&make_template("t2", "Editor Workflow", Some("VSCode")))
            .await
            .unwrap();

        let ctx = ContextInfo {
            url: None,
            title: None,
            app_name: Some("Safari".into()),
            interactive_elements: None,
        };
        let matches = store.find_by_context(&ctx).await.unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "Browser Workflow");
    }

    #[tokio::test]
    async fn test_in_memory_save_overwrites() {
        let store = InMemoryWorkflowStore::new();
        let mut template = make_template("t1", "Version 1", None);
        store.save(&template).await.unwrap();

        template.name = "Version 2".into();
        store.save(&template).await.unwrap();

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Version 2");
    }
}
