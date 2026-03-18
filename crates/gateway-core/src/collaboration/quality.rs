//! Quality gate evaluation for specialist output.
//!
//! Quality gates determine whether a specialist's output meets the
//! required quality threshold. They are used by the ExpertOrchestrator
//! to decide whether to accept or retry specialist work.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Result of a quality gate evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityResult {
    /// Whether the output passed the quality gate.
    pub passed: bool,
    /// Quality score (0.0 to 1.0).
    pub score: f32,
    /// Optional feedback explaining the score.
    pub feedback: Option<String>,
}

/// Trait for evaluating the quality of specialist output.
///
/// Quality gates are used by the ExpertOrchestrator to decide whether
/// a specialist's output should be accepted or whether the task should
/// be retried with a different specialist.
#[async_trait]
pub trait QualityGate: Send + Sync {
    /// Evaluate the quality of a specialist's output.
    ///
    /// # Arguments
    /// * `task` - The original task description
    /// * `result` - The specialist's output
    ///
    /// # Returns
    /// A `QualityResult` with pass/fail, score, and optional feedback.
    async fn evaluate(&self, task: &str, result: &str) -> QualityResult;
}

/// A simple threshold-based quality gate.
///
/// Always returns a fixed score and passes if the result is non-empty
/// and the score meets the threshold.
pub struct ThresholdQualityGate {
    /// Minimum score to pass.
    threshold: f32,
}

impl ThresholdQualityGate {
    /// Create a new threshold gate.
    pub fn new(threshold: f32) -> Self {
        Self {
            threshold: threshold.clamp(0.0, 1.0),
        }
    }
}

#[async_trait]
impl QualityGate for ThresholdQualityGate {
    async fn evaluate(&self, _task: &str, result: &str) -> QualityResult {
        // Simple heuristic: non-empty results with sufficient length pass.
        let score = if result.is_empty() {
            0.0
        } else if result.len() < 10 {
            0.3
        } else if result.len() < 50 {
            0.6
        } else {
            0.9
        };

        QualityResult {
            passed: score >= self.threshold,
            score,
            feedback: if score < self.threshold {
                Some(format!(
                    "Score {:.2} below threshold {:.2}",
                    score, self.threshold
                ))
            } else {
                None
            },
        }
    }
}

/// Composite quality gate that requires all sub-gates to pass.
pub struct CompositeQualityGate {
    gates: Vec<Box<dyn QualityGate>>,
}

impl CompositeQualityGate {
    /// Create a new composite gate.
    pub fn new(gates: Vec<Box<dyn QualityGate>>) -> Self {
        Self { gates }
    }
}

#[async_trait]
impl QualityGate for CompositeQualityGate {
    async fn evaluate(&self, task: &str, result: &str) -> QualityResult {
        let mut min_score = 1.0_f32;
        let mut all_passed = true;
        let mut feedback_parts = Vec::new();

        for gate in &self.gates {
            let qr = gate.evaluate(task, result).await;
            min_score = min_score.min(qr.score);
            if !qr.passed {
                all_passed = false;
            }
            if let Some(fb) = qr.feedback {
                feedback_parts.push(fb);
            }
        }

        QualityResult {
            passed: all_passed,
            score: min_score,
            feedback: if feedback_parts.is_empty() {
                None
            } else {
                Some(feedback_parts.join("; "))
            },
        }
    }
}

// R2-L151: AlwaysPassGate and AlwaysFailGate moved to #[cfg(test)] — only used in tests
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An always-passing quality gate for testing.
    pub struct AlwaysPassGate;

    #[async_trait]
    impl QualityGate for AlwaysPassGate {
        async fn evaluate(&self, _task: &str, _result: &str) -> QualityResult {
            QualityResult {
                passed: true,
                score: 1.0,
                feedback: None,
            }
        }
    }

    /// An always-failing quality gate for testing.
    pub struct AlwaysFailGate {
        feedback: String,
    }

    impl AlwaysFailGate {
        /// Create a new always-fail gate with custom feedback.
        pub fn new(feedback: impl Into<String>) -> Self {
            Self {
                feedback: feedback.into(),
            }
        }
    }

    #[async_trait]
    impl QualityGate for AlwaysFailGate {
        async fn evaluate(&self, _task: &str, _result: &str) -> QualityResult {
            QualityResult {
                passed: false,
                score: 0.0,
                feedback: Some(self.feedback.clone()),
            }
        }
    }
    use super::*;

    #[tokio::test]
    async fn test_threshold_gate_passes() {
        let gate = ThresholdQualityGate::new(0.5);
        let result = gate
            .evaluate(
                "Write a poem",
                "This is a sufficiently long result text that should pass the gate easily.",
            )
            .await;
        assert!(result.passed);
        assert!(result.score >= 0.5);
        assert!(result.feedback.is_none());
    }

    #[tokio::test]
    async fn test_threshold_gate_fails_empty() {
        let gate = ThresholdQualityGate::new(0.5);
        let result = gate.evaluate("Write a poem", "").await;
        assert!(!result.passed);
        assert_eq!(result.score, 0.0);
        assert!(result.feedback.is_some());
    }

    #[tokio::test]
    async fn test_threshold_gate_short_result() {
        let gate = ThresholdQualityGate::new(0.5);
        let result = gate.evaluate("task", "short").await;
        assert!(!result.passed);
        assert!(result.score < 0.5);
    }

    #[tokio::test]
    async fn test_threshold_gate_medium_result() {
        let gate = ThresholdQualityGate::new(0.5);
        let result = gate
            .evaluate("task", "This is a medium-length result text.")
            .await;
        assert!(result.passed);
        assert!(result.score >= 0.5);
    }

    #[tokio::test]
    async fn test_composite_gate_all_pass() {
        let gate =
            CompositeQualityGate::new(vec![Box::new(AlwaysPassGate), Box::new(AlwaysPassGate)]);
        let result = gate.evaluate("task", "result").await;
        assert!(result.passed);
        assert_eq!(result.score, 1.0);
    }

    #[tokio::test]
    async fn test_composite_gate_one_fails() {
        let gate = CompositeQualityGate::new(vec![
            Box::new(AlwaysPassGate),
            Box::new(AlwaysFailGate::new("not good enough")),
        ]);
        let result = gate.evaluate("task", "result").await;
        assert!(!result.passed);
        assert_eq!(result.score, 0.0);
        assert!(result.feedback.unwrap().contains("not good enough"));
    }

    #[tokio::test]
    async fn test_always_pass_gate() {
        let gate = AlwaysPassGate;
        let result = gate.evaluate("any", "any").await;
        assert!(result.passed);
        assert_eq!(result.score, 1.0);
    }

    #[tokio::test]
    async fn test_always_fail_gate() {
        let gate = AlwaysFailGate::new("reason");
        let result = gate.evaluate("any", "any").await;
        assert!(!result.passed);
        assert_eq!(result.score, 0.0);
        assert_eq!(result.feedback.unwrap(), "reason");
    }

    #[tokio::test]
    async fn test_threshold_clamping() {
        let gate = ThresholdQualityGate::new(1.5); // should clamp to 1.0
        let result = gate
            .evaluate(
                "task",
                "This is long enough to get a 0.9 score from the heuristic",
            )
            .await;
        assert!(!result.passed); // 0.9 < 1.0 (clamped)

        let gate = ThresholdQualityGate::new(-0.5); // should clamp to 0.0
        let result = gate.evaluate("task", "").await;
        assert!(result.passed); // 0.0 >= 0.0
    }
}
