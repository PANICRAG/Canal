//! Prompt Inspector for Context Engineering v2.
//!
//! Provides detailed inspection of the composed system prompt,
//! including per-section token counts, budget utilization, and
//! content hashes for change detection.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::prompt_generator::SystemPromptGenerator;
use super::resolver::ResolvedContext;

/// Detailed inspection of a composed prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptInspection {
    /// Total estimated tokens in the prompt.
    pub total_tokens: usize,
    /// Total token budget (0 = unlimited).
    pub total_budget: usize,
    /// Token budget utilization (0.0 - 1.0).
    pub utilization: f64,
    /// Breakdown by section.
    pub sections: Vec<SectionInfo>,
    /// The full rendered prompt text.
    pub rendered_prompt: String,
}

/// Information about a single prompt section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionInfo {
    /// Section name (e.g., "L1 Platform Rules").
    pub name: String,
    /// Source file or origin of the content.
    pub source: String,
    /// Estimated token count for this section.
    pub tokens: usize,
    /// Token budget for this section (if any).
    pub budget: Option<usize>,
    /// Whether this section was truncated.
    pub truncated: bool,
    /// SHA-256 hash of the content for change detection.
    pub content_hash: String,
}

impl PromptInspection {
    /// Create a new inspection from a resolved context.
    pub fn from_resolved(ctx: &ResolvedContext, max_tokens: usize) -> Self {
        let mut sections = Vec::new();

        // L1: Platform Rules
        if !ctx.platform_rules.is_empty() {
            sections.push(SectionInfo::new(
                "L1 Platform Rules",
                "config/platform-rules.yaml",
                &ctx.platform_rules,
            ));
        }

        // L2: Organization Conventions
        if let Some(ref org) = ctx.org_conventions {
            sections.push(SectionInfo::new(
                "L2 Organization Conventions",
                "database/organizations",
                org,
            ));
        }

        // L3: User Preferences
        if let Some(ref user) = ctx.user_preferences {
            sections.push(SectionInfo::new(
                "L3 User Preferences",
                "CLAUDE.md + database/users",
                user,
            ));
        }

        // L4: Session Context
        if let Some(ref session) = ctx.session_context {
            sections.push(SectionInfo::new(
                "L4 Session Context",
                "session_state",
                session,
            ));
        }

        // Skill Descriptions
        if !ctx.skill_descriptions.is_empty() {
            sections.push(SectionInfo::new(
                "Skill Descriptions",
                "skill_registry",
                &ctx.skill_descriptions,
            ));
        }

        // Loaded Skills
        for skill in &ctx.loaded_skills {
            sections.push(SectionInfo::new(
                &format!("Loaded Skill: {}", skill.name),
                &format!("skills/{}", skill.name),
                &skill.content,
            ));
        }

        // L5: Task Instructions
        if let Some(ref task) = ctx.task_instructions {
            sections.push(SectionInfo::new(
                "L5 Task Instructions",
                "task_context",
                task,
            ));
        }

        // L6: SubAgent Context
        if let Some(ref subagent) = ctx.subagent_context {
            sections.push(SectionInfo::new(
                "L6 SubAgent Context",
                "subagent_fork",
                subagent,
            ));
        }

        let total_tokens: usize = sections.iter().map(|s| s.tokens).sum();
        let utilization = if max_tokens > 0 {
            total_tokens as f64 / max_tokens as f64
        } else {
            0.0
        };

        // Generate the full rendered prompt
        let generator = SystemPromptGenerator::new();
        let rendered_prompt = generator.generate(ctx);

        Self {
            total_tokens,
            total_budget: max_tokens,
            utilization,
            sections,
            rendered_prompt,
        }
    }
}

impl SectionInfo {
    /// Create a new section info from content.
    pub fn new(name: &str, source: &str, content: &str) -> Self {
        Self {
            name: name.to_string(),
            source: source.to_string(),
            tokens: SystemPromptGenerator::estimate_tokens(content),
            budget: None,
            truncated: false,
            content_hash: sha256_hex(content),
        }
    }

