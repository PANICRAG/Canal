//! Workflow registry combining built-in and custom templates.
//!
//! Extends `TemplateRegistry` with custom template management, usage
//! tracking, and CRUD operations for user-defined workflows.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::templates::{TemplateRegistry, WorkflowTemplate};

/// Extended workflow template with usage metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserWorkflowTemplate {
    /// The underlying workflow template definition.
    pub template: WorkflowTemplate,
    /// Creator identifier.
    pub created_by: Option<String>,
    /// Whether the template is published (visible to all users).
    pub published: bool,
    /// Number of times this template has been executed.
    pub usage_count: u64,
    /// Average execution duration in milliseconds.
    pub avg_execution_ms: Option<u64>,
}

/// Summary info for listing templates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplateInfo {
    /// Template ID.
    pub id: String,
    /// Template name.
    pub name: String,
    /// Template description.
    pub description: String,
    /// Whether this is a built-in template.
    pub builtin: bool,
    /// Usage count.
    pub usage_count: u64,
    /// Average execution time.
    pub avg_execution_ms: Option<u64>,
}

/// Combined registry of built-in and custom workflow templates.
///
/// Built-in templates are read-only and always available. Custom templates
/// can be registered, updated, and deleted by users.
pub struct WorkflowRegistry {
    /// Built-in templates (read-only).
    builtins: TemplateRegistry,
    /// Custom user templates (read-write).
    custom: Arc<RwLock<HashMap<String, UserWorkflowTemplate>>>,
}

impl WorkflowRegistry {
    /// Create a new registry with built-in templates loaded.
    pub fn new() -> Self {
        Self {
            builtins: TemplateRegistry::with_builtins(),
            custom: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a custom workflow template.
    ///
    /// Returns an error if the template ID conflicts with a built-in template.
    pub async fn register_custom(&self, template: UserWorkflowTemplate) -> Result<(), String> {
        let id = &template.template.id;

        // Prevent overriding built-in templates
        if self.builtins.get(id).is_some() {
            return Err(format!("Cannot override built-in template: {}", id));
        }

        let mut custom = self.custom.write().await;
        custom.insert(id.clone(), template);
        Ok(())
    }

    /// Get a template by ID (checks builtins first, then custom).
    pub async fn get(&self, id: &str) -> Option<WorkflowTemplate> {
        // Check builtins first
        if let Some(builtin) = self.builtins.get(id) {
            return Some(builtin.clone());
        }

        // Check custom templates
        let custom = self.custom.read().await;
        custom.get(id).map(|t| t.template.clone())
    }

    /// List all available templates with summary info.
    pub async fn list_all(&self) -> Vec<WorkflowTemplateInfo> {
        let mut result = Vec::new();

        // Add builtins
        for id in self.builtins.list() {
            if let Some(template) = self.builtins.get(id) {
                result.push(WorkflowTemplateInfo {
                    id: template.id.clone(),
                    name: template.name.clone(),
                    description: template.description.clone(),
                    builtin: true,
                    usage_count: 0,
                    avg_execution_ms: None,
                });
            }
        }

        // Add custom templates
        let custom = self.custom.read().await;
        for (_, user_template) in custom.iter() {
            result.push(WorkflowTemplateInfo {
                id: user_template.template.id.clone(),
                name: user_template.template.name.clone(),
                description: user_template.template.description.clone(),
                builtin: false,
                usage_count: user_template.usage_count,
                avg_execution_ms: user_template.avg_execution_ms,
            });
        }

        result
    }

    /// Delete a custom template. Returns the deleted template, or None if not found.
    ///
    /// Cannot delete built-in templates.
    pub async fn delete_custom(&self, id: &str) -> Option<UserWorkflowTemplate> {
        let mut custom = self.custom.write().await;
        custom.remove(id)
    }

    /// Record a usage event for a template (updates count and avg execution time).
    pub async fn record_usage(&self, id: &str, duration_ms: u64) {
        let mut custom = self.custom.write().await;
        if let Some(template) = custom.get_mut(id) {
            let old_count = template.usage_count;
            let old_avg = template.avg_execution_ms.unwrap_or(0);
            template.usage_count = old_count + 1;
            // Incremental moving average
            template.avg_execution_ms = Some((old_avg * old_count + duration_ms) / (old_count + 1));
        }
    }

    /// Get access to the built-in template registry.
    pub fn builtins(&self) -> &TemplateRegistry {
        &self.builtins
    }
}

impl Default for WorkflowRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collaboration::templates::{TemplateConfig, TemplatePattern};

    fn make_custom_template(id: &str, name: &str) -> UserWorkflowTemplate {
        UserWorkflowTemplate {
            template: WorkflowTemplate {
                id: id.to_string(),
                name: name.to_string(),
                description: format!("Custom template: {}", name),
                pattern: TemplatePattern::Simple,
                default_config: TemplateConfig::default(),
            },
            created_by: Some("test_user".to_string()),
            published: false,
            usage_count: 0,
            avg_execution_ms: None,
        }
    }

    #[tokio::test]
    async fn test_workflow_registry_crud() {
        let registry = WorkflowRegistry::new();

        // Register
        let template = make_custom_template("my_workflow", "My Workflow");
        registry.register_custom(template).await.unwrap();

        // Get
        let fetched = registry.get("my_workflow").await;
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().name, "My Workflow");

        // List
        let all = registry.list_all().await;
        assert!(all.iter().any(|t| t.id == "my_workflow" && !t.builtin));
        // Should also have builtins
        assert!(all.iter().any(|t| t.builtin));

        // Delete
        let deleted = registry.delete_custom("my_workflow").await;
        assert!(deleted.is_some());
        assert!(registry.get("my_workflow").await.is_none());
    }

