//! CvExperienceAdapter — stores and retrieves CV action experiences.
//!
//! Self-contained experience store for computer use episodic and narrative
//! memories. Uses in-memory storage with category-based filtering.
//!
//! Two memory types:
//! - **Episodic**: Individual action outcomes (per-click/type success/failure).
//! - **Narrative**: Task-level summaries (code-generated, 0 LLM tokens).

use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::ContextInfo;
use crate::workflow_recorder::WorkflowStep;

/// Episodic memory — individual action outcomes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtaskExperience {
    /// Context pattern (app name, title substring).
    pub context_pattern: String,
    /// What action was attempted.
    pub subtask_description: String,
    /// Actions that were performed.
    pub grounded_actions: Vec<super::workflow_recorder::RecordedAction>,
    /// Whether the action succeeded.
    pub success: bool,
    /// Detection method that was used.
    pub detection_method: String,
    /// pHash of the screen at action time.
    pub screen_similarity_hash: u64,
    /// When this experience was created.
    pub created_at: DateTime<Utc>,
}

/// Narrative memory — task-level summary (code-generated, not LLM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarrativeMemory {
    /// The user's original task description.
    pub task_description: String,
    /// Code-generated reflection.
    pub reflection: String,
    /// Whether the overall task succeeded.
    pub success: bool,
    /// Total number of steps.
    pub total_steps: usize,
    /// Key learnings from this task.
    pub key_learnings: Vec<String>,
    /// When this narrative was created.
    pub created_at: DateTime<Utc>,
}

/// Stored experience entry with category.
#[derive(Debug, Clone)]
struct ExperienceEntry {
    category: &'static str,
    content: String,
    context_pattern: Option<String>,
    metadata: serde_json::Value,
}

/// Adapter for storing and retrieving CV experiences.
///
/// Uses in-memory storage. In production, can be backed by the learning system.
pub struct CvExperienceAdapter {
    entries: RwLock<Vec<ExperienceEntry>>,
    max_entries: usize,
}

