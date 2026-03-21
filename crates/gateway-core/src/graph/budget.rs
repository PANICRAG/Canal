//! Node-level token budget management.
//!
//! Provides per-node and global token budget tracking for graph execution.
//! Each node can have its own budget limit, and the system tracks consumption
//! incrementally using atomic operations for thread safety.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

/// Action to take when a budget limit is reached.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BudgetAction {
    /// Log a warning and continue.
    Warn,
    /// Reduce max_tokens for subsequent calls.
    Throttle,
    /// Terminate the node immediately.
    Terminate,
}

impl Default for BudgetAction {
    fn default() -> Self {
        Self::Terminate
    }
}

/// Budget configuration for a single node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeBudget {
    /// Maximum tokens this node may consume.
    pub max_tokens: u32,
    /// Percentage threshold (0-100) at which to emit a warning.
    pub warn_threshold_pct: u8,
    /// Action to take when the budget is exceeded.
    pub on_exceed: BudgetAction,
}

impl NodeBudget {
    /// Create a new node budget with the given max tokens and default 80% warning.
    pub fn new(max_tokens: u32) -> Self {
        Self {
            max_tokens,
            warn_threshold_pct: 80,
            on_exceed: BudgetAction::Terminate,
        }
    }

    /// Set the warning threshold percentage.
    pub fn with_warn_threshold(mut self, pct: u8) -> Self {
        self.warn_threshold_pct = pct.min(100);
        self
    }

    /// Set the action on budget exceed.
    pub fn with_action(mut self, action: BudgetAction) -> Self {
        self.on_exceed = action;
        self
    }
}

/// Budget allocation strategy for parallel branches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParallelBudgetStrategy {
    /// All branches share the remaining budget (first-come, first-served).
    Shared,
    /// Remaining budget is split equally among branches.
    EqualSplit,
    /// Remaining budget is distributed proportionally to node budgets.
    Proportional,
}

impl Default for ParallelBudgetStrategy {
    fn default() -> Self {
        Self::Shared
    }
}

/// Result of a budget check after recording token consumption.
#[derive(Debug, Clone)]
pub enum BudgetCheckResult {
    /// Consumption is within limits.
    Ok {
        /// Total tokens consumed so far.
        consumed: u32,
        /// Remaining tokens in the global budget.
        remaining: u32,
    },
    /// Consumption has crossed the warning threshold.
    Warning {
        /// Total tokens consumed in the relevant scope.
        consumed: u32,
        /// Budget limit for the relevant scope.
        limit: u32,
        /// Scope identifier (e.g., "global" or "node:xyz").
        scope: String,
    },
    /// Consumption has exceeded the budget limit.
    Exceeded {
        /// Total tokens consumed in the relevant scope.
        consumed: u32,
        /// Budget limit for the relevant scope.
        limit: u32,
        /// Scope identifier.
        scope: String,
    },
}

impl BudgetCheckResult {
    /// Returns true if the result indicates the budget is exceeded.
    pub fn is_exceeded(&self) -> bool {
        matches!(self, BudgetCheckResult::Exceeded { .. })
    }

    /// Returns true if the result indicates a warning.
    pub fn is_warning(&self) -> bool {
        matches!(self, BudgetCheckResult::Warning { .. })
    }
}

/// Execution-wide token budget tracker.
///
/// Tracks both global and per-node token consumption using atomic operations
/// for safe concurrent access from parallel branches.
pub struct ExecutionBudget {
    /// Total budget for the entire execution.
    total_budget: u32,
    /// Global consumed counter (atomic for concurrent access).
    consumed: Arc<AtomicU32>,
    /// Per-node budget configurations.
    node_budgets: HashMap<String, NodeBudget>,
    /// Per-node consumed counters (DashMap for concurrent access).
    node_consumed: DashMap<String, u32>,
    /// Parallel budget allocation strategy.
    pub parallel_strategy: ParallelBudgetStrategy,
}

