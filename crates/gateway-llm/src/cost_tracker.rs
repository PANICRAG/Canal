//! Internal cost tracking for LLM usage across providers and models.
//!
//! Tracks token consumption and estimates costs based on configurable
//! per-model pricing. Uses `std::sync::RwLock` rather than tokio's async
//! mutex because all operations are fast, in-memory map lookups and
//! arithmetic — no `.await` is ever held across the lock.

use std::collections::HashMap;
use std::sync::RwLock;

use serde::Serialize;

use crate::router::Usage;

// ---------------------------------------------------------------------------
// Pricing
// ---------------------------------------------------------------------------

/// Per-model token pricing expressed in USD per million tokens.
#[derive(Debug, Clone)]
pub struct TokenPricing {
    /// Cost in USD per 1 000 000 input (prompt) tokens.
    pub input_per_million: f64,
    /// Cost in USD per 1 000 000 output (completion) tokens.
    pub output_per_million: f64,
}

// ---------------------------------------------------------------------------
// Usage record
// ---------------------------------------------------------------------------

/// Accumulated usage statistics for a single model.
#[derive(Debug, Clone, Serialize)]
pub struct ModelUsageRecord {
    /// Model identifier (as passed to `record`).
    pub model: String,
    /// Provider name derived from the model string (everything before the
    /// first `/`, or `"unknown"` when no slash is present).
    pub provider: String,
    /// Cumulative input (prompt) tokens across all requests.
    pub total_input_tokens: u64,
    /// Cumulative output (completion) tokens across all requests.
    pub total_output_tokens: u64,
    /// Number of requests recorded for this model.
    pub total_requests: u64,
    /// Running estimated cost in USD based on the configured pricing.
    pub estimated_cost_usd: f64,
}

// ---------------------------------------------------------------------------
// Cost tracker
// ---------------------------------------------------------------------------

/// Thread-safe, in-memory cost tracker.
///
/// ```text
///   ┌──────────────────────────────────────────────────────────┐
///   │  InternalCostTracker                                     │
///   │                                                          │
///   │  pricing: HashMap<model_key, TokenPricing>  (immutable)  │
///   │  records: RwLock<HashMap<model, ModelUsageRecord>>        │
///   └──────────────────────────────────────────────────────────┘
/// ```
pub struct InternalCostTracker {
    /// Accumulated per-model usage records protected by a reader-writer lock.
    records: RwLock<HashMap<String, ModelUsageRecord>>,
    /// Immutable pricing table keyed by model name / prefix.
    pricing: HashMap<String, TokenPricing>,
}

impl InternalCostTracker {
    // -- constructors -------------------------------------------------------

