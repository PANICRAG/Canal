//! PRD (Product Requirements Document) types and heuristics for A43 Research Planner pipeline.
//!
//! This module provides:
//! - Core types for the research → assess → clarify → PRD → plan pipeline
//! - Code-based heuristics for complexity assessment (0 LLM calls)
//! - Template-based question generation (0 LLM calls)
//! - PRD distillation and compression for complex tasks (0 LLM calls)
//!
//! # LLM Call Budget
//!
//! Only ResearchPlanner (multi-turn agent) and PrdAssembler (1 call) use LLM.
//! All other operations in this module are pure code.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Core enums
// ============================================================================

/// Task complexity assessed by code heuristics (not LLM).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskComplexity {
    Simple,
    Medium,
    Complex,
}

impl std::fmt::Display for TaskComplexity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskComplexity::Simple => write!(f, "simple"),
            TaskComplexity::Medium => write!(f, "medium"),
            TaskComplexity::Complex => write!(f, "complex"),
        }
    }
}

/// Task type from research agent's `submit_research` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    NewFeature,
    BugFix,
    Refactor,
    Architecture,
    Config,
    Docs,
}

impl std::fmt::Display for TaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskType::NewFeature => write!(f, "new_feature"),
            TaskType::BugFix => write!(f, "bug_fix"),
            TaskType::Refactor => write!(f, "refactor"),
            TaskType::Architecture => write!(f, "architecture"),
            TaskType::Config => write!(f, "config"),
            TaskType::Docs => write!(f, "docs"),
        }
    }
}

// ============================================================================
// Research output
// ============================================================================

/// Research output from ResearchPlanner agent (structured via `submit_research` tool).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchOutput {
    /// Classified task type.
    pub task_type: TaskType,
    /// 1-3 sentence summary of what the task requires.
    pub requirements_summary: String,
    /// Key findings from codebase exploration.
    pub research_findings: String,
    /// Files that need modification.
    pub affected_files: Vec<String>,
    /// Reusable patterns found in the codebase.
    #[serde(default)]
    pub existing_patterns: Vec<String>,
    /// Possible implementation directions.
    #[serde(default)]
    pub approach_hints: Vec<String>,
}

// ============================================================================
// PRD document types
// ============================================================================

/// PRD document — fixed 8-section template, LLM fills content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrdDocument {
    /// Short descriptive title.
    pub title: String,
    /// Problem statement.
    pub problem: String,
    /// Concrete goals (2-5 items).
    pub goals: Vec<String>,
    /// Explicit non-goals (1-3 items).
    pub non_goals: Vec<String>,
    /// Technical design overview.
    pub design: String,
    /// Implementation approaches (2-3 options).
    pub approaches: Vec<ImplementationApproach>,
    /// Index of recommended approach in `approaches`.
    pub recommended_approach: usize,
    /// Identified risks with severity and mitigation.
    pub risks: Vec<Risk>,
    /// Verifiable success criteria (2-4 items).
    pub success_criteria: Vec<String>,
    /// Assessed complexity (set by code, not LLM).
    pub complexity: TaskComplexity,
    /// Research findings summary (optional, from ResearchPlanner).
    #[serde(default)]
    pub research_findings: Option<String>,
}

/// A proposed implementation approach within a PRD.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImplementationApproach {
    /// Short name for this approach.
    pub name: String,
    /// Description of what this approach entails.
    pub description: String,
    /// Estimated effort level.
    pub estimated_effort: String,
    /// Advantages of this approach.
    pub pros: Vec<String>,
    /// Disadvantages of this approach.
    pub cons: Vec<String>,
    /// Files that would be affected by this approach.
    #[serde(default)]
    pub affected_files: Vec<String>,
}

/// A risk identified during PRD generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Risk {
    /// What could go wrong.
    pub description: String,
    /// Severity: "low", "medium", or "high".
    pub severity: String,
    /// How to mitigate the risk.
    pub mitigation: String,
}

// ============================================================================
// Clarification types
// ============================================================================

/// Clarifying question — selected from template pool by code (not LLM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarifyingQuestion {
    /// Question identifier.
    pub id: u32,
    /// The question text.
    pub question: String,
    /// Expected answer format.
    pub answer_type: ClarificationAnswerType,
    /// Why this question is being asked.
    pub reason: String,
    /// Default answer (if user skips).
    #[serde(default)]
    pub default: Option<String>,
}

/// Answer format for a clarifying question.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum ClarificationAnswerType {
    /// Free-form text answer.
    Text,
    /// Choose from predefined options.
    Choice(Vec<String>),
    /// Yes/No binary answer.
    YesNo,
}

/// User's responses to clarifying questions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClarificationResponse {
    /// Map of question_id → answer text.
    pub answers: HashMap<u32, String>,
    /// If true, skip remaining unanswered questions (use defaults).
    #[serde(default)]
    pub skip_remaining: bool,
}

// ============================================================================
// PRD approval decision
// ============================================================================

/// User's decision on a generated PRD.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum PrdApprovalDecision {
    /// Approve the PRD with the chosen approach index.
    Approve { chosen_approach: usize },
    /// Request revisions with feedback.
    Revise { feedback: String },
    /// Reject the PRD entirely.
    Reject { reason: Option<String> },
}

// ============================================================================
// Distillation types (Complex coding tasks only)
// ============================================================================

/// Core concepts distilled from an expanded PRD (0 LLM calls).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConcepts {
    /// Problem essence — 1 sentence (compressed from full problem).
    pub problem_essence: String,
    /// Key decisions that influence overall direction.
    pub key_decisions: Vec<KeyDecision>,
    /// Hard constraints — non-negotiable limitations.
    pub critical_constraints: Vec<String>,
    /// Why the recommended approach was chosen.
    pub chosen_rationale: String,
}

/// A key architectural/design decision extracted from approach comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyDecision {
    /// The decision question (e.g., "Use AgentRunner or Function Calling?").
    pub question: String,
    /// What was chosen.
    pub chosen: String,
    /// Why it was chosen.
    pub reason: String,
    /// Alternatives that were rejected and why.
    pub alternatives_rejected: Vec<String>,
}