impl ExecutionBudget {
    /// Create a new execution budget with the given total token limit.
    pub fn new(total: u32) -> Self {
        Self {
            total_budget: total,
            consumed: Arc::new(AtomicU32::new(0)),
            node_budgets: HashMap::new(),
            node_consumed: DashMap::new(),
            parallel_strategy: ParallelBudgetStrategy::default(),
        }
    }

    /// Add a per-node budget configuration.
    pub fn with_node_budget(mut self, node_id: &str, budget: NodeBudget) -> Self {
        self.node_budgets.insert(node_id.to_string(), budget);
        self
    }

    /// Set the parallel budget allocation strategy.
    pub fn with_parallel_strategy(mut self, strategy: ParallelBudgetStrategy) -> Self {
        self.parallel_strategy = strategy;
        self
    }

    /// Record an incremental token consumption for a node.
    ///
    /// Returns a `BudgetCheckResult` indicating whether the consumption is
    /// within limits, at the warning threshold, or has exceeded the budget.
    pub fn record(&self, node_id: &str, tokens_delta: u32) -> BudgetCheckResult {
        if tokens_delta == 0 {
            return BudgetCheckResult::Ok {
                consumed: self.consumed.load(Ordering::Relaxed),
                remaining: self.remaining(),
            };
        }

        // Global accumulation
        let new_total = self.consumed.fetch_add(tokens_delta, Ordering::Relaxed) + tokens_delta;

        // Global check
        if new_total > self.total_budget {
            return BudgetCheckResult::Exceeded {
                consumed: new_total,
                limit: self.total_budget,
                scope: "global".into(),
            };
        }

        // Per-node accumulation (DashMap atomic update)
        let mut node_entry = self.node_consumed.entry(node_id.to_string()).or_insert(0);
        *node_entry += tokens_delta;
        let node_total = *node_entry;

        // Per-node check
        if let Some(budget) = self.node_budgets.get(node_id) {
            if node_total > budget.max_tokens {
                return BudgetCheckResult::Exceeded {
                    consumed: node_total,
                    limit: budget.max_tokens,
                    scope: format!("node:{}", node_id),
                };
            }
            let threshold =
                (budget.max_tokens as f64 * budget.warn_threshold_pct as f64 / 100.0) as u32;
            if node_total > threshold {
                return BudgetCheckResult::Warning {
                    consumed: node_total,
                    limit: budget.max_tokens,
                    scope: format!("node:{}", node_id),
                };
            }
        }

        BudgetCheckResult::Ok {
            consumed: new_total,
            remaining: self.total_budget.saturating_sub(new_total),
        }
    }

    /// Get the per-branch budget for parallel execution.
    pub fn get_branch_budget(&self, branch_count: usize) -> u32 {
        let remaining = self.remaining();
        match self.parallel_strategy {
            ParallelBudgetStrategy::Shared => remaining,
            ParallelBudgetStrategy::EqualSplit => {
                if branch_count == 0 {
                    remaining
                } else {
                    remaining / branch_count as u32
                }
            }
            ParallelBudgetStrategy::Proportional => remaining,
        }
    }

    /// Get the remaining token budget.
    pub fn remaining(&self) -> u32 {
        self.total_budget
            .saturating_sub(self.consumed.load(Ordering::Relaxed))
    }

    /// Get the total consumed tokens.
    pub fn consumed(&self) -> u32 {
        self.consumed.load(Ordering::Relaxed)
    }

    /// Get the total budget.
    pub fn total_budget(&self) -> u32 {
        self.total_budget
    }