    /// R3-L: Create a tracker with custom pricing.
    pub fn new(pricing: HashMap<String, TokenPricing>) -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
            pricing,
        }
    }

    /// Create a tracker pre-populated with default pricing for well-known
    /// models.
    pub fn with_default_pricing() -> Self {
        let mut pricing = HashMap::new();

        pricing.insert(
            "qwen3-max".to_string(),
            TokenPricing {
                input_per_million: 1.20,
                output_per_million: 6.00,
            },
        );
        pricing.insert(
            "claude-sonnet-4".to_string(),
            TokenPricing {
                input_per_million: 3.00,
                output_per_million: 15.00,
            },
        );
        pricing.insert(
            "claude-opus-4-5".to_string(),
            TokenPricing {
                input_per_million: 5.00,
                output_per_million: 25.00,
            },
        );
        pricing.insert(
            "gemini-3-pro".to_string(),
            TokenPricing {
                input_per_million: 2.00,
                output_per_million: 12.00,
            },
        );
        pricing.insert(
            "gemini-3-flash".to_string(),
            TokenPricing {
                input_per_million: 0.50,
                output_per_million: 3.00,
            },
        );
        pricing.insert(
            "gpt-4o".to_string(),
            TokenPricing {
                input_per_million: 2.50,
                output_per_million: 10.00,
            },
        );

        Self {
            records: RwLock::new(HashMap::new()),
            pricing,
        }
    }

    // -- pricing lookup -----------------------------------------------------

    /// Resolve pricing for `model` using the following strategy:
    ///
    /// 1. Try an exact match against the pricing table.
    /// 2. Progressively shorten `model` by removing trailing characters and
    ///    try again. This means `"claude-sonnet-4-6"` will match the
    ///    `"claude-sonnet-4"` pricing entry because that is the longest
    ///    prefix in the table that matches.
    fn find_pricing(&self, model: &str) -> Option<&TokenPricing> {
        // Exact match — fast path.
        if let Some(p) = self.pricing.get(model) {
            return Some(p);
        }

        // Longest-prefix match: try every length from (model.len() - 1) down
        // to 1 and return the first hit. Because we iterate from longest to
        // shortest the first match *is* the longest prefix match.
        for end in (1..model.len()).rev() {
            // Only split on a char boundary to avoid panics with multi-byte
            // UTF-8 model names (unlikely but defensive).
            if model.is_char_boundary(end) {
                let prefix = &model[..end];
                if let Some(p) = self.pricing.get(prefix) {
                    return Some(p);
                }
            }
        }

        None
    }

    // -- recording ----------------------------------------------------------

    /// Record a single LLM request's token usage for `model`.
    ///
    /// If pricing information is available (exact or prefix match) the
    /// estimated cost is accumulated; otherwise cost remains at zero for this
    /// request but the token counts are still tracked.
    pub fn record(&self, model: &str, usage: &Usage) {
        let input_tokens = usage.prompt_tokens.max(0) as u64;
        let output_tokens = usage.completion_tokens.max(0) as u64;

        let cost = self
            .find_pricing(model)
            .map(|p| {
                (input_tokens as f64 * p.input_per_million
                    + output_tokens as f64 * p.output_per_million)
                    / 1_000_000.0
            })
            .unwrap_or(0.0);

        let provider = model
            .split('/')
            .next()
            .filter(|_s| model.contains('/'))
            .unwrap_or("unknown")
            .to_string();

        let mut records = self.records.write().unwrap_or_else(|e| e.into_inner());
        let entry = records
            .entry(model.to_string())
            .or_insert_with(|| ModelUsageRecord {
                model: model.to_string(),
                provider: provider.clone(),
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_requests: 0,
                estimated_cost_usd: 0.0,
            });

        entry.total_input_tokens += input_tokens;
        entry.total_output_tokens += output_tokens;
        entry.total_requests += 1;
        entry.estimated_cost_usd += cost;
    }

    // -- queries ------------------------------------------------------------

    /// Return a snapshot of all recorded models sorted alphabetically by
    /// model name.
    pub fn get_summary(&self) -> Vec<ModelUsageRecord> {
        let records = self.records.read().unwrap_or_else(|e| e.into_inner());
        let mut summary: Vec<ModelUsageRecord> = records.values().cloned().collect();
        summary.sort_by(|a, b| a.model.cmp(&b.model));
        summary
    }

    /// Return the usage record for a single model, if any requests have been
    /// recorded for it.
    pub fn get_model_cost(&self, model: &str) -> Option<ModelUsageRecord> {
        let records = self.records.read().unwrap_or_else(|e| e.into_inner());
        records.get(model).cloned()
    }

    /// Return the sum of `estimated_cost_usd` across all models.
    pub fn get_total_cost_usd(&self) -> f64 {
        let records = self.records.read().unwrap_or_else(|e| e.into_inner());
        records.values().map(|r| r.estimated_cost_usd).sum()
    }

    // -- mutation ------------------------------------------------------------

    /// Clear all accumulated usage records. Pricing is left intact.
    pub fn reset(&self) {
        let mut records = self.records.write().unwrap_or_else(|e| e.into_inner());
        records.clear();
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::Usage;

    /// Helper to build a `Usage` value quickly.
    fn usage(prompt: i32, completion: i32) -> Usage {
        Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
        }
    }

    #[test]
    fn test_default_pricing_is_populated() {
        let tracker = InternalCostTracker::with_default_pricing();
        assert!(tracker.pricing.contains_key("gpt-4o"));
        assert!(tracker.pricing.contains_key("claude-sonnet-4"));
        assert!(tracker.pricing.contains_key("claude-opus-4-5"));
        assert!(tracker.pricing.contains_key("gemini-3-pro"));
        assert!(tracker.pricing.contains_key("gemini-3-flash"));
        assert!(tracker.pricing.contains_key("qwen3-max"));
    }

    #[test]
    fn test_exact_pricing_match() {
        let tracker = InternalCostTracker::with_default_pricing();
        let p = tracker.find_pricing("gpt-4o").expect("should match");
        assert!((p.input_per_million - 2.50).abs() < f64::EPSILON);
        assert!((p.output_per_million - 10.00).abs() < f64::EPSILON);
    }

    #[test]
    fn test_prefix_pricing_match() {
        let tracker = InternalCostTracker::with_default_pricing();
        // A versioned model name should still resolve via prefix.
        let p = tracker
            .find_pricing("claude-sonnet-4-6")
            .expect("should match prefix");
        assert!((p.input_per_million - 3.00).abs() < f64::EPSILON);
    }

    #[test]
    fn test_longest_prefix_wins() {
        let mut pricing = HashMap::new();
        pricing.insert(
            "gemini-3".to_string(),
            TokenPricing {
                input_per_million: 0.10,
                output_per_million: 0.20,
            },
        );
        pricing.insert(
            "gemini-3-pro".to_string(),
            TokenPricing {
                input_per_million: 2.00,
                output_per_million: 12.00,
            },
        );
        let tracker = InternalCostTracker {
            records: RwLock::new(HashMap::new()),
            pricing,
        };
        let p = tracker
            .find_pricing("gemini-3-pro-latest")
            .expect("should match longest prefix");
        // Should match "gemini-3-pro", not "gemini-3".
        assert!((p.input_per_million - 2.00).abs() < f64::EPSILON);
    }

    #[test]
    fn test_no_pricing_match() {
        let tracker = InternalCostTracker::with_default_pricing();
        assert!(tracker.find_pricing("totally-unknown-model").is_none());
    }

    #[test]
    fn test_record_and_get_model_cost() {
        let tracker = InternalCostTracker::with_default_pricing();
        tracker.record("gpt-4o", &usage(1_000_000, 500_000));

        let record = tracker.get_model_cost("gpt-4o").expect("should exist");
        assert_eq!(record.total_input_tokens, 1_000_000);
        assert_eq!(record.total_output_tokens, 500_000);
        assert_eq!(record.total_requests, 1);
        // Cost = (1_000_000 * 2.50 + 500_000 * 10.00) / 1_000_000
        //      = 2.50 + 5.00 = 7.50
        assert!((record.estimated_cost_usd - 7.50).abs() < 1e-9);
    }

    #[test]
    fn test_record_accumulates() {
        let tracker = InternalCostTracker::with_default_pricing();
        tracker.record("gpt-4o", &usage(100, 200));
        tracker.record("gpt-4o", &usage(300, 400));

        let record = tracker.get_model_cost("gpt-4o").expect("should exist");
        assert_eq!(record.total_input_tokens, 400);
        assert_eq!(record.total_output_tokens, 600);
        assert_eq!(record.total_requests, 2);
    }

    #[test]
    fn test_negative_tokens_clamped_to_zero() {
        let tracker = InternalCostTracker::with_default_pricing();
        tracker.record("gpt-4o", &usage(-50, -100));

        let record = tracker.get_model_cost("gpt-4o").expect("should exist");
        assert_eq!(record.total_input_tokens, 0);
        assert_eq!(record.total_output_tokens, 0);
        assert!((record.estimated_cost_usd).abs() < f64::EPSILON);
    }

    #[test]
    fn test_unknown_model_records_tokens_but_zero_cost() {
        let tracker = InternalCostTracker::with_default_pricing();
        tracker.record("mystery-llm", &usage(1000, 2000));

        let record = tracker.get_model_cost("mystery-llm").expect("should exist");
        assert_eq!(record.total_input_tokens, 1000);
        assert_eq!(record.total_output_tokens, 2000);
        assert!((record.estimated_cost_usd).abs() < f64::EPSILON);
    }

    #[test]
    fn test_provider_extraction_with_slash() {
        let tracker = InternalCostTracker::with_default_pricing();
        tracker.record("anthropic/claude-sonnet-4", &usage(10, 10));

        let record = tracker
            .get_model_cost("anthropic/claude-sonnet-4")
            .expect("should exist");
        assert_eq!(record.provider, "anthropic");
    }

    #[test]
    fn test_provider_defaults_to_unknown() {
        let tracker = InternalCostTracker::with_default_pricing();
        tracker.record("gpt-4o", &usage(10, 10));

        let record = tracker.get_model_cost("gpt-4o").expect("should exist");
        assert_eq!(record.provider, "unknown");
    }

    #[test]
    fn test_get_summary_sorted() {
        let tracker = InternalCostTracker::with_default_pricing();
        tracker.record("gpt-4o", &usage(10, 10));
        tracker.record("claude-sonnet-4", &usage(10, 10));
        tracker.record("gemini-3-flash", &usage(10, 10));

        let summary = tracker.get_summary();
        assert_eq!(summary.len(), 3);
        assert_eq!(summary[0].model, "claude-sonnet-4");
        assert_eq!(summary[1].model, "gemini-3-flash");
        assert_eq!(summary[2].model, "gpt-4o");
    }

    #[test]
    fn test_get_total_cost_usd() {
        let tracker = InternalCostTracker::with_default_pricing();
        tracker.record("gpt-4o", &usage(1_000_000, 0));
        // Cost for gpt-4o: 1_000_000 * 2.50 / 1_000_000 = 2.50
        tracker.record("claude-sonnet-4", &usage(0, 1_000_000));
        // Cost for claude-sonnet-4: 1_000_000 * 15.00 / 1_000_000 = 15.00

        let total = tracker.get_total_cost_usd();
        assert!((total - 17.50).abs() < 1e-9);
    }

    #[test]
    fn test_reset_clears_records() {
        let tracker = InternalCostTracker::with_default_pricing();
        tracker.record("gpt-4o", &usage(100, 200));
        assert!(!tracker.get_summary().is_empty());

        tracker.reset();
        assert!(tracker.get_summary().is_empty());
        assert!(tracker.get_model_cost("gpt-4o").is_none());
        assert!((tracker.get_total_cost_usd()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_prefix_match_on_record() {
        let tracker = InternalCostTracker::with_default_pricing();
        // Record with a versioned model name that should prefix-match
        // "claude-opus-4-5" pricing ($5.00 / $25.00).
        tracker.record("claude-opus-4-5-20251101", &usage(1_000_000, 1_000_000));

        let record = tracker
            .get_model_cost("claude-opus-4-5-20251101")
            .expect("should exist");
        // Cost = (1M * 5.00 + 1M * 25.00) / 1M = 30.00
        assert!((record.estimated_cost_usd - 30.00).abs() < 1e-9);
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let tracker = Arc::new(InternalCostTracker::with_default_pricing());
        let mut handles = vec![];

        for i in 0..10 {
            let t = Arc::clone(&tracker);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    t.record("gpt-4o", &usage(10, 20));
                }
                // Interleave reads to exercise the RwLock under contention.
                if i % 2 == 0 {
                    let _ = t.get_summary();
                } else {
                    let _ = t.get_total_cost_usd();
                }
            }));
        }

        for h in handles {
            h.join().expect("thread panicked");
        }

        let record = tracker.get_model_cost("gpt-4o").expect("should exist");
        assert_eq!(record.total_requests, 1000);
        assert_eq!(record.total_input_tokens, 10_000);
        assert_eq!(record.total_output_tokens, 20_000);
    }
}