impl CvExperienceAdapter {
    /// Create a new experience adapter.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            max_entries: 10_000,
        }
    }

    /// Create with custom max entries.
    pub fn with_max_entries(max_entries: usize) -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            max_entries,
        }
    }

    /// Store an episodic experience (per-action outcome).
    pub fn store_episodic(&self, experience: &SubtaskExperience) {
        let content = format!(
            "CU action '{}' on '{}': {} (method: {})",
            experience.subtask_description,
            experience.context_pattern,
            if experience.success {
                "succeeded"
            } else {
                "failed"
            },
            experience.detection_method,
        );

        let entry = ExperienceEntry {
            category: "cv_episodic",
            content,
            context_pattern: Some(experience.context_pattern.clone()),
            metadata: serde_json::json!({
                "screen_hash": experience.screen_similarity_hash,
                "detection_method": experience.detection_method,
                "success": experience.success,
                "context_pattern": experience.context_pattern,
            }),
        };

        self.add_entry(entry);
    }

    /// Store a narrative memory (per-task summary) — code-based, no LLM.
    ///
    /// `task` is the user's original instruction, NOT derived from step actions.
    pub fn store_narrative(&self, task: &str, steps: &[WorkflowStep]) {
        let narrative = Self::create_narrative(task, steps);
        let content = format!(
            "CU task '{}': {} ({} steps, {} retries, {} failures). Learnings: {}",
            narrative.task_description,
            if narrative.success {
                "succeeded"
            } else {
                "failed"
            },
            narrative.total_steps,
            steps.iter().map(|s| s.retries as usize).sum::<usize>(),
            steps.iter().filter(|s| !s.verified).count(),
            narrative.key_learnings.join("; "),
        );

        let entry = ExperienceEntry {
            category: "cv_narrative",
            content,
            context_pattern: None,
            metadata: serde_json::json!({
                "task": narrative.task_description,
                "success": narrative.success,
                "total_steps": narrative.total_steps,
                "key_learnings": narrative.key_learnings,
            }),
        };

        self.add_entry(entry);
    }

    /// Query relevant experiences for current task + context.
    pub fn recall(&self, task: &str, context: Option<&ContextInfo>, limit: usize) -> Vec<String> {
        let entries = self.entries.read().unwrap();
        let task_lower = task.to_lowercase();

        let mut results: Vec<&ExperienceEntry> = entries
            .iter()
            .filter(|e| {
                // Simple keyword matching
                let content_lower = e.content.to_lowercase();
                let words: Vec<&str> = task_lower.split_whitespace().collect();
                let match_score = words.iter().filter(|w| content_lower.contains(*w)).count();
                match_score > 0
            })
            .collect();

        // Filter by context if provided
        if let Some(ctx) = context {
            results.retain(|entry| {
                entry
                    .context_pattern
                    .as_ref()
                    .map(|pattern| {
                        ctx.app_name.as_deref().unwrap_or("").contains(pattern)
                            || ctx.title.as_deref().unwrap_or("").contains(pattern)
                    })
                    .unwrap_or(true) // Keep if no pattern stored
            });
        }

        results
            .into_iter()
            .take(limit)
            .map(|e| e.content.clone())
            .collect()
    }

    /// Get the total number of stored experiences.
    pub fn count(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    /// Code-based narrative creation — no LLM, 0 tokens.
    fn create_narrative(task: &str, steps: &[WorkflowStep]) -> NarrativeMemory {
        let failed_steps: Vec<_> = steps.iter().filter(|s| !s.verified).collect();
        let retry_steps: Vec<_> = steps.iter().filter(|s| s.retries > 0).collect();

        let mut key_learnings = vec![];
        for step in &retry_steps {
            key_learnings.push(format!(
                "Step '{}' required {} retries, final method: {}",
                step.action.description(),
                step.retries,
                step.detection_method
            ));
        }
        for step in &failed_steps {
            key_learnings.push(format!(
                "Step '{}' failed: {}",
                step.action.description(),
                step.failure_reason.as_deref().unwrap_or("unknown")
            ));
        }

        NarrativeMemory {
            task_description: task.to_string(),
            reflection: format!(
                "{} steps, {} retries, {} failures",
                steps.len(),
                retry_steps.len(),
                failed_steps.len()
            ),
            success: failed_steps.is_empty(),
            total_steps: steps.len(),
            key_learnings,
            created_at: Utc::now(),
        }
    }

    /// Add an entry, enforcing max_entries limit.
    fn add_entry(&self, entry: ExperienceEntry) {
        let mut entries = self.entries.write().unwrap();
        if entries.len() >= self.max_entries {
            entries.remove(0); // LRU eviction
        }
        entries.push(entry);
    }
}