    /// Get the per-node consumed tokens.
    pub fn node_consumed(&self, node_id: &str) -> u32 {
        self.node_consumed.get(node_id).map(|v| *v).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_tracking_incremental() {
        let budget = ExecutionBudget::new(1000);

        let r1 = budget.record("node_a", 100);
        assert!(!r1.is_exceeded());
        assert!(!r1.is_warning());
        assert_eq!(budget.consumed(), 100);

        let r2 = budget.record("node_b", 100);
        assert!(!r2.is_exceeded());
        assert_eq!(budget.consumed(), 200);

        let r3 = budget.record("node_c", 100);
        assert!(!r3.is_exceeded());
        assert_eq!(budget.consumed(), 300);
        assert_eq!(budget.remaining(), 700);
    }

    #[test]
    fn test_budget_per_node_tracking() {
        let budget = ExecutionBudget::new(10000).with_node_budget("node_a", NodeBudget::new(200));

        // First call: 150 tokens → within budget
        let r1 = budget.record("node_a", 150);
        assert!(!r1.is_exceeded());
        // 150 > 160 (80% of 200)? No, 150 < 160 → Ok
        assert!(!r1.is_warning());

        // Second call: 80 more → total 230 > 200 → Exceeded
        let r2 = budget.record("node_a", 80);
        assert!(r2.is_exceeded());
        if let BudgetCheckResult::Exceeded { scope, .. } = r2 {
            assert_eq!(scope, "node:node_a");
        }
    }

    #[test]
    fn test_budget_warning_threshold() {
        let budget = ExecutionBudget::new(10000)
            .with_node_budget("node_a", NodeBudget::new(100).with_warn_threshold(80));

        // 85 tokens > 80% of 100 → Warning
        let r = budget.record("node_a", 85);
        assert!(r.is_warning());
        if let BudgetCheckResult::Warning {
            consumed,
            limit,
            scope,
        } = r
        {
            assert_eq!(consumed, 85);
            assert_eq!(limit, 100);
            assert_eq!(scope, "node:node_a");
        }
    }

    #[test]
    fn test_budget_exceeded_global() {
        let budget = ExecutionBudget::new(1000);

        budget.record("node_a", 400);
        budget.record("node_b", 400);

        // Third call: 400 more → total 1200 > 1000 → Exceeded (global)
        let r = budget.record("node_c", 400);
        assert!(r.is_exceeded());
        if let BudgetCheckResult::Exceeded { scope, .. } = r {
            assert_eq!(scope, "global");
        }
    }

    #[test]
    fn test_parallel_budget_shared() {
        let budget =
            ExecutionBudget::new(1000).with_parallel_strategy(ParallelBudgetStrategy::Shared);

        budget.record("setup", 200);
        let branch_budget = budget.get_branch_budget(5);
        assert_eq!(branch_budget, 800); // All 800 shared
    }

    #[test]
    fn test_parallel_budget_equal_split() {
        let budget =
            ExecutionBudget::new(1000).with_parallel_strategy(ParallelBudgetStrategy::EqualSplit);

        budget.record("setup", 200);
        let branch_budget = budget.get_branch_budget(4);
        assert_eq!(branch_budget, 200); // 800 / 4 = 200 each
    }

    #[test]
    fn test_budget_zero_delta() {
        let budget = ExecutionBudget::new(1000);
        let r = budget.record("node_a", 0);
        assert!(!r.is_exceeded());
        assert!(!r.is_warning());
        assert_eq!(budget.consumed(), 0);
    }

    #[test]
    fn test_concurrent_budget_recording() {
        use std::thread;

        let budget = Arc::new(ExecutionBudget::new(100_000));

        let mut handles = vec![];
        for i in 0..10 {
            let b = budget.clone();
            let node = format!("node_{}", i);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    b.record(&node, 10);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // 10 threads × 100 iterations × 10 tokens = 10000
        assert_eq!(budget.consumed(), 10_000);
        assert_eq!(budget.remaining(), 90_000);
    }

    #[test]
    fn test_node_budget_builder() {
        let nb = NodeBudget::new(500)
            .with_warn_threshold(90)
            .with_action(BudgetAction::Warn);

        assert_eq!(nb.max_tokens, 500);
        assert_eq!(nb.warn_threshold_pct, 90);
        assert!(matches!(nb.on_exceed, BudgetAction::Warn));
    }
}