/// Compressed PRD passed to StepPlanner for complex coding tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistilledPrd {
    /// Problem essence from CoreConcepts.
    pub problem_essence: String,
    /// Only the chosen approach (not all alternatives).
    pub chosen_approach: ImplementationApproach,
    /// Key decisions with rationale.
    pub key_decisions: Vec<KeyDecision>,
    /// Hard constraints.
    pub critical_constraints: Vec<String>,
    /// All success criteria (preserved in full).
    pub success_criteria: Vec<String>,
    /// Only high-severity risks.
    pub high_risks: Vec<Risk>,
}

// ============================================================================
// Complexity assessment — pure code heuristics (0 LLM)
// ============================================================================

/// Assess task complexity based on research output using code heuristics.
///
/// Scoring factors:
/// - Affected file count: 0-1 → 0, 2-5 → +1, 6+ → +2
/// - Task type inherent complexity: bug/config/docs → 0, refactor/feature → +1, architecture → +2
/// - Multiple approach hints: 2+ → +1
/// - Research depth (long findings): >500 chars → +1
///
/// Score mapping: 0-1 → Simple, 2-3 → Medium, 4+ → Complex
pub fn assess_complexity(research: &ResearchOutput) -> TaskComplexity {
    let mut score: u32 = 0;

    // Factor 1: affected file count
    match research.affected_files.len() {
        0..=1 => {}
        2..=5 => score += 1,
        _ => score += 2,
    }

    // Factor 2: task type inherent complexity
    match research.task_type {
        TaskType::BugFix | TaskType::Config | TaskType::Docs => {}
        TaskType::Refactor | TaskType::NewFeature => score += 1,
        TaskType::Architecture => score += 2,
    }

    // Factor 3: multiple possible approaches
    if research.approach_hints.len() >= 2 {
        score += 1;
    }

    // Factor 4: research depth (long findings = more complex)
    if research.research_findings.len() > 500 {
        score += 1;
    }

    match score {
        0..=1 => TaskComplexity::Simple,
        2..=3 => TaskComplexity::Medium,
        _ => TaskComplexity::Complex,
    }
}

/// Check if a task is a "coding" type that benefits from PRD distillation.
pub fn is_coding_task(task_type: &TaskType) -> bool {
    matches!(
        task_type,
        TaskType::NewFeature | TaskType::Refactor | TaskType::Architecture
    )
}

// ============================================================================
// Question generation — template pool (0 LLM)
// ============================================================================

/// Generate clarifying questions from a template pool based on task type and complexity.
///
/// Simple tasks get no questions. Medium/Complex tasks get type-specific questions.
/// Complex tasks additionally get a scope boundary question.
pub fn get_template_questions(
    task_type: &TaskType,
    complexity: &TaskComplexity,
) -> Vec<ClarifyingQuestion> {
    if *complexity == TaskComplexity::Simple {
        return vec![];
    }

    let mut questions = vec![];

    // Universal question (medium+)
    questions.push(make_question(
        1,
        "这个改动需要保持向后兼容吗？",
        ClarificationAnswerType::YesNo,
        "影响实现策略和测试范围",
        Some("yes"),
    ));

    // Per task-type questions
    match task_type {
        TaskType::NewFeature => {
            questions.push(make_question(
                2,
                "这个功能的 MVP 范围是什么？只需要核心功能还是完整版？",
                ClarificationAnswerType::Choice(vec!["MVP核心".into(), "完整版".into()]),
                "控制实现范围",
                Some("MVP核心"),
            ));
        }
        TaskType::Refactor => {
            questions.push(make_question(
                2,
                "重构的目标是什么？性能优化 / 代码清理 / 架构升级？",
                ClarificationAnswerType::Choice(vec!["性能".into(), "清理".into(), "架构".into()]),
                "决定重构方向",
                None,
            ));
        }
        TaskType::Architecture => {
            questions.push(make_question(
                2,
                "需要保留现有 API 接口不变吗？",
                ClarificationAnswerType::YesNo,
                "影响迁移策略",
                Some("yes"),
            ));
            questions.push(make_question(
                3,
                "预期的时间范围？",
                ClarificationAnswerType::Choice(vec![
                    "1天内".into(),
                    "1周内".into(),
                    "不限".into(),
                ]),
                "影响方案选择",
                Some("1周内"),
            ));
        }
        TaskType::BugFix => {
            questions.push(make_question(
                2,
                "能复现这个 bug 吗？复现步骤是什么？",
                ClarificationAnswerType::Text,
                "帮助定位问题",
                None,
            ));
        }
        _ => {}
    }

    // Complex tasks: extra scope boundary question
    if *complexity == TaskComplexity::Complex {
        let next_id = questions.len() as u32 + 1;
        questions.push(make_question(
            next_id,
            "有哪些东西是明确不在这次改动范围内的？",
            ClarificationAnswerType::Text,
            "明确 non-goals",
            None,
        ));
    }

    questions
}

/// Helper to construct a ClarifyingQuestion.
fn make_question(
    id: u32,
    question: &str,
    answer_type: ClarificationAnswerType,
    reason: &str,
    default: Option<&str>,
) -> ClarifyingQuestion {
    ClarifyingQuestion {
        id,
        question: question.to_string(),
        answer_type,
        reason: reason.to_string(),
        default: default.map(|s| s.to_string()),
    }
}

// ============================================================================
// PRD distillation — pure code (0 LLM)
// ============================================================================

/// Truncate a string to the nearest sentence boundary within `max_chars`.
fn truncate_to_sentence(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }
    // Find byte offset of max_chars-th character (safe for CJK/emoji)
    let byte_limit = text
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    let truncated = &text[..byte_limit];
    // Find the last sentence-ending punctuation within the limit
    if let Some(pos) = truncated.rfind(|c: char| matches!(c, '。' | '.' | '！' | '!')) {
        // Include the punctuation character
        let end = pos
            + truncated[pos..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(1);
        text[..end].to_string()
    } else {
        // No sentence boundary found; hard truncate with ellipsis
        let short_limit = text
            .char_indices()
            .nth(max_chars.saturating_sub(3))
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        format!("{}...", &text[..short_limit])
    }
}