    /// Set the token budget for this section.
    pub fn with_budget(mut self, budget: usize) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Mark this section as truncated.
    pub fn with_truncated(mut self, truncated: bool) -> Self {
        self.truncated = truncated;
        self
    }
}

/// Compute SHA-256 hex digest of a string.
fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context::resolver::ResolvedContext;

    #[test]
    fn test_inspection_empty_context() {
        let ctx = ResolvedContext::default();
        let inspection = PromptInspection::from_resolved(&ctx, 8000);

        assert_eq!(inspection.total_tokens, 0);
        assert_eq!(inspection.total_budget, 8000);
        assert_eq!(inspection.utilization, 0.0);
        assert!(inspection.sections.is_empty());
    }

    #[test]
    fn test_inspection_with_platform_rules() {
        let mut ctx = ResolvedContext::default();
        ctx.platform_rules = "Always use English. Follow best practices.".to_string();

        let inspection = PromptInspection::from_resolved(&ctx, 8000);

        assert_eq!(inspection.sections.len(), 1);
        assert_eq!(inspection.sections[0].name, "L1 Platform Rules");
        assert!(inspection.sections[0].tokens > 0);
        assert!(!inspection.sections[0].content_hash.is_empty());
    }

    #[test]
    fn test_inspection_full_context() {
        let mut ctx = ResolvedContext::default();
        ctx.platform_rules = "Platform rules here".to_string();
        ctx.org_conventions = Some("Org conventions here".to_string());
        ctx.user_preferences = Some("User prefs here".to_string());
        ctx.skill_descriptions = "Skill list here".to_string();
        ctx.task_instructions = Some("Task instructions here".to_string());

        let inspection = PromptInspection::from_resolved(&ctx, 8000);

        assert_eq!(inspection.sections.len(), 5);
        assert!(inspection.total_tokens > 0);
        assert!(inspection.utilization > 0.0);
    }

    #[test]
    fn test_inspection_utilization() {
        let mut ctx = ResolvedContext::default();
        // ~250 tokens worth of content (1000 chars / 4)
        ctx.platform_rules = "x".repeat(1000);

        let inspection = PromptInspection::from_resolved(&ctx, 500);

        // Utilization should be around 0.5
        assert!(inspection.utilization > 0.3);
    }

    #[test]
    fn test_section_info_new() {
        let info = SectionInfo::new("Test Section", "test/source", "Hello world");

        assert_eq!(info.name, "Test Section");
        assert_eq!(info.source, "test/source");
        assert!(info.tokens > 0);
        assert!(!info.content_hash.is_empty());
        assert!(!info.truncated);
        assert!(info.budget.is_none());
    }

    #[test]
    fn test_section_info_builders() {
        let info = SectionInfo::new("Test", "src", "content")
            .with_budget(100)
            .with_truncated(true);

        assert_eq!(info.budget, Some(100));
        assert!(info.truncated);
    }

    #[test]
    fn test_sha256_deterministic() {
        let h1 = sha256_hex("hello");
        let h2 = sha256_hex("hello");
        assert_eq!(h1, h2);

        let h3 = sha256_hex("world");
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_inspection_serialization() {
        let mut ctx = ResolvedContext::default();
        ctx.platform_rules = "Rules".to_string();

        let inspection = PromptInspection::from_resolved(&ctx, 8000);
        let json = serde_json::to_string(&inspection).unwrap();
        let parsed: PromptInspection = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.total_budget, 8000);
        assert_eq!(parsed.sections.len(), 1);
    }

    #[test]
    fn test_unlimited_budget() {
        let ctx = ResolvedContext::default();
        let inspection = PromptInspection::from_resolved(&ctx, 0);
        assert_eq!(inspection.total_budget, 0);
        assert_eq!(inspection.utilization, 0.0);
    }
}