impl Default for CvExperienceAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow_recorder::RecordedAction;

    #[test]
    fn test_store_episodic() {
        let adapter = CvExperienceAdapter::new();
        let experience = SubtaskExperience {
            context_pattern: "Safari".into(),
            subtask_description: "Click Submit".into(),
            grounded_actions: vec![RecordedAction::Click {
                target_description: "Submit".into(),
                x: 100,
                y: 200,
                detection_method: "exact".into(),
            }],
            success: true,
            detection_method: "omniparser(exact)".into(),
            screen_similarity_hash: 12345,
            created_at: Utc::now(),
        };
        adapter.store_episodic(&experience);
        assert_eq!(adapter.count(), 1);
    }

    #[test]
    fn test_store_narrative() {
        let adapter = CvExperienceAdapter::new();
        let steps = vec![WorkflowStep {
            index: 0,
            action: RecordedAction::Click {
                target_description: "Submit".into(),
                x: 100,
                y: 200,
                detection_method: "exact".into(),
            },
            screenshot_before_path: None,
            screenshot_after_path: None,
            phash_before: None,
            phash_after: None,
            context_before: None,
            context_after: None,
            duration_ms: 50,
            verified: true,
            retries: 0,
            detection_method: "exact".into(),
            failure_reason: None,
        }];
        adapter.store_narrative("Fill out form", &steps);
        assert_eq!(adapter.count(), 1);
    }

    #[test]
    fn test_recall_by_keyword() {
        let adapter = CvExperienceAdapter::new();
        let experience = SubtaskExperience {
            context_pattern: "Safari".into(),
            subtask_description: "Click Submit button".into(),
            grounded_actions: vec![],
            success: true,
            detection_method: "exact".into(),
            screen_similarity_hash: 0,
            created_at: Utc::now(),
        };
        adapter.store_episodic(&experience);

        let results = adapter.recall("click submit", None, 5);
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("Click Submit"));
    }

    #[test]
    fn test_recall_no_match() {
        let adapter = CvExperienceAdapter::new();
        let experience = SubtaskExperience {
            context_pattern: "Safari".into(),
            subtask_description: "Click Submit".into(),
            grounded_actions: vec![],
            success: true,
            detection_method: "exact".into(),
            screen_similarity_hash: 0,
            created_at: Utc::now(),
        };
        adapter.store_episodic(&experience);

        let results = adapter.recall("scroll down page", None, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_recall_with_context_filter() {
        let adapter = CvExperienceAdapter::new();
        adapter.store_episodic(&SubtaskExperience {
            context_pattern: "Safari".into(),
            subtask_description: "Click Submit".into(),
            grounded_actions: vec![],
            success: true,
            detection_method: "exact".into(),
            screen_similarity_hash: 0,
            created_at: Utc::now(),
        });
        adapter.store_episodic(&SubtaskExperience {
            context_pattern: "VSCode".into(),
            subtask_description: "Click Save".into(),
            grounded_actions: vec![],
            success: true,
            detection_method: "exact".into(),
            screen_similarity_hash: 0,
            created_at: Utc::now(),
        });

        let ctx = ContextInfo {
            url: None,
            title: None,
            app_name: Some("Safari".into()),
            interactive_elements: None,
        };
        let results = adapter.recall("click", Some(&ctx), 5);
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("Submit"));
    }

    #[test]
    fn test_max_entries_eviction() {
        let adapter = CvExperienceAdapter::with_max_entries(2);
        for i in 0..3 {
            adapter.store_episodic(&SubtaskExperience {
                context_pattern: "App".into(),
                subtask_description: format!("Action {}", i),
                grounded_actions: vec![],
                success: true,
                detection_method: "exact".into(),
                screen_similarity_hash: 0,
                created_at: Utc::now(),
            });
        }
        assert_eq!(adapter.count(), 2);
    }

    #[test]
    fn test_narrative_creation() {
        let steps = vec![
            WorkflowStep {
                index: 0,
                action: RecordedAction::Click {
                    target_description: "Submit".into(),
                    x: 100,
                    y: 200,
                    detection_method: "exact".into(),
                },
                screenshot_before_path: None,
                screenshot_after_path: None,
                phash_before: None,
                phash_after: None,
                context_before: None,
                context_after: None,
                duration_ms: 50,
                verified: true,
                retries: 2,
                detection_method: "fuzzy".into(),
                failure_reason: None,
            },
            WorkflowStep {
                index: 1,
                action: RecordedAction::Type {
                    target_description: "Name".into(),
                    text: "hello".into(),
                    is_parameter: false,
                },
                screenshot_before_path: None,
                screenshot_after_path: None,
                phash_before: None,
                phash_after: None,
                context_before: None,
                context_after: None,
                duration_ms: 30,
                verified: false,
                retries: 0,
                detection_method: "keyboard".into(),
                failure_reason: Some("element not found".into()),
            },
        ];

        let narrative = CvExperienceAdapter::create_narrative("Fill form", &steps);
        assert_eq!(narrative.task_description, "Fill form");
        assert!(!narrative.success);
        assert_eq!(narrative.total_steps, 2);
        assert!(!narrative.key_learnings.is_empty());
    }
}