/// Extract key decisions by comparing approaches in a PRD.
fn extract_decisions_from_approaches(
    approaches: &[ImplementationApproach],
    recommended: usize,
) -> Vec<KeyDecision> {
    if approaches.len() < 2 {
        return vec![];
    }

    let chosen = &approaches[recommended.min(approaches.len() - 1)];
    let mut decisions = vec![];

    // Compare chosen approach name vs alternatives
    let alternatives: Vec<String> = approaches
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != recommended)
        .map(|(_, a)| format!("{} — {}", a.name, a.cons.join(", ")))
        .collect();

    // Primary decision: which approach
    decisions.push(KeyDecision {
        question: format!("Which approach: {} vs others?", chosen.name),
        chosen: chosen.name.clone(),
        reason: chosen.pros.join("; "),
        alternatives_rejected: alternatives,
    });

    // If approaches have different affected files, note the scope decision
    let chosen_files: std::collections::HashSet<&String> = chosen.affected_files.iter().collect();
    for (i, approach) in approaches.iter().enumerate() {
        if i == recommended {
            continue;
        }
        let other_files: std::collections::HashSet<&String> =
            approach.affected_files.iter().collect();
        let diff: Vec<_> = other_files.difference(&chosen_files).collect();
        if !diff.is_empty() {
            decisions.push(KeyDecision {
                question: format!("Scope: include files from '{}'?", approach.name),
                chosen: "No — using chosen approach's file set".to_string(),
                reason: format!("Chosen approach ({}) has a more focused scope", chosen.name),
                alternatives_rejected: vec![format!(
                    "{} would also touch: {}",
                    approach.name,
                    diff.iter()
                        .map(|f| f.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )],
            });
        }
    }

    decisions
}

/// Distill core concepts from a PRD (0 LLM calls).
///
/// Extracts the essential decision points, constraints, and rationale
/// from a full PRD document for efficient downstream consumption.
pub fn distill_core_concepts(prd: &PrdDocument) -> CoreConcepts {
    // 1. problem_essence: truncate to ~100 chars at sentence boundary
    let problem_essence = truncate_to_sentence(&prd.problem, 100);

    // 2. key_decisions: compare approaches to find decision points
    let key_decisions =
        extract_decisions_from_approaches(&prd.approaches, prd.recommended_approach);

    // 3. critical_constraints: non_goals + high-severity risk mitigations
    let mut critical_constraints: Vec<String> = prd.non_goals.clone();
    for risk in &prd.risks {
        if risk.severity == "high" {
            critical_constraints.push(risk.mitigation.clone());
        }
    }

    // 4. chosen_rationale: recommended approach's pros
    let chosen_rationale = if prd.recommended_approach < prd.approaches.len() {
        prd.approaches[prd.recommended_approach].pros.join("; ")
    } else {
        String::new()
    };

    CoreConcepts {
        problem_essence,
        key_decisions,
        critical_constraints,
        chosen_rationale,
    }
}

/// Compress a PRD into a distilled version for StepPlanner consumption (0 LLM calls).
///
/// Keeps only the chosen approach, high-severity risks, and core concepts.
/// The full PRD is shown to the user for approval; the distilled version
/// is what StepPlanner actually receives to keep prompts concise.
pub fn compress_prd(prd: &PrdDocument, core: &CoreConcepts) -> DistilledPrd {
    let chosen_approach = if prd.recommended_approach < prd.approaches.len() {
        prd.approaches[prd.recommended_approach].clone()
    } else {
        // Fallback to first approach if index is invalid
        prd.approaches
            .first()
            .cloned()
            .unwrap_or(ImplementationApproach {
                name: "default".into(),
                description: String::new(),
                estimated_effort: "hours".into(),
                pros: vec![],
                cons: vec![],
                affected_files: vec![],
            })
    };

    let high_risks: Vec<Risk> = prd
        .risks
        .iter()
        .filter(|r| r.severity == "high")
        .cloned()
        .collect();

    DistilledPrd {
        problem_essence: core.problem_essence.clone(),
        chosen_approach,
        key_decisions: core.key_decisions.clone(),
        critical_constraints: core.critical_constraints.clone(),
        success_criteria: prd.success_criteria.clone(),
        high_risks,
    }
}

// ============================================================================
// Tool definitions for LLM Function Calling
// ============================================================================

use crate::llm::router::ToolDefinition;

/// Build the `submit_research` tool definition for ResearchPlanner agent.
///
/// This tool forces the research agent to output structured findings
/// instead of free-form text.
pub fn submit_research_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: "submit_research".into(),
        description: "Submit structured research findings after exploring the codebase".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "task_type": {
                    "type": "string",
                    "enum": ["new_feature", "bug_fix", "refactor", "architecture", "config", "docs"],
                    "description": "Classified type of the task"
                },
                "requirements_summary": {
                    "type": "string",
                    "description": "1-3 sentence summary of what the task requires"
                },
                "research_findings": {
                    "type": "string",
                    "description": "Key findings from codebase exploration"
                },
                "affected_files": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files that need modification"
                },
                "existing_patterns": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Reusable patterns found in the codebase"
                },
                "approach_hints": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Possible implementation directions"
                }
            },
            "required": ["task_type", "requirements_summary", "research_findings", "affected_files"]
        }),
    }
}

/// Build the `generate_prd` tool definition for PrdAssembler.
///
/// Standard schema for Simple/Medium tasks.
/// For Complex coding tasks, use `generate_prd_expanded_tool_def()`.
pub fn generate_prd_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: "generate_prd".into(),
        description: "Generate a structured PRD document from research findings".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short descriptive title for the PRD"
                },
                "problem": {
                    "type": "string",
                    "description": "Problem statement (max 200 characters)"
                },
                "goals": {
                    "type": "array",
                    "minItems": 2,
                    "maxItems": 5,
                    "items": { "type": "string" },
                    "description": "Concrete goals (each max 100 characters)"
                },
                "non_goals": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 3,
                    "items": { "type": "string" },
                    "description": "Explicit non-goals"
                },
                "design": {
                    "type": "string",
                    "description": "Technical design overview (max 500 characters)"
                },
                "approach_a": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "effort": { "type": "string", "enum": ["minutes", "hours", "days"] },
                        "pros": { "type": "array", "maxItems": 3, "items": { "type": "string" } },
                        "cons": { "type": "array", "maxItems": 3, "items": { "type": "string" } },
                        "affected_files": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["name", "description", "effort", "pros", "cons"]
                },
                "approach_b": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "effort": { "type": "string", "enum": ["minutes", "hours", "days"] },
                        "pros": { "type": "array", "maxItems": 3, "items": { "type": "string" } },
                        "cons": { "type": "array", "maxItems": 3, "items": { "type": "string" } },
                        "affected_files": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["name", "description", "effort", "pros", "cons"]
                },
                "recommended": {
                    "type": "string",
                    "enum": ["a", "b"],
                    "description": "Which approach is recommended"
                },
                "risks": {
                    "type": "array",
                    "maxItems": 3,
                    "items": {
                        "type": "object",
                        "properties": {
                            "description": { "type": "string" },
                            "severity": { "type": "string", "enum": ["low", "medium", "high"] },
                            "mitigation": { "type": "string" }
                        },
                        "required": ["description", "severity", "mitigation"]
                    }
                },
                "success_criteria": {
                    "type": "array",
                    "minItems": 2,
                    "maxItems": 4,
                    "items": { "type": "string" },
                    "description": "Verifiable success criteria"
                }
            },
            "required": ["title", "problem", "goals", "non_goals", "design", "approach_a", "approach_b", "recommended", "success_criteria"]
        }),
    }
}