    #[tokio::test]
    async fn test_cannot_override_builtin() {
        let registry = WorkflowRegistry::new();

        // "simple" is a builtin template
        let template = make_custom_template("simple", "Override Simple");
        let result = registry.register_custom(template).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("built-in"));
    }

    #[tokio::test]
    async fn test_builtin_templates_accessible() {
        let registry = WorkflowRegistry::new();

        // Builtins should be accessible
        let simple = registry.get("simple").await;
        assert!(simple.is_some());

        let plan = registry.get("plan_execute").await;
        assert!(plan.is_some());
    }

    #[tokio::test]
    async fn test_template_usage_recording() {
        let registry = WorkflowRegistry::new();

        let template = make_custom_template("my_flow", "My Flow");
        registry.register_custom(template).await.unwrap();

        registry.record_usage("my_flow", 100).await;
        registry.record_usage("my_flow", 200).await;
        registry.record_usage("my_flow", 300).await;

        let all = registry.list_all().await;
        let my_flow = all.iter().find(|t| t.id == "my_flow").unwrap();
        assert_eq!(my_flow.usage_count, 3);
        assert_eq!(my_flow.avg_execution_ms, Some(200)); // (100+200+300)/3 = 200
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let registry = WorkflowRegistry::new();
        let result = registry.delete_custom("nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_register_multiple_and_get_by_id() {
        let registry = WorkflowRegistry::new();

        let t1 = make_custom_template("search_flow", "Search Flow");
        let t2 = make_custom_template("email_flow", "Email Flow");
        registry.register_custom(t1).await.unwrap();
        registry.register_custom(t2).await.unwrap();

        let search = registry.get("search_flow").await;
        assert!(search.is_some());
        assert_eq!(search.unwrap().name, "Search Flow");

        let email = registry.get("email_flow").await;
        assert!(email.is_some());
        assert_eq!(email.unwrap().name, "Email Flow");

        // Non-existent
        let none = registry.get("nonexistent").await;
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn test_list_all_includes_both() {
        let registry = WorkflowRegistry::new();

        let template = make_custom_template("custom_1", "Custom One");
        registry.register_custom(template).await.unwrap();

        let all = registry.list_all().await;
        let builtin_count = all.iter().filter(|t| t.builtin).count();
        let custom_count = all.iter().filter(|t| !t.builtin).count();

        assert!(builtin_count >= 5); // 5 built-in templates
        assert_eq!(custom_count, 1);
    }
}