/// Build the expanded `generate_prd` tool definition for Complex coding tasks.
///
/// Relaxes limits: longer descriptions, more goals/risks, allows 3rd approach,
/// adds trade-off analysis and architecture notes.
pub fn generate_prd_expanded_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: "generate_prd".into(),
        description: "Generate an expanded PRD document for complex coding tasks".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "problem": { "type": "string", "description": "Problem statement (up to 500 characters)" },
                "goals": {
                    "type": "array", "minItems": 3, "maxItems": 8,
                    "items": { "type": "string" }
                },
                "non_goals": {
                    "type": "array", "minItems": 2, "maxItems": 5,
                    "items": { "type": "string" }
                },
                "design": { "type": "string", "description": "Technical design overview (up to 1500 characters)" },
                "approach_a": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "effort": { "type": "string", "enum": ["minutes", "hours", "days"] },
                        "pros": { "type": "array", "maxItems": 5, "items": { "type": "string" } },
                        "cons": { "type": "array", "maxItems": 5, "items": { "type": "string" } },
                        "affected_files": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["name", "description", "effort", "pros", "cons"]
                },
                "approach_b": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "effort": { "type": "string", "enum": ["minutes", "hours", "days"] },
                        "pros": { "type": "array", "maxItems": 5, "items": { "type": "string" } },
                        "cons": { "type": "array", "maxItems": 5, "items": { "type": "string" } },
                        "affected_files": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["name", "description", "effort", "pros", "cons"]
                },
                "approach_c": {
                    "type": "object",
                    "description": "Optional third approach for complex decisions",
                    "properties": {
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "effort": { "type": "string", "enum": ["minutes", "hours", "days"] },
                        "pros": { "type": "array", "maxItems": 5, "items": { "type": "string" } },
                        "cons": { "type": "array", "maxItems": 5, "items": { "type": "string" } },
                        "affected_files": { "type": "array", "items": { "type": "string" } }
                    }
                },
                "recommended": {
                    "type": "string",
                    "enum": ["a", "b", "c"],
                    "description": "Which approach is recommended"
                },
                "trade_off_analysis": {
                    "type": "string",
                    "description": "Analysis of trade-offs between approaches (up to 500 characters)"
                },
                "risks": {
                    "type": "array", "maxItems": 5,
                    "items": {
                        "type": "object",
                        "properties": {
                            "description": { "type": "string" },
                            "severity": { "type": "string", "enum": ["low", "medium", "high"] },
                            "mitigation": { "type": "string" }
                        },
                        "required": ["description", "severity", "mitigation"]
                    }
                },
                "success_criteria": {
                    "type": "array", "minItems": 3, "maxItems": 6,
                    "items": { "type": "string" }
                },
                "architecture_notes": {
                    "type": "string",
                    "description": "Architecture-level notes (up to 500 characters)"
                }
            },
            "required": ["title", "problem", "goals", "non_goals", "design", "approach_a", "approach_b", "recommended", "success_criteria"]
        }),
    }
}

/// Parse a `generate_prd` tool call response into a PrdDocument.
///
/// Handles both standard and expanded schemas. Sets complexity
/// based on the caller's assessed value.
pub fn parse_prd_response(
    input: &serde_json::Value,
    complexity: TaskComplexity,
    research_findings: Option<String>,
) -> anyhow::Result<PrdDocument> {
    let title = input["title"]
        .as_str()
        .unwrap_or("Untitled PRD")
        .to_string();
    let problem = input["problem"].as_str().unwrap_or("").to_string();
    let goals: Vec<String> = input["goals"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let non_goals: Vec<String> = input["non_goals"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let design = input["design"].as_str().unwrap_or("").to_string();

    let mut approaches = vec![];
    for key in &["approach_a", "approach_b", "approach_c"] {
        if let Some(obj) = input.get(*key).and_then(|v| v.as_object()) {
            approaches.push(parse_approach(obj));
        }
    }

    if approaches.len() < 2 {
        anyhow::bail!("PRD must have at least 2 approaches");
    }

    let recommended = match input["recommended"].as_str() {
        Some("a") => 0,
        Some("b") => 1,
        Some("c") => 2,
        _ => 0,
    };

    let risks: Vec<Risk> = input["risks"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| {
                    Some(Risk {
                        description: v["description"].as_str()?.to_string(),
                        severity: v["severity"].as_str().unwrap_or("medium").to_string(),
                        mitigation: v["mitigation"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let success_criteria: Vec<String> = input["success_criteria"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Ok(PrdDocument {
        title,
        problem,
        goals,
        non_goals,
        design,
        approaches,
        recommended_approach: recommended,
        risks,
        success_criteria,
        complexity,
        research_findings,
    })
}

fn parse_approach(obj: &serde_json::Map<String, serde_json::Value>) -> ImplementationApproach {
    ImplementationApproach {
        name: obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unnamed")
            .to_string(),
        description: obj
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        estimated_effort: obj
            .get("effort")
            .and_then(|v| v.as_str())
            .unwrap_or("hours")
            .to_string(),
        pros: obj
            .get("pros")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        cons: obj
            .get("cons")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        affected_files: obj
            .get("affected_files")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Parse a `submit_research` tool call response into a ResearchOutput.
pub fn parse_research_response(input: &serde_json::Value) -> anyhow::Result<ResearchOutput> {
    let task_type_str = input["task_type"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing task_type in research output"))?;

    let task_type = match task_type_str {
        "new_feature" => TaskType::NewFeature,
        "bug_fix" => TaskType::BugFix,
        "refactor" => TaskType::Refactor,
        "architecture" => TaskType::Architecture,
        "config" => TaskType::Config,
        "docs" => TaskType::Docs,
        other => anyhow::bail!("Unknown task_type: {}", other),
    };

    Ok(ResearchOutput {
        task_type,
        requirements_summary: input["requirements_summary"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        research_findings: input["research_findings"]
            .as_str()
            .unwrap_or("")
            .to_string(),
        affected_files: input["affected_files"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        existing_patterns: input["existing_patterns"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        approach_hints: input["approach_hints"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

// ============================================================================
// System prompts
// ============================================================================

/// Default system prompt for the ResearchPlanner agent.
pub const RESEARCH_PLANNER_SYSTEM_PROMPT: &str = r#"You are a codebase researcher. Use Read/Glob/Grep tools to explore the codebase and produce a structured research report.
You have READ-ONLY access — you cannot modify any files.

== Research Phases ==
Phase 1: SCOPE ANALYSIS — Extract goals, constraints, preferences from the user's request. Classify task type.
Phase 2: CODEBASE DISCOVERY — Use Glob to find relevant files, Grep to search for interfaces/types.
Phase 3: ARCHITECTURE MAPPING — Read core files, trace call chains, understand module boundaries.
Phase 4: IMPACT ANALYSIS — Which files need modification? Any breaking changes?
Phase 5: GAP ANALYSIS — What can be reused? What's missing?
Phase 6: OUTPUT — Call submit_research with structured findings.

== Tool Guidelines ==
- Start with Glob to find files → then Read key files → then Grep to confirm patterns
- Read at most 10 files
- Prioritize mod.rs files and type definitions
- After completing research, you MUST call submit_research with your structured findings.
- Do NOT respond with plain text — always use the submit_research tool.

== Output Quality ==
- affected_files: list ONLY files that need actual modification (not just reading)
- existing_patterns: list patterns that can be reused (e.g., "DashMap + oneshot for HITL gates")
- approach_hints: provide 1-3 possible implementation directions
"#;

/// System prompt template for the PrdAssembler.
///
/// Placeholders: {requirements_summary}, {research_findings}, {affected_files},
/// {existing_patterns}, {approach_hints}, {clarification_answers}
pub const PRD_ASSEMBLER_SYSTEM_PROMPT: &str = r#"你是 PRD 生成器。基于以下研究结果，填充 PRD 模板的每个字段。

== 研究结果 ==
需求概述: {requirements_summary}
代码分析: {research_findings}
影响文件: {affected_files}
现有模式: {existing_patterns}
方案方向: {approach_hints}
用户澄清: {clarification_answers}

== 规则 ==
1. TITLE: 简短描述性标题
2. PROBLEM: 一句话描述问题（max 200 字符）
3. GOALS: 2-5 个具体目标，每个 max 100 字符
4. NON_GOALS: 1-3 个明确不做的事
5. DESIGN: 技术设计概述（max 500 字符）
6. APPROACH_A/B: 必须提供 2 个方案，含 name/description/effort/pros/cons/affected_files
7. RISKS: 1-3 个风险 + 严重程度 + 缓解措施
8. SUCCESS_CRITERIA: 2-4 个可验证的成功标准

你必须调用 generate_prd 工具。不要用文字回复。
"#;

/// Build the PrdAssembler system prompt with research context filled in.
pub fn build_prd_assembler_prompt(
    research: &ResearchOutput,
    clarification_answers: &str,
) -> String {
    PRD_ASSEMBLER_SYSTEM_PROMPT
        .replace("{requirements_summary}", &research.requirements_summary)
        .replace("{research_findings}", &research.research_findings)
        .replace("{affected_files}", &research.affected_files.join(", "))
        .replace(
            "{existing_patterns}",
            &research.existing_patterns.join(", "),
        )
        .replace("{approach_hints}", &research.approach_hints.join(", "))
        .replace("{clarification_answers}", clarification_answers)
}

/// Build the StepPlanner context injection for Medium tasks (full PRD).
pub fn build_step_planner_prd_context(prd: &PrdDocument) -> String {
    let chosen = &prd.approaches[prd.recommended_approach.min(prd.approaches.len() - 1)];
    format!(
        "== PRD 上下文 ==\n问题: {}\n选择的方案: {} — {}\n影响文件: {}\n成功标准: {}",
        prd.problem,
        chosen.name,
        chosen.description,
        chosen.affected_files.join(", "),
        prd.success_criteria.join("; "),
    )
}

/// Build the StepPlanner context injection for Complex coding tasks (distilled PRD).
pub fn build_step_planner_distilled_context(distilled: &DistilledPrd) -> String {
    let decisions = distilled
        .key_decisions
        .iter()
        .map(|d| format!("  - {} → {}（理由: {}）", d.question, d.chosen, d.reason))
        .collect::<Vec<_>>()
        .join("\n");

    let risks = distilled
        .high_risks
        .iter()
        .map(|r| format!("  - [{}] {} → {}", r.severity, r.description, r.mitigation))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "== 核心设计决策 ==\n问题本质: {}\n方案: {} — {}\n关键决策:\n{}\n硬约束: {}\n影响文件: {}\n成功标准: {}\n高风险:\n{}",
        distilled.problem_essence,
        distilled.chosen_approach.name,
        distilled.chosen_approach.description,
        decisions,
        distilled.critical_constraints.join("; "),
        distilled.chosen_approach.affected_files.join(", "),
        distilled.success_criteria.join("; "),
        risks,
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- Serde roundtrip tests --

    #[test]
    fn test_task_complexity_serde() {
        for tc in [
            TaskComplexity::Simple,
            TaskComplexity::Medium,
            TaskComplexity::Complex,
        ] {
            let json = serde_json::to_string(&tc).unwrap();
            let decoded: TaskComplexity = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, tc);
        }
    }

    #[test]
    fn test_task_type_serde() {
        for tt in [
            TaskType::NewFeature,
            TaskType::BugFix,
            TaskType::Refactor,
            TaskType::Architecture,
            TaskType::Config,
            TaskType::Docs,
        ] {
            let json = serde_json::to_string(&tt).unwrap();
            let decoded: TaskType = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, tt);
        }
    }

    #[test]
    fn test_research_output_serde() {
        let ro = ResearchOutput {
            task_type: TaskType::NewFeature,
            requirements_summary: "Add PRD pipeline".into(),
            research_findings: "Found existing patterns in approval.rs".into(),
            affected_files: vec!["prd.rs".into(), "factory.rs".into()],
            existing_patterns: vec!["DashMap + oneshot".into()],
            approach_hints: vec!["Wrap planner".into(), "New graph nodes".into()],
        };
        let json = serde_json::to_string(&ro).unwrap();
        let decoded: ResearchOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.task_type, TaskType::NewFeature);
        assert_eq!(decoded.affected_files.len(), 2);
    }

    #[test]
    fn test_prd_document_serde() {
        let prd = make_test_prd();
        let json = serde_json::to_string(&prd).unwrap();
        let decoded: PrdDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.title, prd.title);
        assert_eq!(decoded.approaches.len(), 2);
        assert_eq!(decoded.recommended_approach, 0);
    }

    #[test]
    fn test_prd_approval_decision_serde() {
        let approve = PrdApprovalDecision::Approve { chosen_approach: 0 };
        let json = serde_json::to_string(&approve).unwrap();
        let decoded: PrdApprovalDecision = serde_json::from_str(&json).unwrap();
        match decoded {
            PrdApprovalDecision::Approve { chosen_approach } => assert_eq!(chosen_approach, 0),
            _ => panic!("Expected Approve"),
        }

        let revise = PrdApprovalDecision::Revise {
            feedback: "change design".into(),
        };
        let json = serde_json::to_string(&revise).unwrap();
        let decoded: PrdApprovalDecision = serde_json::from_str(&json).unwrap();
        match decoded {
            PrdApprovalDecision::Revise { feedback } => assert_eq!(feedback, "change design"),
            _ => panic!("Expected Revise"),
        }

        let reject = PrdApprovalDecision::Reject {
            reason: Some("not needed".into()),
        };
        let json = serde_json::to_string(&reject).unwrap();
        let decoded: PrdApprovalDecision = serde_json::from_str(&json).unwrap();
        match decoded {
            PrdApprovalDecision::Reject { reason } => {
                assert_eq!(reason, Some("not needed".into()));
            }
            _ => panic!("Expected Reject"),
        }
    }

    #[test]
    fn test_distilled_prd_serde() {
        let distilled = DistilledPrd {
            problem_essence: "Need PRD pipeline".into(),
            chosen_approach: ImplementationApproach {
                name: "Approach A".into(),
                description: "Use graph nodes".into(),
                estimated_effort: "hours".into(),
                pros: vec!["Clean".into()],
                cons: vec!["Complex".into()],
                affected_files: vec!["factory.rs".into()],
            },
            key_decisions: vec![KeyDecision {
                question: "Use graph?".into(),
                chosen: "Yes".into(),
                reason: "Better composability".into(),
                alternatives_rejected: vec!["Direct impl — less flexible".into()],
            }],
            critical_constraints: vec!["No breaking changes".into()],
            success_criteria: vec!["All tests pass".into()],
            high_risks: vec![],
        };
        let json = serde_json::to_string(&distilled).unwrap();
        let decoded: DistilledPrd = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.problem_essence, "Need PRD pipeline");
        assert_eq!(decoded.key_decisions.len(), 1);
    }

    #[test]
    fn test_core_concepts_serde() {
        let core = CoreConcepts {
            problem_essence: "Problem".into(),
            key_decisions: vec![],
            critical_constraints: vec!["constraint".into()],
            chosen_rationale: "fast".into(),
        };
        let json = serde_json::to_string(&core).unwrap();
        let decoded: CoreConcepts = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.chosen_rationale, "fast");
    }

    #[test]
    fn test_clarifying_question_serde() {
        let q = ClarifyingQuestion {
            id: 1,
            question: "Backward compat?".into(),
            answer_type: ClarificationAnswerType::YesNo,
            reason: "Affects strategy".into(),
            default: Some("yes".into()),
        };
        let json = serde_json::to_string(&q).unwrap();
        let decoded: ClarifyingQuestion = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, 1);
        assert!(matches!(
            decoded.answer_type,
            ClarificationAnswerType::YesNo
        ));
    }

    #[test]
    fn test_clarification_response_serde() {
        let mut answers = HashMap::new();
        answers.insert(1, "yes".into());
        answers.insert(2, "MVP核心".into());
        let resp = ClarificationResponse {
            answers,
            skip_remaining: false,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: ClarificationResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.answers.len(), 2);
        assert!(!decoded.skip_remaining);
    }

    // -- Complexity assessment tests --

    #[test]
    fn test_assess_complexity_simple() {
        let research = ResearchOutput {
            task_type: TaskType::BugFix,
            requirements_summary: "Fix a typo".into(),
            research_findings: "Found the typo in line 42".into(),
            affected_files: vec!["file.rs".into()],
            existing_patterns: vec![],
            approach_hints: vec!["Fix the typo".into()],
        };
        assert_eq!(assess_complexity(&research), TaskComplexity::Simple);
    }

    #[test]
    fn test_assess_complexity_medium() {
        let research = ResearchOutput {
            task_type: TaskType::NewFeature,
            requirements_summary: "Add new endpoint".into(),
            research_findings: "Need to modify routes and handler".into(),
            affected_files: vec!["routes.rs".into(), "handler.rs".into(), "types.rs".into()],
            existing_patterns: vec!["REST pattern".into()],
            approach_hints: vec!["Add route".into()],
        };
        // NewFeature(+1) + 3 files(+1) = 2 → Medium
        assert_eq!(assess_complexity(&research), TaskComplexity::Medium);
    }

    #[test]
    fn test_assess_complexity_complex() {
        let research = ResearchOutput {
            task_type: TaskType::Architecture,
            requirements_summary: "Redesign module system".into(),
            research_findings: "x".repeat(600), // >500 chars
            affected_files: (0..8).map(|i| format!("file{}.rs", i)).collect(),
            existing_patterns: vec![],
            approach_hints: vec!["Option A".into(), "Option B".into()],
        };
        // Architecture(+2) + 8 files(+2) + 2 hints(+1) + long findings(+1) = 6 → Complex
        assert_eq!(assess_complexity(&research), TaskComplexity::Complex);
    }

    #[test]
    fn test_assess_complexity_boundary() {
        // Score exactly 1 → Simple
        let research = ResearchOutput {
            task_type: TaskType::Config,
            requirements_summary: "Update config".into(),
            research_findings: "Short".into(),
            affected_files: vec!["a.yaml".into(), "b.yaml".into()],
            existing_patterns: vec![],
            approach_hints: vec![],
        };
        // Config(+0) + 2 files(+1) = 1 → Simple
        assert_eq!(assess_complexity(&research), TaskComplexity::Simple);

        // Score exactly 4 → Complex
        let research2 = ResearchOutput {
            task_type: TaskType::Architecture, // +2
            requirements_summary: "Big change".into(),
            research_findings: "x".repeat(501), // +1
            affected_files: vec!["a.rs".into(), "b.rs".into()], // +1
            existing_patterns: vec![],
            approach_hints: vec![],
        };
        assert_eq!(assess_complexity(&research2), TaskComplexity::Complex);
    }

    // -- Question generation tests --

    #[test]
    fn test_questions_simple_returns_empty() {
        let qs = get_template_questions(&TaskType::NewFeature, &TaskComplexity::Simple);
        assert!(qs.is_empty());
    }

    #[test]
    fn test_questions_medium_new_feature() {
        let qs = get_template_questions(&TaskType::NewFeature, &TaskComplexity::Medium);
        assert_eq!(qs.len(), 2); // universal + feature-specific
        assert_eq!(qs[0].id, 1); // backward compat
        assert_eq!(qs[1].id, 2); // MVP scope
    }

    #[test]
    fn test_questions_complex_architecture() {
        let qs = get_template_questions(&TaskType::Architecture, &TaskComplexity::Complex);
        // universal(1) + api compat(1) + time range(1) + scope boundary(1) = 4
        assert_eq!(qs.len(), 4);
        // Last question should be the scope boundary question
        assert!(qs.last().unwrap().question.contains("不在这次改动范围内"));
    }

    #[test]
    fn test_questions_medium_bugfix() {
        let qs = get_template_questions(&TaskType::BugFix, &TaskComplexity::Medium);
        assert_eq!(qs.len(), 2); // universal + repro steps
        assert!(qs[1].question.contains("复现"));
    }

    // -- Distillation tests --

    #[test]
    fn test_distill_core_concepts() {
        let prd = make_test_prd();
        let core = distill_core_concepts(&prd);

        assert!(!core.problem_essence.is_empty());
        assert!(!core.key_decisions.is_empty());
        assert!(!core.critical_constraints.is_empty());
        assert!(!core.chosen_rationale.is_empty());
    }

    #[test]
    fn test_distill_extracts_decisions() {
        let prd = make_test_prd();
        let core = distill_core_concepts(&prd);

        // Should have at least 1 decision (approach comparison)
        assert!(!core.key_decisions.is_empty());
        let first = &core.key_decisions[0];
        assert!(first.question.contains("Approach A"));
        assert_eq!(first.chosen, "Approach A");
        assert!(!first.alternatives_rejected.is_empty());
    }

    #[test]
    fn test_distill_merges_constraints() {
        let prd = make_test_prd();
        let core = distill_core_concepts(&prd);

        // Should include non_goals + high-severity risk mitigations
        assert!(core
            .critical_constraints
            .contains(&"Don't refactor unrelated code".to_string()));
        assert!(core
            .critical_constraints
            .contains(&"Add extensive error handling".to_string()));
    }

    #[test]
    fn test_compress_prd_keeps_only_chosen() {
        let prd = make_test_prd();
        let core = distill_core_concepts(&prd);
        let distilled = compress_prd(&prd, &core);

        assert_eq!(distilled.chosen_approach.name, "Approach A");
        assert_eq!(distilled.success_criteria.len(), prd.success_criteria.len());
    }

    #[test]
    fn test_compress_prd_filters_high_risks() {
        let prd = make_test_prd();
        let core = distill_core_concepts(&prd);
        let distilled = compress_prd(&prd, &core);

        // Only high-severity risks should be kept
        assert!(distilled.high_risks.iter().all(|r| r.severity == "high"));
        // The test PRD has 1 high risk
        assert_eq!(distilled.high_risks.len(), 1);
    }

    #[test]
    fn test_is_coding_task() {
        assert!(is_coding_task(&TaskType::NewFeature));
        assert!(is_coding_task(&TaskType::Refactor));
        assert!(is_coding_task(&TaskType::Architecture));
        assert!(!is_coding_task(&TaskType::BugFix));
        assert!(!is_coding_task(&TaskType::Config));
        assert!(!is_coding_task(&TaskType::Docs));
    }

    #[test]
    fn test_truncate_to_sentence() {
        assert_eq!(truncate_to_sentence("Short.", 100), "Short.");
        assert_eq!(
            truncate_to_sentence("First sentence. Second sentence.", 20),
            "First sentence."
        );
        assert_eq!(
            truncate_to_sentence("No period here so we hard truncate", 20),
            "No period here so..."
        );
    }

    #[test]
    fn test_task_complexity_display() {
        assert_eq!(TaskComplexity::Simple.to_string(), "simple");
        assert_eq!(TaskComplexity::Medium.to_string(), "medium");
        assert_eq!(TaskComplexity::Complex.to_string(), "complex");
    }

    #[test]
    fn test_task_type_display() {
        assert_eq!(TaskType::NewFeature.to_string(), "new_feature");
        assert_eq!(TaskType::BugFix.to_string(), "bug_fix");
    }

    // -- Test helpers --

    // -- Tool definition tests --

    #[test]
    fn test_submit_research_tool_def() {
        let def = submit_research_tool_def();
        assert_eq!(def.name, "submit_research");
        let schema = &def.input_schema;
        assert!(schema["required"].as_array().unwrap().len() >= 4);
    }

    #[test]
    fn test_generate_prd_tool_def() {
        let def = generate_prd_tool_def();
        assert_eq!(def.name, "generate_prd");
        let schema = &def.input_schema;
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("approach_a")));
        assert!(required.iter().any(|v| v.as_str() == Some("approach_b")));
    }

    #[test]
    fn test_generate_prd_expanded_tool_def() {
        let def = generate_prd_expanded_tool_def();
        assert_eq!(def.name, "generate_prd");
        let schema = &def.input_schema;
        // Expanded allows approach_c
        assert!(schema["properties"]["approach_c"].is_object());
        // And trade_off_analysis
        assert!(schema["properties"]["trade_off_analysis"].is_object());
    }

    // -- Parser tests --

    #[test]
    fn test_parse_research_response() {
        let input = serde_json::json!({
            "task_type": "new_feature",
            "requirements_summary": "Add PRD pipeline",
            "research_findings": "Found patterns",
            "affected_files": ["factory.rs", "prd.rs"],
            "existing_patterns": ["DashMap pattern"],
            "approach_hints": ["Graph nodes"]
        });
        let result = parse_research_response(&input).unwrap();
        assert_eq!(result.task_type, TaskType::NewFeature);
        assert_eq!(result.affected_files.len(), 2);
        assert_eq!(result.existing_patterns.len(), 1);
    }

    #[test]
    fn test_parse_research_response_unknown_type() {
        let input = serde_json::json!({
            "task_type": "unknown_type",
            "requirements_summary": "test",
            "research_findings": "test",
            "affected_files": []
        });
        assert!(parse_research_response(&input).is_err());
    }

    #[test]
    fn test_parse_prd_response() {
        let input = serde_json::json!({
            "title": "Test PRD",
            "problem": "Test problem",
            "goals": ["goal 1", "goal 2"],
            "non_goals": ["non-goal 1"],
            "design": "Test design",
            "approach_a": {
                "name": "Option A",
                "description": "First option",
                "effort": "hours",
                "pros": ["fast"],
                "cons": ["limited"],
                "affected_files": ["a.rs"]
            },
            "approach_b": {
                "name": "Option B",
                "description": "Second option",
                "effort": "days",
                "pros": ["thorough"],
                "cons": ["slow"]
            },
            "recommended": "a",
            "risks": [{
                "description": "risk 1",
                "severity": "medium",
                "mitigation": "mitigation 1"
            }],
            "success_criteria": ["criteria 1", "criteria 2"]
        });
        let prd = parse_prd_response(&input, TaskComplexity::Medium, None).unwrap();
        assert_eq!(prd.title, "Test PRD");
        assert_eq!(prd.approaches.len(), 2);
        assert_eq!(prd.recommended_approach, 0);
        assert_eq!(prd.complexity, TaskComplexity::Medium);
    }

    #[test]
    fn test_parse_prd_response_insufficient_approaches() {
        let input = serde_json::json!({
            "title": "Bad PRD",
            "problem": "Test",
            "goals": ["g"],
            "non_goals": ["n"],
            "design": "d",
            "approach_a": {
                "name": "Only one",
                "description": "Single",
                "effort": "hours",
                "pros": [],
                "cons": []
            },
            "recommended": "a",
            "success_criteria": ["c"]
        });
        assert!(parse_prd_response(&input, TaskComplexity::Simple, None).is_err());
    }

    // -- Prompt builder tests --

    #[test]
    fn test_build_prd_assembler_prompt() {
        let research = ResearchOutput {
            task_type: TaskType::NewFeature,
            requirements_summary: "Add feature X".into(),
            research_findings: "Found pattern Y".into(),
            affected_files: vec!["a.rs".into()],
            existing_patterns: vec!["pattern Z".into()],
            approach_hints: vec!["hint W".into()],
        };
        let prompt = build_prd_assembler_prompt(&research, "backward compat: yes");
        assert!(prompt.contains("Add feature X"));
        assert!(prompt.contains("Found pattern Y"));
        assert!(prompt.contains("a.rs"));
        assert!(prompt.contains("backward compat: yes"));
    }

    #[test]
    fn test_build_step_planner_prd_context() {
        let prd = make_test_prd();
        let ctx = build_step_planner_prd_context(&prd);
        assert!(ctx.contains("Approach A"));
        assert!(ctx.contains("factory.rs"));
    }

    #[test]
    fn test_build_step_planner_distilled_context() {
        let prd = make_test_prd();
        let core = distill_core_concepts(&prd);
        let distilled = compress_prd(&prd, &core);
        let ctx = build_step_planner_distilled_context(&distilled);
        assert!(ctx.contains("核心设计决策"));
        assert!(ctx.contains("Approach A"));
    }

    // -- Test helpers --

    fn make_test_prd() -> PrdDocument {
        PrdDocument {
            title: "Add PRD Pipeline".into(),
            problem: "Planner does not explore codebase before generating plans.".into(),
            goals: vec![
                "Research codebase before planning".into(),
                "Generate structured PRDs".into(),
            ],
            non_goals: vec!["Don't refactor unrelated code".into()],
            design: "Insert research and PRD nodes before planner in graph".into(),
            approaches: vec![
                ImplementationApproach {
                    name: "Approach A".into(),
                    description: "Add graph nodes for research + PRD".into(),
                    estimated_effort: "hours".into(),
                    pros: vec!["Composable".into(), "Testable".into()],
                    cons: vec!["More complex".into()],
                    affected_files: vec!["factory.rs".into(), "prd.rs".into()],
                },
                ImplementationApproach {
                    name: "Approach B".into(),
                    description: "Inline research into planner prompt".into(),
                    estimated_effort: "minutes".into(),
                    pros: vec!["Simple".into()],
                    cons: vec!["Not composable".into(), "Hard to test".into()],
                    affected_files: vec![
                        "planner.rs".into(),
                        "factory.rs".into(),
                        "extra.rs".into(),
                    ],
                },
            ],
            recommended_approach: 0,
            risks: vec![
                Risk {
                    description: "May break existing planner flow".into(),
                    severity: "high".into(),
                    mitigation: "Add extensive error handling".into(),
                },
                Risk {
                    description: "Research may be slow".into(),
                    severity: "low".into(),
                    mitigation: "Add timeout".into(),
                },
            ],
            success_criteria: vec![
                "Research output includes affected files".into(),
                "PRD follows template".into(),
            ],
            complexity: TaskComplexity::Complex,
            research_findings: Some("Found approval.rs pattern".into()),
        }
    }
}
